use self::quote::{
    QUOTE_CACHE_TTL, QuoteCacheKey, bitcoin_health_check_with_retry, make_quote,
    reserve_proof_with_timeout, unlocked_monero_balance_with_timeout,
};
use crate::asb::{Behaviour, OutEvent};
use crate::monero;
use crate::network::cooperative_xmr_redeem_after_punish::CooperativeXmrRedeemRejectReason;
use crate::network::cooperative_xmr_redeem_after_punish::Response::{Fullfilled, Rejected};
use crate::network::quote::{BidQuote, RefundPolicyWire};
use crate::network::swap_setup::alice::WalletSnapshot;
use crate::network::transfer_proof;
use crate::protocol::alice::swap::has_already_processed_enc_sig;
use crate::protocol::alice::{AliceState, HermesFundingPolicy, State3, Swap, TipConfig};
use crate::protocol::{Database, State};
use anyhow::{Context, Result, anyhow, bail};
use bitcoin_wallet::BitcoinWallet;
use futures::future;
use futures::future::{BoxFuture, FutureExt};
use futures::stream::{FuturesUnordered, StreamExt};
use libp2p::metrics::{Metrics, Recorder};
use libp2p::request_response::{OutboundFailure, OutboundRequestId, ResponseChannel};
use libp2p::swarm::SwarmEvent;
use libp2p::{PeerId, Swarm};
use moka::sync::Cache;
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use std::collections::HashMap;
use std::convert::TryInto;
use std::fmt::Debug;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use swap_core::bitcoin;
use swap_env::config::RefundPolicy;
use swap_env::env;
use swap_feed::LatestRate;
use swap_p2p::protocols::cooperative_xmr_redeem_after_punish;
use tokio::sync::{mpsc, oneshot};
use tor_hsservice::RunningOnionService;
use uuid::Uuid;

pub use service::{EventLoopRequest, EventLoopService, OnionServiceStatusInfo};

#[allow(missing_debug_implementations)]
pub struct EventLoop<LR>
where
    LR: LatestRate + Send + 'static + Debug + Clone,
{
    swarm: libp2p::Swarm<Behaviour<LR>>,
    metrics: Option<Metrics>,
    env_config: env::Config,
    bitcoin_wallet: Arc<dyn BitcoinWallet>,
    monero_wallet: Arc<monero::Wallets>,
    db: Arc<dyn Database + Send + Sync>,
    latest_rate: LR,
    min_buy: bitcoin::Amount,
    max_buy: bitcoin::Amount,
    external_redeem_address: Option<bitcoin::Address>,
    btc_redeem_fee_multiplier: Decimal,
    developer_tip: TipConfig,
    hermes_funding_policy: HermesFundingPolicy,
    refund_policy: RefundPolicy,

    config_path: PathBuf,

    /// Cache for quotes
    quote_cache: Cache<QuoteCacheKey, Result<Arc<BidQuote>, Arc<anyhow::Error>>>,

    swap_sender: mpsc::Sender<Swap>,

    /// Stores where to send [`EncryptedSignature`]s to
    /// The corresponding receiver for this channel is stored in the EventLoopHandle
    /// that is responsible for the swap.
    ///
    /// Once a [`EncryptedSignature`] has been sent to the EventLoopHandle,
    /// the sender is removed from this map.
    recv_encrypted_signature: HashMap<Uuid, bmrng::RequestSender<bitcoin::EncryptedSignature, ()>>,

    /// Stores where to send burn-on-refund instructions to
    /// The corresponding receiver is stored in the EventLoopHandle
    /// Uses watch channel to allow multiple updates before consumption
    recv_burn_on_refund_instruction: HashMap<Uuid, tokio::sync::watch::Sender<Option<bool>>>,

    /// Once we receive an [`EncryptedSignature`] from Bob, we forward it to the EventLoopHandle.
    /// Once the EventLoopHandle acknowledges the receipt of the [`EncryptedSignature`], we need to confirm this to Bob.
    /// When the EventLoopHandle acknowledges the receipt, a future in this collection resolves and returns the libp2p channel
    /// which we use to confirm to Bob that we have received the [`EncryptedSignature`].
    ///
    /// Flow:
    /// 1. When signature forwarded via recv_encrypted_signature sender
    /// 2. New future pushed here to await EventLoopHandle's acknowledgement
    /// 3. When future completes, the EventLoop uses the ResponseChannel to send an acknowledgment to Bob
    /// 4. Future is removed from this collection
    inflight_encrypted_signatures: FuturesUnordered<BoxFuture<'static, ResponseChannel<()>>>,

    /// In-flight quote computation. At most one real future at a time;
    /// a permanent `pending()` sentinel keeps the stream alive.
    inflight_quote_computation:
        FuturesUnordered<BoxFuture<'static, Result<Arc<BidQuote>, Arc<anyhow::Error>>>>,

    /// Response channels waiting for the in-flight quote computation to finish.
    /// Drained once the computation resolves.
    pending_quote_channels: HashMap<PeerId, ResponseChannel<BidQuote>>,

    /// Controller RPC responders waiting for the in-flight quote computation.
    /// Drained alongside `pending_quote_channels` when the computation resolves.
    pending_quote_controller_responders:
        Vec<oneshot::Sender<Result<Arc<BidQuote>, Arc<anyhow::Error>>>>,

    /// In-flight wallet snapshot computations for swap setup.
    /// Each future waits for a single swap setup handler to request a wallet snapshot.
    /// It then computes the wallet snapshot and returns the BTC amount, responder and wallet snapshot.
    #[allow(clippy::type_complexity)]
    inflight_wallet_snapshots: FuturesUnordered<
        BoxFuture<
            'static,
            Result<(
                bitcoin::Amount,
                bmrng::Responder<(WalletSnapshot, bitcoin::Amount, bool)>,
                WalletSnapshot,
            )>,
        >,
    >,

    /// Channel for sending transfer proofs to Bobs. The sender is shared with every EventLoopHandle.
    /// The receiver is polled by the event loop to send transfer proofs over the network to Bob.
    ///
    /// Flow:
    /// 1. EventLoopHandle sends (PeerId, Request, Responder) through sender
    /// 2. Event loop receives and attempts to send to peer
    /// 3. Result (Ok or network failure) is sent back to EventLoopHandle
    #[allow(clippy::type_complexity)]
    outgoing_transfer_proofs_requests: tokio::sync::mpsc::UnboundedReceiver<(
        PeerId,
        transfer_proof::Request,
        oneshot::Sender<Result<(), OutboundFailure>>,
    )>,
    #[allow(clippy::type_complexity)]
    outgoing_transfer_proofs_sender: tokio::sync::mpsc::UnboundedSender<(
        PeerId,
        transfer_proof::Request,
        oneshot::Sender<Result<(), OutboundFailure>>,
    )>,

    /// Channel for service requests
    service_requests: mpsc::UnboundedReceiver<EventLoopRequest>,

    /// Handle to the primary onion service (if registered)
    onion_service_handle: Option<Arc<RunningOnionService>>,

    /// Temporarily stores transfer proof requests for peers that are currently disconnected.
    ///
    /// When a transfer proof cannot be sent because there's no connection to the peer:
    /// 1. It is moved from [`outgoing_transfer_proofs_requests`] to this buffer
    /// 2. Once a connection is established with the peer, the proof is send back into the [`outgoing_transfer_proofs_sender`]
    /// 3. The buffered request is then removed from this collection
    #[allow(clippy::type_complexity)]
    buffered_transfer_proofs: HashMap<
        PeerId,
        Vec<(
            transfer_proof::Request,
            oneshot::Sender<Result<(), OutboundFailure>>,
        )>,
    >,

    /// Tracks [`transfer_proof::Request`]s which are currently inflight and awaiting an acknowledgement from Bob
    ///
    /// When a transfer proof is sent to Bob:
    /// 1. A unique request ID is generated by libp2p
    /// 2. The response channel is stored in this map with the request ID as key
    /// 3. When Bob acknowledges the proof, we use the stored channel to notify the EventLoopHandle
    /// 4. The entry is then removed from this map
    inflight_transfer_proofs:
        HashMap<OutboundRequestId, oneshot::Sender<Result<(), OutboundFailure>>>,
}

impl<LR> EventLoop<LR>
where
    LR: LatestRate + Send + 'static + Debug + Clone,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        swarm: Swarm<Behaviour<LR>>,
        metrics: Option<Metrics>,
        env_config: env::Config,
        bitcoin_wallet: Arc<dyn BitcoinWallet>,
        monero_wallet: Arc<monero::Wallets>,
        db: Arc<dyn Database + Send + Sync>,
        latest_rate: LR,
        min_buy: bitcoin::Amount,
        max_buy: bitcoin::Amount,
        external_redeem_address: Option<bitcoin::Address>,
        btc_redeem_fee_multiplier: Decimal,
        developer_tip: TipConfig,
        hermes_funding_policy: HermesFundingPolicy,
        refund_policy: RefundPolicy,
        onion_service_handle: Option<Arc<RunningOnionService>>,
        config_path: PathBuf,
    ) -> Result<(Self, mpsc::Receiver<Swap>, EventLoopService)> {
        let swap_channel = MpscChannels::default();
        let (outgoing_transfer_proofs_sender, outgoing_transfer_proofs_requests) =
            tokio::sync::mpsc::unbounded_channel();
        let (service_sender, service_requests) = mpsc::unbounded_channel();

        let quote_cache = Cache::builder().time_to_live(QUOTE_CACHE_TTL).build();

        let event_loop = EventLoop {
            swarm,
            metrics,
            env_config,
            bitcoin_wallet,
            monero_wallet,
            db,
            latest_rate,
            swap_sender: swap_channel.sender,
            min_buy,
            max_buy,
            external_redeem_address,
            btc_redeem_fee_multiplier,
            developer_tip,
            hermes_funding_policy,
            refund_policy,
            config_path,
            quote_cache,
            recv_encrypted_signature: Default::default(),
            recv_burn_on_refund_instruction: Default::default(),
            inflight_encrypted_signatures: Default::default(),
            inflight_quote_computation: Default::default(),
            pending_quote_channels: Default::default(),
            pending_quote_controller_responders: Default::default(),
            inflight_wallet_snapshots: Default::default(),
            outgoing_transfer_proofs_requests,
            outgoing_transfer_proofs_sender,
            service_requests,
            onion_service_handle,
            buffered_transfer_proofs: Default::default(),
            inflight_transfer_proofs: Default::default(),
        };

        let service = EventLoopService::new(service_sender);

        Ok((event_loop, swap_channel.receiver, service))
    }

    pub fn peer_id(&self) -> PeerId {
        *Swarm::local_peer_id(&self.swarm)
    }

    pub fn external_addresses(&self) -> Vec<libp2p::Multiaddr> {
        self.swarm.external_addresses().cloned().collect()
    }

    pub async fn run(mut self) {
        // ensure that these streams are NEVER empty, otherwise it will
        // terminate forever.
        self.inflight_encrypted_signatures
            .push(future::pending().boxed());
        self.inflight_quote_computation
            .push(future::pending().boxed());
        self.inflight_wallet_snapshots
            .push(future::pending().boxed());

        let swaps = match self.db.all().await {
            Ok(swaps) => swaps,
            Err(e) => {
                tracing::error!("Failed to load swaps from database: {}", e);
                return;
            }
        };

        let unfinished_swaps = swaps
            .into_iter()
            .filter(|(_, _, state)| !state.swap_finished())
            .collect::<Vec<_>>();

        for (peer_id, swap_id, state) in unfinished_swaps {
            let handle = self.new_handle(peer_id, swap_id);

            let swap = Swap {
                event_loop_handle: handle,
                bitcoin_wallet: self.bitcoin_wallet.clone(),
                monero_wallet: self.monero_wallet.clone(),
                env_config: self.env_config,
                db: self.db.clone(),
                state: state.try_into().expect("Alice state loaded from db"),
                swap_id,
                developer_tip: self.developer_tip.clone(),
                hermes_funding_policy: self.hermes_funding_policy,
            };

            match self.swap_sender.send(swap).await {
                Ok(_) => tracing::info!(%swap_id, "Resuming swap"),
                Err(_) => {
                    tracing::warn!(%swap_id, "Failed to resume swap because receiver has been dropped")
                }
            }
        }

        loop {
            tokio::select! {
                swarm_event = self.swarm.select_next_some() => {
                    if let Some(metrics) = &self.metrics {
                        metrics.record(&swarm_event);
                    }

                    match swarm_event {
                        SwarmEvent::Behaviour(OutEvent::SwapSetupInitiated { mut send_wallet_snapshot }) => {
                            let bitcoin_wallet = self.bitcoin_wallet.clone();
                            let monero_wallet = self.monero_wallet.clone();
                            let external_redeem_address = self.external_redeem_address.clone();
                            let btc_redeem_fee_multiplier = self.btc_redeem_fee_multiplier;

                            self.inflight_wallet_snapshots.push(async move {
                                // Wait for the swap setup handler to request the wallet snapshot
                                let (btc, responder) = send_wallet_snapshot.recv().await?;

                                // Compute the wallet snapshot
                                let wallet_snapshot = capture_wallet_snapshot(bitcoin_wallet, &monero_wallet, &external_redeem_address, btc_redeem_fee_multiplier, btc).await?;

                                // This is used further down to then actually respond to the swap setup handler
                                Ok((btc, responder, wallet_snapshot))
                            }.boxed());
                        }
                        SwarmEvent::Behaviour(OutEvent::SwapSetupCompleted{peer_id, swap_id, state3}) => {
                            if let Err(error) = self.handle_execution_setup_done(peer_id, swap_id, state3).await {
                                tracing::error!(%swap_id, ?error, "Failed to handle execution setup done");
                            }
                        }
                        SwarmEvent::Behaviour(OutEvent::SwapDeclined { peer, error }) => {
                            tracing::warn!(%peer, "Ignoring spot price request: {}", error);
                        }
                        SwarmEvent::Behaviour(OutEvent::QuoteRequested { channel, peer }) => {
                            if let Some(quote) = self.fresh_quote() {
                                if self.swarm.behaviour_mut().quote.send_response(channel, quote).is_err() {
                                    tracing::debug!(%peer, "Failed to respond with quote");
                                }
                            } else {
                                self.pending_quote_channels.insert(peer, channel);
                                self.ensure_quote_computation_is_inflight();
                            }
                        }
                        SwarmEvent::Behaviour(OutEvent::TransferProofAcknowledged { peer, id }) => {
                            tracing::debug!(%peer, "Bob acknowledged transfer proof");

                            if let Some(responder) = self.inflight_transfer_proofs.remove(&id) {
                                let _ = responder.send(Ok(()));
                            }
                        }
                        SwarmEvent::Behaviour(OutEvent::EncryptedSignatureReceived{ msg, channel, peer }) => {
                            let swap_id = msg.swap_id;
                            let swap_peer = self.db.get_peer_id(swap_id).await;

                            // Ensure that an incoming encrypted signature is sent by the peer-id associated with the swap
                            let swap_peer = match swap_peer {
                                Ok(swap_peer) => swap_peer,
                                Err(_) => {
                                    tracing::warn!(
                                        unknown_swap_id = %swap_id,
                                        from = %peer,
                                        "Ignoring encrypted signature for unknown swap");

                                    if let Ok(()) = self.swarm.disconnect_peer_id(peer) {
                                        tracing::debug!(%peer, "Disconnected peer for malicious encrypted signature request")
                                    }

                                    continue;
                                }
                            };

                            if swap_peer != peer {
                                tracing::warn!(
                                    %swap_id,
                                    received_from = %peer,
                                    expected_from = %swap_peer,
                                    "Ignoring malicious encrypted signature which was not expected from this peer",
                                    );

                                if let Ok(()) = self.swarm.disconnect_peer_id(peer) {
                                    tracing::debug!(%peer, "Disconnected peer for malicious encrypted signature request")
                                }

                                continue;
                            }

                            // Immediately acknowledge if we've already processed this encrypted signature
                            // This handles the case where Bob didn't receive our previous acknowledgment
                            // and is retrying sending the encrypted signature
                            if let Ok(state) = self.db.get_state(swap_id).await {
                                let state: AliceState = state.try_into()
                                    .expect("Alices database only contains Alice states");

                                // Check if we have already processed the encrypted signature
                                if has_already_processed_enc_sig(&state) {
                                    tracing::warn!(%swap_id, "Received encrypted signature for swap in state {}. We have already processed this encrypted signature. Acknowledging immediately.", state);

                                    // We push create a future that will resolve immediately, and returns the channel
                                    // This will be resolved in the next iteration of the event loop, and the acknowledgment will be sent to Bob
                                    self.inflight_encrypted_signatures.push(async move {
                                        channel
                                    }.boxed());

                                    continue;
                                }
                            }

                            let sender = match self.recv_encrypted_signature.remove(&swap_id) {
                                Some(sender) => sender,
                                None => {
                                    // TODO: Don't just drop encsig if we currently don't have a running swap for it, save in db
                                    // 1. Save the encrypted signature in the database
                                    // 2. Acknowledge the receipt of the encrypted signature
                                    tracing::warn!(%swap_id, "No sender for encrypted signature, maybe already handled?");
                                    continue;
                                }
                            };

                            let mut responder = match sender.send(msg.tx_redeem_encsig).await {
                                Ok(responder) => responder,
                                Err(_) => {
                                    tracing::warn!(%swap_id, "Failed to relay encrypted signature to swap");
                                    continue;
                                }
                            };

                            self.inflight_encrypted_signatures.push(async move {
                                let _ = responder.recv().await;

                                channel
                            }.boxed());
                        }
                        SwarmEvent::Behaviour(OutEvent::CooperativeXmrRedeemRequested { swap_id, channel, peer }) => {
                            let _ = self.handle_cooperative_redeem_request(swap_id, channel, peer).await
                                .inspect_err(|err| tracing::error!(error=?err, "Could not process cooperative redeem request, ignoring"));
                        }
                        SwarmEvent::Behaviour(OutEvent::Rendezvous(swap_p2p::protocols::rendezvous::register::Event::Registered { peer_id })) => {
                            tracing::trace!("Successfully registered with rendezvous node: {}", peer_id);
                        }
                        SwarmEvent::Behaviour(OutEvent::Rendezvous(swap_p2p::protocols::rendezvous::register::Event::RegisterRequestFailed { peer_id, error })) => {
                            tracing::trace!("Registration with rendezvous node {} failed: {:?}", peer_id, error);
                        }
                        SwarmEvent::Behaviour(OutEvent::Rendezvous(swap_p2p::protocols::rendezvous::register::Event::RegisterDispatchFailed { peer_id, error })) => {
                            tracing::trace!("Failed to dispatch registration to rendezvous node {}: {:?}", peer_id, error);
                        }
                        SwarmEvent::Behaviour(OutEvent::OutboundRequestResponseFailure {peer, error, request_id, protocol}) => {
                            tracing::error!(
                                %peer,
                                %request_id,
                                ?error,
                                %protocol,
                                "Failed to send request-response request to peer");

                            if let Some(responder) = self.inflight_transfer_proofs.remove(&request_id) {
                                let _ = responder.send(Err(error));
                            }
                        }
                        SwarmEvent::Behaviour(OutEvent::InboundRequestResponseFailure {peer, error, request_id, protocol}) => {
                            tracing::error!(
                                %peer,
                                %request_id,
                                ?error,
                                %protocol,
                                "Failed to receive request-response request from peer");
                        }
                        SwarmEvent::Behaviour(OutEvent::Failure {peer, error}) => {
                            tracing::error!(
                                %peer,
                                "Communication error: {:?}", error);
                        }
                        SwarmEvent::ConnectionEstablished { peer_id: peer, endpoint, .. } => {
                            tracing::trace!(%peer, address = %endpoint.get_remote_address(), "New connection established");

                            // If we have buffered transfer proofs for this peer, we can now send them
                            if let Some(transfer_proofs) = self.buffered_transfer_proofs.remove(&peer) {
                                for (transfer_proof, responder) in transfer_proofs {
                                    tracing::debug!(%peer, "Found buffered transfer proof for peer");

                                    // We have an established connection to the peer, so we can add the transfer proof to the queue
                                    // This is then polled in the next iteration of the event loop, and attempted to be sent to the peer
                                    if let Err(e) = self.outgoing_transfer_proofs_sender.send((peer, transfer_proof, responder)) {
                                        tracing::error!(%peer, error = ?e, "Failed to forward buffered transfer proof to event loop channel");
                                    }
                                }
                            }
                        }
                        SwarmEvent::IncomingConnectionError { send_back_addr: address, error, .. } => {
                            if let libp2p::swarm::ListenError::Denied { cause } = &error {
                                if let Some(exceeded) = cause.downcast_ref::<libp2p::connection_limits::Exceeded>() {
                                    tracing::warn!(%address, error = %exceeded, "Rejected inbound connection to prevent against denial-of-service");
                                } else {
                                    tracing::trace!(%address, "Failed to set up connection with peer: {:?}", error);
                                }
                            } else {
                                tracing::trace!(%address, "Failed to set up connection with peer: {:?}", error);
                            }
                        }
                        SwarmEvent::ConnectionClosed { peer_id: peer, num_established: 0, endpoint, cause: Some(error), connection_id } => {
                            tracing::trace!(%peer, address = %endpoint.get_remote_address(), %connection_id, "Lost connection to peer: {:?}", error);
                        }
                        SwarmEvent::ConnectionClosed { peer_id: peer, num_established: 0, endpoint, cause: None, connection_id } => {
                            tracing::trace!(%peer, address = %endpoint.get_remote_address(), %connection_id,  "Successfully closed connection");
                        }
                        SwarmEvent::Behaviour(OutEvent::Ping(ping_event)) => {
                            if let Some(metrics) = &self.metrics {
                                metrics.record(&ping_event);
                            }
                        }
                        SwarmEvent::Behaviour(OutEvent::Identify(identify_event)) => {
                            if let Some(metrics) = &self.metrics {
                                metrics.record(identify_event.as_ref());
                            }
                        }
                        SwarmEvent::NewListenAddr{address, .. } => {
                            let multiaddr = format!("{address}/p2p/{}", self.swarm.local_peer_id());
                            tracing::info!(%address, %multiaddr, "New listen address reported");

                            if let Ok(path) = std::env::var("ASB_DEV_ADDR_OUTPUT_PATH") {
                                if !multiaddr.contains("/ip4/127.0.0.1/") { continue; }
                                let Ok(mut file) = std::fs::File::create(&path) else { continue; };
                                let Ok(_) = writeln!(&mut file, "VITE_TESTNET_STUB_PROVIDER_ADDRESS={multiaddr}") else {
                                    tracing::error!("Couldn't write multiaddr to `{path}`");
                                    continue;
                                };
                                tracing::info!("Wrote multiaddr to `{path}`");
                            }
                        }
                        _ => {}
                    }
                },
                Some((peer, transfer_proof, responder)) = self.outgoing_transfer_proofs_requests.recv() => {
                    // If we are not connected to the peer, we buffer the transfer proof
                    if !self.swarm.behaviour_mut().transfer_proof.is_connected(&peer) {
                        tracing::warn!(%peer, "No active connection to peer, buffering transfer proof");
                        self.buffered_transfer_proofs.entry(peer).or_default().push((transfer_proof, responder));
                        continue;
                    }

                    // If we are connected to the peer, we attempt to send the transfer proof
                    let id = self.swarm.behaviour_mut().transfer_proof.send_request(&peer, transfer_proof);
                    self.inflight_transfer_proofs.insert(id, responder);
                },
                Some(response_channel) = self.inflight_encrypted_signatures.next() => {
                    let _ = self.swarm.behaviour_mut().encrypted_signature.send_response(response_channel, ());
                },
                Some(quote_result) = self.inflight_quote_computation.next() => {
                    let quote = match &quote_result {
                        Ok(quote_arc) => (**quote_arc).clone(),
                        // We respond with a zero quote. This will stop Bob from trying to start a swap but doesn't require
                        // a breaking network change by changing the definition of the quote protocol
                        //
                        // The error is already logged in the make_quote_or_use_cached function
                        // We don't log it here to avoid spamming on each request
                        Err(_) => BidQuote::ZERO,
                    };

                    tracing::trace!(?quote, num_requests = self.pending_quote_channels.len(), "Responding with quote to requests");

                    for (peer, channel) in self.pending_quote_channels.drain() {
                        if self.swarm.behaviour_mut().quote.send_response(channel, quote.clone()).is_err() {
                            tracing::debug!(%peer, "Failed to respond with quote");
                        }
                    }

                    // Also respond to any controller RPC callers waiting on this computation.
                    for responder in self.pending_quote_controller_responders.drain(..) {
                        let _ = responder.send(quote_result.clone());
                    }
                },

                // Swap setup routine:
                // 1. We receive a `SwapSetupInitiated` event with a `send_wallet_snapshot` receiver
                // 2. We push a future to `inflight_wallet_snapshots` that waits for the swap setup handler to
                //    request the wallet snapshot (with the BTC amount), then computes it
                // 3. Once the future resolves, we compute the amnesty amount and respond to the swap setup handler
                Some(result) = self.inflight_wallet_snapshots.next() => {
                    let (btc, responder, wallet_snapshot) = match result {
                        Ok((btc, responder, wallet_snapshot)) => (btc, responder, wallet_snapshot),
                        Err(error) => {
                            // TODO: Propagate error to the swap_setup handler instead of swallowing it
                            tracing::error!("Swap request will be ignored because we were unable to create wallet snapshot for swap: {:#}", error);
                            continue;
                        }
                    };

                    let (btc_amnesty_amount, should_publish_tx_withhold) = match apply_anti_spam_policy(btc, &self.refund_policy) {
                        Ok(amount) => amount,
                        Err(error) => {
                            // TODO: Propagate error to the swap_setup handler instead of swallowing it
                            tracing::error!("Swap request will be ignored because we were unable to compute the amnesty amount for the swap: {:#}", error);
                            continue;
                        }
                    };

                    if responder.respond((wallet_snapshot, btc_amnesty_amount, should_publish_tx_withhold)).is_err() {
                        tracing::warn!("Failed to send wallet snapshot and amnesty amount back to swap setup handler, connection may have been dropped");
                    }
                },
                Some(request) = self.service_requests.recv() => {
                    match request {
                        EventLoopRequest::GetMultiaddresses { respond_to } => {
                            let peer_id = *self.swarm.local_peer_id();
                            let addresses = self.swarm.external_addresses().cloned().collect();
                            let _ = respond_to.send((peer_id, addresses));
                        }
                        EventLoopRequest::GetActiveConnections { respond_to } => {
                            let count = self.swarm.connected_peers().count();
                            let _ = respond_to.send(count);
                        }
                        EventLoopRequest::GetRegistrationStatus { respond_to } => {
                            let registrations = self
                                .swarm
                                .behaviour()
                                .rendezvous
                                .as_ref()
                                .map(|b| b.status())
                                .unwrap_or_default(); // If rendezvous behaviour is disabled we report empty list

                            let _ = respond_to.send(registrations);
                        }
                        EventLoopRequest::SetBurnOnRefund { swap_id, burn, respond_to } => {
                            let result = if let Some(sender) = self.recv_burn_on_refund_instruction.get(&swap_id) {
                                sender.send(Some(burn))
                                    .map_err(|_| anyhow!("Failed to send burn instruction - receiver dropped"))
                            } else {
                                Err(anyhow!("No active swap found with id {}", swap_id))
                            };
                            let _ = respond_to.send(result);
                        }
                        EventLoopRequest::GrantMercy { swap_id, respond_to } => {
                            let result = self.handle_grant_mercy(swap_id).await;
                            let _ = respond_to.send(result);
                        }
                        EventLoopRequest::GetWormholeServices { respond_to } => {
                            let services = self.swarm.behaviour().wormhole
                                .as_ref()
                                .map(|w| w.services())
                                .unwrap_or_default();
                            let _ = respond_to.send(services);
                        }
                        EventLoopRequest::GetOnionServiceStatus { respond_to } => {
                            let info = self.onion_service_handle.as_ref().map(|svc| {
                                let status = svc.status();
                                OnionServiceStatusInfo {
                                    state: format!("{:?}", status.state()),
                                    reachable: status.state().is_fully_reachable(),
                                    problem: status.current_problem().map(|p| format!("{p:?}")),
                                }
                            });
                            let _ = respond_to.send(info);
                        }
                        EventLoopRequest::GetCurrentQuote { respond_to } => {
                            self.pending_quote_controller_responders.push(respond_to);
                            self.ensure_quote_computation_is_inflight();
                        }
                        EventLoopRequest::SetExternalBitcoinRedeemAddress { address, respond_to } => {
                            let result = self.handle_set_external_bitcoin_redeem_address(address).await;
                            let _ = respond_to.send(result);
                        }
                        EventLoopRequest::GetExternalBitcoinRedeemAddress { respond_to } => {
                            let _ = respond_to.send(self.external_redeem_address.clone());
                        }
                    }
                }
            }
        }
    }

    /// Start a quote computation if none is currently in flight.
    ///
    /// The `inflight_quote_computation` stream always contains a permanent
    /// `pending()` keep-alive future, so `len() == 1` means no real
    /// computation is running. Called by every site that queues a
    /// consumer for the next quote result (p2p quote protocol, controller
    /// RPC) to guarantee there is a future that will eventually wake up
    /// the result-draining select arm.
    fn ensure_quote_computation_is_inflight(&mut self) {
        if self.inflight_quote_computation.len() == 1 {
            self.inflight_quote_computation
                .push(self.make_quote_or_use_cached(
                    self.min_buy,
                    self.max_buy,
                    self.developer_tip.ratio,
                    self.refund_policy.clone().into(),
                ));
        }
    }

    fn fresh_quote(&self) -> Option<BidQuote> {
        let key = QuoteCacheKey {
            min_buy: self.min_buy,
            max_buy: self.max_buy,
        };
        match self.quote_cache.get(&key)? {
            Ok(quote) => Some((*quote).clone()),
            Err(_) => Some(BidQuote::ZERO),
        }
    }

    /// Get a quote from the cache or compute a new one.
    ///
    /// Returns a `'static` future so it can be stored in the event loop
    /// and polled without blocking other select arms.
    fn make_quote_or_use_cached(
        &self,
        min_buy: bitcoin::Amount,
        max_buy: bitcoin::Amount,
        developer_tip: Decimal,
        refund_policy: RefundPolicyWire,
    ) -> BoxFuture<'static, Result<Arc<BidQuote>, Arc<anyhow::Error>>> {
        let quote_cache = self.quote_cache.clone();
        let rate = self.latest_rate.clone();
        let db = self.db.clone();
        let monero_wallet = self.monero_wallet.clone();
        let monero_wallet_for_proof = self.monero_wallet.clone();
        let monero_wallet_for_health = self.monero_wallet.clone();
        let bitcoin_wallet = self.bitcoin_wallet.clone();
        let peer_id = self.peer_id();

        async move {
            // We use the min and max buy amounts to create a unique key for the cache
            // Although these values stay constant over the lifetime of an instance of the asb, this might change in the future
            let key = QuoteCacheKey { min_buy, max_buy };

            // Check if we have a cached quote
            if let Some(cached) = quote_cache.get(&key) {
                tracing::trace!("Got a request for a quote, using cached value.");
                return cached;
            }

            // We have a cache miss, so we compute a new quote
            tracing::trace!("Got a request for a quote, computing new quote.");

            let get_reserved_items = || async {
                let all_swaps = db.all().await?;
                let alice_states: Vec<_> = all_swaps
                    .into_iter()
                    .filter_map(|(_, _, state)| match state {
                        State::Alice(state) => Some(state),
                        _ => None,
                    })
                    .collect();

                Ok(alice_states)
            };

            let get_unlocked_balance = || async {
                unlocked_monero_balance_with_timeout(monero_wallet.main_wallet().await).await
            };

            let get_reserve_proof = || async move {
                reserve_proof_with_timeout(monero_wallet_for_proof.main_wallet().await, peer_id)
                    .await
            };

            // Quote zero unless both the Bitcoin and Monero backends are reachable.
            let health_check = async {
                bitcoin_health_check_with_retry(bitcoin_wallet)
                    .await
                    .context("Bitcoin wallet health check failed")?;
                monero_wallet_for_health
                    .rpc_health_check()
                    .await
                    .context("Monero daemon RPC health check failed")?;
                Ok::<(), anyhow::Error>(())
            };

            let result = match health_check.await {
                Ok(()) => {
                    make_quote(
                        min_buy,
                        max_buy,
                        rate,
                        get_unlocked_balance,
                        get_reserved_items,
                        get_reserve_proof,
                        developer_tip,
                        refund_policy,
                    )
                    .await
                }
                Err(err) => Err(Arc::new(err)),
            };

            // Insert the computed quote into the cache
            // Need to clone it as insert takes ownership
            quote_cache.insert(key, result.clone());

            // If the quote failed, we log the error
            if let Err(err) = &result {
                tracing::warn!(?err, "Failed to make quote. We will retry again later.");
            }

            // Return the computed quote
            result
        }
        .boxed()
    }

    async fn handle_execution_setup_done(
        &mut self,
        bob_peer_id: PeerId,
        swap_id: Uuid,
        state3: State3,
    ) -> Result<()> {
        if self
            .db
            .has_swap(swap_id)
            .await
            .context("Failed to check if UUID is already in use")?
        {
            // TODO: We should ideally check this during swap setup, not after
            return Err(anyhow::anyhow!("UUID is already in use"));
        }

        let handle = self.new_handle(bob_peer_id, swap_id);

        let initial_state = AliceState::Started {
            state3: Box::new(state3),
        };

        let swap = Swap {
            event_loop_handle: handle,
            bitcoin_wallet: self.bitcoin_wallet.clone(),
            monero_wallet: self.monero_wallet.clone(),
            env_config: self.env_config,
            db: self.db.clone(),
            state: initial_state,
            swap_id,
            developer_tip: self.developer_tip.clone(),
            hermes_funding_policy: self.hermes_funding_policy,
        };

        self.db
            .insert_peer_id(swap_id, bob_peer_id)
            .await
            .context("Failed to save peer-id in database")?;
        self.swap_sender
            .send(swap)
            .await
            .context("Failed to send message to spawn swap state machine")?;

        Ok(())
    }

    async fn handle_cooperative_redeem_request(
        &mut self,
        swap_id: Uuid,
        channel: ResponseChannel<cooperative_xmr_redeem_after_punish::Response>,
        peer: PeerId,
    ) -> Result<()> {
        let swap_peer = self.db.get_peer_id(swap_id).await;
        let swap_state = self.db.get_state(swap_id).await;

        // If we do not find the swap in the database, or we do not have a peer-id for it, reject
        let (swap_peer, swap_state) = match (swap_peer, swap_state) {
            (Ok(peer), Ok(state)) => (peer, state),
            _ => {
                tracing::warn!(
                    swap_id = %swap_id,
                    received_from = %peer,
                    reason = "swap not found",
                    "Rejecting cooperative XMR redeem request"
                );
                self.swarm
                    .behaviour_mut()
                    .cooperative_xmr_redeem
                    .send_response(
                        channel,
                        Rejected {
                            swap_id,
                            reason: CooperativeXmrRedeemRejectReason::UnknownSwap,
                        },
                    )
                    .map_err(|_| anyhow!("Couldn't reject cooperative redeem request"))?;

                if let Ok(()) = self.swarm.disconnect_peer_id(peer) {
                    tracing::debug!(%peer, "Disconnected peer for malicious cooperative Monero redeem request")
                }

                bail!("swap not found")
            }
        };

        // If the peer is not the one associated with the swap, reject
        if swap_peer != peer {
            tracing::warn!(
                swap_id = %swap_id,
                received_from = %peer,
                expected_from = %swap_peer,
                reason = "unexpected peer",
                "Rejecting cooperative XMR redeem request"
            );
            self.swarm
                .behaviour_mut()
                .cooperative_xmr_redeem
                .send_response(
                    channel,
                    Rejected {
                        swap_id,
                        reason: CooperativeXmrRedeemRejectReason::MaliciousRequest,
                    },
                )
                .map_err(|_| anyhow!("Failed to reject cooperative XMR redeem request"))?;

            if let Ok(()) = self.swarm.disconnect_peer_id(peer) {
                tracing::debug!(%peer, "Disconnected peer for malicious cooperative Monero redeem request")
            }

            bail!("malicious request (wrong peer)")
        }

        // Bob cannot refund the Bitcoin anymore. We can publish tx_punish to redeem the Bitcoin.
        // Therefore it is safe to reveal s_a to let him redeem the Monero
        let State::Alice(AliceState::BtcPunished {
            state3,
            transfer_proof: Some(transfer_proof),
            ..
        }) = swap_state
        else {
            tracing::warn!(
                swap_id = %swap_id,
                reason = "swap is in invalid state",
                "Rejecting cooperative Monero redeem request"
            );
            self.swarm
                .behaviour_mut()
                .cooperative_xmr_redeem
                .send_response(
                    channel,
                    Rejected {
                        swap_id,
                        reason: CooperativeXmrRedeemRejectReason::SwapInvalidState,
                    },
                )
                .map_err(|_| {
                    anyhow!("Failed to send rejection for cooperative Monero redeem request")
                })?;

            if let Ok(()) = self.swarm.disconnect_peer_id(peer) {
                tracing::debug!(%peer, "Disconnected peer for malicious cooperative Monero redeem request")
            }

            bail!("swap in invalid state")
        };

        // === Background ===
        // On 2026-05-25 an attacker managed to maliciously increase the fees
        // in a way that causes Alice to get significantly less BTC.
        // ==================

        // Interpret a loss of more than 1 / MAX_CANCEL_FEE_PART
        // of the swap amount to be maliciously high.
        const MAX_LOSS_PART: u64 = 4;

        if state3.check_max_loss_under_tolerance(MAX_LOSS_PART)? == false {
            tracing::info!(
                swap_id = %swap_id,
                reason = "malicious swap",
                "Rejecting cooperative Monero redeem request"
            );
            self.swarm
                .behaviour_mut()
                .cooperative_xmr_redeem
                .send_response(
                    channel,
                    Rejected {
                        swap_id,
                        reason: CooperativeXmrRedeemRejectReason::MaliciousRequest,
                    },
                )
                .map_err(|_| {
                    anyhow!("Failed to send rejection for cooperative Monero redeem request")
                })?;
            bail!(
                "Malicious swap detected (swap lost us more than 1/{MAX_LOSS_PART} of swap amount)"
            )
        }

        self.swarm
            .behaviour_mut()
            .cooperative_xmr_redeem
            .send_response(
                channel,
                Fullfilled {
                    swap_id,
                    s_a: state3.s_a,
                    lock_transfer_proof: transfer_proof,
                },
            )
            .map_err(|_| anyhow!("Failed to respond to cooperative XMR redeem request"))?;

        tracing::info!(swap_id = %swap_id, peer = %peer, "Fullfilled cooperative XMR redeem request");

        Ok(())
    }

    /// Create a new [`EventLoopHandle`] that is scoped for communication with
    /// the given peer.
    fn new_handle(&mut self, peer: PeerId, swap_id: Uuid) -> EventLoopHandle {
        // Create a new channel for receiving encrypted signatures from Bob
        // The channel has a capacity of 1 since we only expect one signature per swap
        let (encrypted_signature_sender, encrypted_signature_receiver) = bmrng::channel(1);

        // The sender is stored in the EventLoop
        // The receiver is stored in the EventLoopHandle
        // When a signature is received, the EventLoop uses the sender to notify the EventLoopHandle
        self.recv_encrypted_signature
            .insert(swap_id, encrypted_signature_sender);

        // Create a watch channel for burn-on-refund instructions
        // Uses watch instead of bmrng to allow multiple updates before consumption
        let (burn_instruction_sender, burn_instruction_receiver) =
            tokio::sync::watch::channel(None);
        self.recv_burn_on_refund_instruction
            .insert(swap_id, burn_instruction_sender);

        let transfer_proof_sender = self.outgoing_transfer_proofs_sender.clone();

        EventLoopHandle {
            swap_id,
            peer,
            recv_encrypted_signature: tokio::sync::Mutex::new(Some(encrypted_signature_receiver)),
            recv_burn_on_refund_instruction: tokio::sync::Mutex::new(burn_instruction_receiver),
            transfer_proof_sender: tokio::sync::Mutex::new(Some(transfer_proof_sender)),
        }
    }

    /// Handle a request to grant mercy for a swap.
    ///
    /// This checks that the swap is not currently running, transitions the
    /// state to BtcMercyGranted, and resumes the swap.
    async fn handle_grant_mercy(&mut self, swap_id: Uuid) -> Result<()> {
        use crate::asb::grant_mercy;

        // Make sure swap isn't already running.
        if self.is_swap_running(swap_id) {
            return Err(anyhow!(
                "Cannot grant mercy while swap {} is still running",
                swap_id
            ));
        }

        // Use the grant_mercy function to transition the state
        let new_state = grant_mercy(swap_id, self.db.clone()).await?;

        // Get peer ID for this swap
        let peer_id = self.db.get_peer_id(swap_id).await?;

        // Create handle and swap to resume
        let handle = self.new_handle(peer_id, swap_id);
        let swap = Swap {
            event_loop_handle: handle,
            bitcoin_wallet: self.bitcoin_wallet.clone(),
            monero_wallet: self.monero_wallet.clone(),
            env_config: self.env_config,
            db: self.db.clone(),
            state: new_state,
            swap_id,
            developer_tip: self.developer_tip.clone(),
            hermes_funding_policy: self.hermes_funding_policy,
        };

        // Send swap to be resumed
        self.swap_sender
            .send(swap)
            .await
            .context("Failed to send swap to be resumed")?;

        tracing::info!(%swap_id, "Granted mercy and resumed swap");

        Ok(())
    }

    /// Change `maker.external_bitcoin_redeem_address` both in-memory and
    /// on disk. Applies only to swaps started _afterwards_.
    ///
    /// Uses `toml_edit` so the on-disk edit is minimal: comments,
    /// key order and formatting of every other field are preserved.
    // TODO: lock file for the whole thing
    async fn handle_set_external_bitcoin_redeem_address(
        &mut self,
        address: Option<bitcoin::Address>,
    ) -> Result<()> {
        let current = tokio::fs::read_to_string(&self.config_path)
            .await
            .context("Failed to read config.toml")?;
        let mut doc: toml_edit::DocumentMut =
            current.parse().context("Failed to parse config.toml")?;

        let maker = doc["maker"]
            .as_table_mut()
            .context("config.toml is missing the [maker] table")?;
        match &address {
            Some(address) => {
                maker["external_bitcoin_redeem_address"] = toml_edit::value(address.to_string());
            }
            None => {
                maker.remove("external_bitcoin_redeem_address");
            }
        }

        tokio::fs::write(&self.config_path, doc.to_string())
            .await
            .context("Failed to write config.toml")?;

        let reloaded = swap_env::config::Config::read(&self.config_path)
            .context("Failed to re-read config.toml after edit")?;

        // Sanity check the address we loaded from the file
        if &reloaded.maker.external_bitcoin_redeem_address != &address {
            bail!(
                "Reloaded config has different address than the one we want to set! Found: {}. Expected: {}",
                reloaded
                    .maker
                    .external_bitcoin_redeem_address
                    .as_ref()
                    .map(bitcoin::Address::to_string)
                    .unwrap_or("None".into()),
                address
                    .as_ref()
                    .map(bitcoin::Address::to_string)
                    .unwrap_or("None".into()),
            );
        }

        self.external_redeem_address = reloaded.maker.external_bitcoin_redeem_address;

        tracing::info!(
            address = ?self.external_redeem_address.as_ref().map(|a| a.to_string()),
            "Updated external_bitcoin_redeem_address",
        );

        Ok(())
    }

    /// Check whether we are currently executing a specific swap.
    fn is_swap_running(&self, swap_id: Uuid) -> bool {
        // Check whether the channels between event loop and event loop handle
        // are still intact.
        // Yes -> swap is running
        // No -> swap is not running (channels were dropped with event loop handle)

        // We are eager to assume a swap is running. It can do more harm to run two instances than to not run a swap.
        // We assume the swap is running if either of the channels is still open.
        if let Some(channel) = self.recv_encrypted_signature.get(&swap_id)
            && !channel.is_closed()
        {
            return true;
        }

        if let Some(channel) = self.recv_burn_on_refund_instruction.get(&swap_id)
            && !channel.is_closed()
        {
            return true;
        }

        return false;
    }
}

// We use a Mutex here to allow recv_encrypted_signature and transfer_proof_sender to be accessed concurrently
#[derive(Debug)]
pub struct EventLoopHandle {
    swap_id: Uuid,
    peer: PeerId,
    recv_encrypted_signature:
        tokio::sync::Mutex<Option<bmrng::RequestReceiver<bitcoin::EncryptedSignature, ()>>>,
    recv_burn_on_refund_instruction: tokio::sync::Mutex<tokio::sync::watch::Receiver<Option<bool>>>,
    #[allow(clippy::type_complexity)]
    transfer_proof_sender: tokio::sync::Mutex<
        Option<
            tokio::sync::mpsc::UnboundedSender<(
                PeerId,
                transfer_proof::Request,
                oneshot::Sender<Result<(), OutboundFailure>>,
            )>,
        >,
    >,
}

impl EventLoopHandle {
    fn build_transfer_proof_request(
        &self,
        transfer_proof: monero::TransferProof,
    ) -> transfer_proof::Request {
        transfer_proof::Request {
            swap_id: self.swap_id,
            tx_lock_proof: transfer_proof,
        }
    }

    /// Wait for an encrypted signature from Bob
    ///
    /// This function can not be called concurrently (even though it doesn't take &self mut)
    /// It internally acquires a Mutex. If another instance of this is already running, it will fail.
    pub async fn recv_encrypted_signature(&self) -> Result<bitcoin::EncryptedSignature> {
        let mut recv_encrypted_signature_guard = self
            .recv_encrypted_signature
            .try_lock()
            .map_err(|_| anyhow!("recv_encrypted_signature is already being called"))?;

        let receiver = recv_encrypted_signature_guard
            .as_mut()
            .context("Encrypted signature was already received")?;

        let (tx_redeem_encsig, responder) = receiver.recv().await?;

        // Acknowledge receipt of the encrypted signature
        // This notifies the EventLoop that the signature has been processed
        // The EventLoop can then send an acknowledgement back to Bob over the network
        responder
            .respond(())
            .context("Failed to acknowledge receipt of encrypted signature")?;

        // Only take after successful receipt and acknowledgement
        recv_encrypted_signature_guard.take();

        Ok(tx_redeem_encsig)
    }

    /// Send a transfer proof to Bob
    ///
    /// This function will retry indefinitely until the transfer proof is sent successfully
    /// and acknowledged by Bob
    ///
    /// This will fail if
    /// 1. the transfer proof has already been sent once
    /// 2. there is an error with the bmrng channel
    ///
    /// This function can not be called concurrently (even though it doesn't take &self mut)
    /// It internally acquires a Mutex. If another instance of this is already running, it will fail.
    pub async fn send_transfer_proof(&self, msg: monero::TransferProof) -> Result<()> {
        let mut transfer_proof_sender_guard = self
            .transfer_proof_sender
            .try_lock()
            .map_err(|_| anyhow!("send_transfer_proof is already being called"))?;

        let sender = transfer_proof_sender_guard
            .as_ref()
            .context("Transfer proof was already sent")?;

        // We will retry indefinitely until we succeed
        let backoff = backoff::ExponentialBackoffBuilder::new()
            .with_max_elapsed_time(None)
            .with_max_interval(Duration::from_secs(60))
            .build();

        let transfer_proof = self.build_transfer_proof_request(msg);

        backoff::future::retry_notify(
            backoff,
            || async {
                // Create a oneshot channel to receive the acknowledgment of the transfer proof
                let (singular_sender, singular_receiver) = oneshot::channel();

                if let Err(err) = sender.send((self.peer, transfer_proof.clone(), singular_sender))
                {
                    return Err(backoff::Error::permanent(anyhow!(err).context(
                        "Failed to communicate transfer proof through event loop channel",
                    )));
                }

                match singular_receiver.await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(err)) => Err(backoff::Error::transient(
                        anyhow!(err)
                            .context("A network error occurred while sending the transfer proof"),
                    )),
                    Err(_) => Err(backoff::Error::permanent(anyhow!(
                        "The sender channel should never be closed without sending a response"
                    ))),
                }
            },
            |e, wait_time: Duration| {
                tracing::warn!(
                    swap_id = %self.swap_id,
                    error = ?e,
                    "Failed to send transfer proof. We will retry in {} seconds",
                    wait_time.as_secs()
                )
            },
        )
        .await?;

        transfer_proof_sender_guard.take();

        Ok(())
    }

    /// Wait for a NEW burn-on-refund instruction from the operator
    ///
    /// This method waits until the operator sends a new decision via the EventLoopService.
    /// Use this in select! arms to react to operator commands.
    ///
    /// Returns the new burn decision when one is received.
    pub async fn wait_for_burn_on_refund_instruction(&self) -> Result<bool> {
        let mut guard = self.recv_burn_on_refund_instruction.lock().await;

        guard
            .changed()
            .await
            .map_err(|_| anyhow!("Burn instruction sender was dropped"))?;

        let value = *guard.borrow();
        Ok(value.expect("changed() returned Ok, so value should be set"))
    }

    /// Get the current burn-on-refund instruction value
    ///
    /// Returns Some(bool) if an instruction has been set, None otherwise.
    /// Use this to check the current decision before taking action.
    pub async fn get_burn_on_refund_instruction(&self) -> Option<bool> {
        let guard = self.recv_burn_on_refund_instruction.lock().await;
        let value = *guard.borrow();
        value
    }
}

/// For a new swap of `swap_amount`, this function calculates how much
/// Bitcoin should go into the anti spam deposit incase of a refund.
/// Returns ZERO when anti_spam_deposit_ratio is 0, indicating immediate and full refund.
/// Also returns whether or not to always withhold the the anti spam deposit output if the taker refunds.
fn apply_anti_spam_policy(
    swap_amount: bitcoin::Amount,
    refund_policy: &RefundPolicy,
) -> Result<(bitcoin::Amount, bool)> {
    let should_always_withhold = refund_policy.always_withhold_deposit;

    // When ratio is 0.0, no amnesty - use full refund path for fewer fees
    if refund_policy.anti_spam_deposit_ratio == Decimal::ZERO {
        return Ok((bitcoin::Amount::ZERO, should_always_withhold));
    }

    let btc_anti_spam_deposit_ratio = refund_policy.anti_spam_deposit_ratio;

    let amount_sats = swap_amount.to_sat();
    let amount_decimal =
        Decimal::from_u64(amount_sats).context("Decimal overflowed by Bitcoin sats")?;

    let btc_amnesty_decimal = amount_decimal
        .checked_mul(btc_anti_spam_deposit_ratio)
        .context("Decimal overflow when computing amnesty amount in sats")?
        .floor();
    let btc_amnesty_sats: u64 = btc_amnesty_decimal
        .try_into()
        .context("Couldn't convert Decimal to u64")?;

    let btc_amnesty_amount = bitcoin::Amount::from_sat(btc_amnesty_sats);

    let minimum_to_cover_fees = bitcoin::Amount::from_sat(
        bitcoin_wallet::MIN_ABSOLUTE_TX_FEE_SATS * swap_machine::common::NUM_WITHHOLD_PATH_TXS + 1,
    );

    Ok((
        btc_amnesty_amount.max(minimum_to_cover_fees),
        should_always_withhold,
    ))
}

/// Multiply a fee amount by `multiplier`, rounding to the nearest satoshi.
fn scale_fee(fee: bitcoin::Amount, multiplier: Decimal) -> Result<bitcoin::Amount> {
    let sats: u64 = Decimal::from(fee.to_sat())
        .checked_mul(multiplier)
        .context("Decimal overflow when scaling fee")?
        .round()
        .try_into()
        .context("Scaled fee does not fit in u64")?;
    Ok(bitcoin::Amount::from_sat(sats))
}

async fn capture_wallet_snapshot(
    bitcoin_wallet: Arc<dyn BitcoinWallet>,
    monero_wallet: &monero::Wallets,
    external_redeem_address: &Option<bitcoin::Address>,
    btc_redeem_fee_multiplier: Decimal,
    transfer_amount: bitcoin::Amount,
) -> Result<WalletSnapshot> {
    let start_time = Instant::now();

    // Don't back a swap setup against an unreachable Bitcoin or Monero backend.
    bitcoin_wallet
        .health_check()
        .await
        .context("Bitcoin wallet health check failed while capturing wallet snapshot")?;
    monero_wallet
        .rpc_health_check()
        .await
        .context("Monero daemon RPC health check failed while capturing wallet snapshot")?;

    let unlocked_balance = monero_wallet.main_wallet().await.unlocked_balance().await?;
    let total_balance = monero_wallet.main_wallet().await.total_balance().await?;

    tracing::info!(%unlocked_balance, %total_balance, "Capturing monero wallet snapshot");

    let redeem_address = external_redeem_address
        .clone()
        .unwrap_or(bitcoin_wallet.new_address().await?);
    let punish_address = external_redeem_address
        .clone()
        .unwrap_or(bitcoin_wallet.new_address().await?);

    let tx_lock_fee = bitcoin_wallet
        .estimate_fee(bitcoin::TxLock::weight(), Some(transfer_amount))
        .await?;
    let redeem_fee = bitcoin_wallet
        .estimate_fee(bitcoin::TxRedeem::weight(), Some(transfer_amount))
        .await?;
    let redeem_fee = scale_fee(redeem_fee, btc_redeem_fee_multiplier)
        .context("Failed to apply btc_redeem_fee_multiplier")?;
    let cancel_fee = bitcoin_wallet
        .estimate_fee(bitcoin::TxCancel::weight(), Some(transfer_amount))
        .await?;
    let refund_fee = bitcoin_wallet
        .estimate_fee(bitcoin::TxFullRefund::weight(), Some(transfer_amount))
        .await?;
    let partial_refund_fee = bitcoin_wallet
        .estimate_fee(bitcoin::TxPartialRefund::weight(), Some(transfer_amount))
        .await?;
    let reclaim_fee = bitcoin_wallet
        .estimate_fee(bitcoin::TxReclaim::weight(), Some(transfer_amount))
        .await?;
    let mercy_fee = bitcoin_wallet
        .estimate_fee(bitcoin::TxMercy::weight(), Some(transfer_amount))
        .await?;
    let punish_fee = bitcoin_wallet
        .estimate_fee(bitcoin::TxPunish::weight(), Some(transfer_amount))
        .await?;
    let withhold_fee = bitcoin_wallet
        .estimate_fee(bitcoin::TxWithhold::weight(), Some(transfer_amount))
        .await?;

    let end_time = Instant::now();

    tracing::debug!(duration_ms=%end_time.duration_since(start_time).as_millis(), "Finished capturing wallet snapshot");

    Ok(WalletSnapshot::new(
        unlocked_balance.into(),
        redeem_address,
        punish_address,
        tx_lock_fee,
        redeem_fee,
        cancel_fee,
        refund_fee,
        partial_refund_fee,
        reclaim_fee,
        mercy_fee,
        punish_fee,
        withhold_fee,
    ))
}

mod service {
    use super::*;

    /// Status snapshot of the primary onion service.
    #[derive(Debug)]
    pub struct OnionServiceStatusInfo {
        pub state: String,
        pub reachable: bool,
        pub problem: Option<String>,
    }

    /// Request types for the EventLoop service with typed responders
    #[derive(Debug)]
    pub enum EventLoopRequest {
        GetMultiaddresses {
            respond_to: oneshot::Sender<(PeerId, Vec<libp2p::Multiaddr>)>,
        },
        GetActiveConnections {
            respond_to: oneshot::Sender<usize>,
        },
        GetRegistrationStatus {
            respond_to: oneshot::Sender<
                Vec<swap_p2p::protocols::rendezvous::register::public::RendezvousNodeStatus>,
            >,
        },
        SetBurnOnRefund {
            swap_id: Uuid,
            burn: bool,
            respond_to: oneshot::Sender<Result<(), anyhow::Error>>,
        },
        GrantMercy {
            swap_id: Uuid,
            respond_to: oneshot::Sender<Result<(), anyhow::Error>>,
        },
        GetWormholeServices {
            respond_to: oneshot::Sender<Vec<crate::network::wormhole::alice::WormholeServiceInfo>>,
        },
        GetOnionServiceStatus {
            respond_to: oneshot::Sender<Option<OnionServiceStatusInfo>>,
        },
        GetCurrentQuote {
            respond_to: oneshot::Sender<Result<Arc<BidQuote>, Arc<anyhow::Error>>>,
        },
        SetExternalBitcoinRedeemAddress {
            address: Option<bitcoin::Address>,
            respond_to: oneshot::Sender<Result<(), anyhow::Error>>,
        },
        GetExternalBitcoinRedeemAddress {
            respond_to: oneshot::Sender<Option<bitcoin::Address>>,
        },
    }

    /// Tower service for communicating with the EventLoop
    #[derive(Debug, Clone)]
    pub struct EventLoopService {
        sender: mpsc::UnboundedSender<EventLoopRequest>,
    }

    impl EventLoopService {
        pub fn new(sender: mpsc::UnboundedSender<EventLoopRequest>) -> Self {
            Self { sender }
        }

        /// Get multiaddresses and peer ID from the event loop
        pub async fn get_multiaddresses(&self) -> anyhow::Result<(PeerId, Vec<libp2p::Multiaddr>)> {
            let (tx, rx) = oneshot::channel();
            self.sender
                .send(EventLoopRequest::GetMultiaddresses { respond_to: tx })
                .map_err(|_| anyhow::anyhow!("EventLoop service is down"))?;
            rx.await
                .map_err(|_| anyhow::anyhow!("EventLoop service did not respond"))
        }

        /// Get the number of active connections from the event loop
        pub async fn get_active_connections(&self) -> anyhow::Result<usize> {
            let (tx, rx) = oneshot::channel();
            self.sender
                .send(EventLoopRequest::GetActiveConnections { respond_to: tx })
                .map_err(|_| anyhow::anyhow!("EventLoop service is down"))?;
            rx.await
                .map_err(|_| anyhow::anyhow!("EventLoop service did not respond"))
        }

        /// Get the registration status at configured rendezvous points
        pub async fn get_registration_status(
            &self,
        ) -> anyhow::Result<Vec<crate::network::rendezvous::register::public::RendezvousNodeStatus>>
        {
            let (tx, rx) = oneshot::channel();
            self.sender
                .send(EventLoopRequest::GetRegistrationStatus { respond_to: tx })
                .map_err(|_| anyhow::anyhow!("EventLoop service is down"))?;
            rx.await
                .map_err(|_| anyhow::anyhow!("EventLoop service did not respond"))
        }

        /// Set the burn-on-refund decision for a specific swap
        ///
        /// This can be called multiple times to update the decision before
        /// the swap state machine polls for it.
        pub async fn set_withhold_deposit(&self, swap_id: Uuid, burn: bool) -> anyhow::Result<()> {
            let (tx, rx) = oneshot::channel();
            self.sender
                .send(EventLoopRequest::SetBurnOnRefund {
                    swap_id,
                    burn,
                    respond_to: tx,
                })
                .map_err(|_| anyhow::anyhow!("EventLoop service is down"))?;
            rx.await
                .map_err(|_| anyhow::anyhow!("EventLoop service did not respond"))?
        }

        /// Get the list of active wormhole services
        pub async fn get_wormhole_services(
            &self,
        ) -> anyhow::Result<Vec<crate::network::wormhole::alice::WormholeServiceInfo>> {
            let (tx, rx) = oneshot::channel();
            self.sender
                .send(EventLoopRequest::GetWormholeServices { respond_to: tx })
                .map_err(|_| anyhow::anyhow!("EventLoop service is down"))?;
            rx.await
                .map_err(|_| anyhow::anyhow!("EventLoop service did not respond"))
        }

        /// Get the status of the primary onion service
        pub async fn get_onion_service_status(
            &self,
        ) -> anyhow::Result<Option<OnionServiceStatusInfo>> {
            let (tx, rx) = oneshot::channel();
            self.sender
                .send(EventLoopRequest::GetOnionServiceStatus { respond_to: tx })
                .map_err(|_| anyhow::anyhow!("EventLoop service is down"))?;
            rx.await
                .map_err(|_| anyhow::anyhow!("EventLoop service did not respond"))
        }

        /// Get the quote the ASB is currently serving to peers.
        ///
        /// Reuses the same cache and in-flight computation as the p2p
        /// quote protocol, so repeated calls during a single computation
        /// share the result.
        pub async fn get_current_quote(&self) -> anyhow::Result<Arc<BidQuote>> {
            let (tx, rx) = oneshot::channel();
            self.sender
                .send(EventLoopRequest::GetCurrentQuote { respond_to: tx })
                .map_err(|_| anyhow::anyhow!("EventLoop service is down"))?;
            rx.await
                .map_err(|_| anyhow::anyhow!("EventLoop service did not respond"))?
                .map_err(|e| anyhow::anyhow!("Failed to compute quote: {}", e))
        }

        /// Grant mercy for a swap in BtcWithholdConfirmed state
        ///
        /// This transitions the swap to BtcMercyGranted and resumes
        /// the swap state machine to publish the mercy transaction.
        pub async fn grant_mercy(&self, swap_id: Uuid) -> anyhow::Result<()> {
            let (tx, rx) = oneshot::channel();
            self.sender
                .send(EventLoopRequest::GrantMercy {
                    swap_id,
                    respond_to: tx,
                })
                .map_err(|_| anyhow::anyhow!("EventLoop service is down"))?;
            rx.await
                .map_err(|_| anyhow::anyhow!("EventLoop service did not respond"))?
        }

        pub async fn set_external_bitcoin_redeem_address(
            &self,
            address: bitcoin::Address,
        ) -> anyhow::Result<()> {
            let (tx, rx) = oneshot::channel();
            self.sender
                .send(EventLoopRequest::SetExternalBitcoinRedeemAddress {
                    address: Some(address),
                    respond_to: tx,
                })
                .map_err(|_| anyhow::anyhow!("EventLoop service is down"))?;
            rx.await
                .map_err(|_| anyhow::anyhow!("EventLoop service did not respond"))?
        }

        pub async fn get_external_bitcoin_redeem_address(
            &self,
        ) -> anyhow::Result<Option<bitcoin::Address>> {
            let (tx, rx) = oneshot::channel();
            self.sender
                .send(EventLoopRequest::GetExternalBitcoinRedeemAddress { respond_to: tx })
                .map_err(|_| anyhow::anyhow!("EventLoop service is down"))?;
            rx.await
                .map_err(|_| anyhow::anyhow!("EventLoop service did not respond"))
        }

        pub async fn clear_external_bitcoin_redeem_address(&self) -> anyhow::Result<()> {
            let (tx, rx) = oneshot::channel();
            self.sender
                .send(EventLoopRequest::SetExternalBitcoinRedeemAddress {
                    address: None,
                    respond_to: tx,
                })
                .map_err(|_| anyhow::anyhow!("EventLoop service is down"))?;
            rx.await
                .map_err(|_| anyhow::anyhow!("EventLoop service did not respond"))?
        }
    }
}

mod quote {
    use crate::monero::{Amount, AmountExt};
    use anyhow::{Context, anyhow};
    use bitcoin_wallet::BitcoinWallet;
    use rust_decimal::Decimal;
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    use swap_feed::LatestRate;
    use tokio::time::timeout;

    use crate::{
        network::quote::{BidQuote, RefundPolicyWire, ReserveProofWithAddress},
        protocol::alice::ReservesMonero,
    };

    /// The time-to-live for quotes in the cache
    pub const QUOTE_CACHE_TTL: Duration = Duration::from_secs(120);

    /// The key for the quote cache
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct QuoteCacheKey {
        pub min_buy: bitcoin::Amount,
        pub max_buy: bitcoin::Amount,
    }

    /// Computes a quote given the provided dependencies
    #[allow(clippy::too_many_arguments)]
    pub async fn make_quote<LR, F, Fut, I, Fut2, T, P, Fut3>(
        min_buy: bitcoin::Amount,
        max_buy: bitcoin::Amount,
        mut latest_rate: LR,
        get_unlocked_balance: F,
        get_reserved_items: I,
        get_reserve_proof: P,
        developer_tip: Decimal,
        refund_policy: RefundPolicyWire,
    ) -> Result<Arc<BidQuote>, Arc<anyhow::Error>>
    where
        LR: LatestRate,
        F: FnOnce() -> Fut,
        Fut: futures::Future<Output = Result<Amount, anyhow::Error>>,
        I: FnOnce() -> Fut2,
        Fut2: futures::Future<Output = Result<Vec<T>, anyhow::Error>>,
        T: ReservesMonero,
        P: FnOnce() -> Fut3,
        Fut3: futures::Future<Output = Result<ReserveProofWithAddress, anyhow::Error>>,
    {
        let start_time = Instant::now();

        let ask_price = latest_rate
            .latest_rate()
            .map_err(|e| Arc::new(anyhow!(e).context("Failed to get latest rate")))?
            .ask()
            .map_err(|e| Arc::new(e.context("Failed to compute asking price")))?;

        // Get reserve proof, if it fails, we simply omit the proof from the quote
        let reserve_proof = match get_reserve_proof().await {
            Ok(proof) => Some(proof),
            Err(err) => {
                tracing::warn!(?err, "Failed to generate reserve proof for quote");
                None
            }
        };

        // Get the unlocked balance
        let unlocked_balance = get_unlocked_balance()
            .await
            .context("Failed to get unlocked Monero balance")
            .map_err(Arc::new)?;

        // Get the reserved amounts
        let reserved_amounts: Vec<_> = get_reserved_items()
            .await
            .context("Failed to get reserved items")
            .map_err(Arc::new)?
            .into_iter()
            .map(|item| item.reserved_monero())
            .collect();

        let unreserved_xmr_balance = unreserved_monero_balance(
            unlocked_balance,
            reserved_amounts.into_iter(),
            developer_tip,
        );

        let max_bitcoin_for_monero = unreserved_xmr_balance
            .max_bitcoin_for_price(ask_price)
            .ok_or_else(|| {
                Arc::new(anyhow!(
                    "Bitcoin price ({}) x Monero ({}) overflow",
                    ask_price,
                    unreserved_xmr_balance
                ))
            })?;

        let end_time = Instant::now();
        tracing::info!(%ask_price, %unreserved_xmr_balance, %max_bitcoin_for_monero, duration_ms=%end_time.duration_since(start_time).as_millis(), "Computed quote");

        if min_buy > max_bitcoin_for_monero {
            tracing::trace!(
                "Your Monero balance is too low to initiate a swap, as your minimum swap amount is {}. You could at most swap {}",
                min_buy,
                max_bitcoin_for_monero
            );

            return Ok(Arc::new(BidQuote {
                price: ask_price,
                min_quantity: bitcoin::Amount::ZERO,
                max_quantity: bitcoin::Amount::ZERO,
                refund_policy: refund_policy.clone(),
                reserve_proof,
            }));
        }

        if max_buy > max_bitcoin_for_monero {
            tracing::trace!(
                "Your Monero balance is too low to initiate a swap with the maximum swap amount {} that you have specified in your config. You can at most swap {}",
                max_buy,
                max_bitcoin_for_monero
            );

            return Ok(Arc::new(BidQuote {
                price: ask_price,
                min_quantity: min_buy,
                max_quantity: max_bitcoin_for_monero,
                refund_policy: refund_policy.clone(),
                reserve_proof,
            }));
        }

        Ok(Arc::new(BidQuote {
            price: ask_price,
            min_quantity: min_buy,
            max_quantity: max_buy,
            refund_policy,
            reserve_proof,
        }))
    }

    /// Calculates the unreserved Monero balance by subtracting reserved amounts from unlocked balance
    pub fn unreserved_monero_balance(
        unlocked_balance: Amount,
        reserved_amounts: impl Iterator<Item = Amount>,
        developer_tip: Decimal,
    ) -> Amount {
        use rust_decimal::prelude::ToPrimitive;

        let unlocked_balance_piconero = Decimal::from(unlocked_balance.as_pico());

        // If a developer tip is configured, we need to account for the fact that
        // to lock X XMR for a swap, we actually need X * (1 + tip_percentage) XMR
        // because the tip is sent as an additional output in the same transaction

        // To find how much we can actually use for swaps, we need to solve:
        //     swap_amount * multiplier <= available_after_reserved
        // <=> swap_amount <= available_after_reserved / multiplier

        // Calculate the effective multiplier: 1 + tip_percentage
        let multiplier = Decimal::ONE + developer_tip;

        // The amount of Monero we can send somewhere if for every transaction we send,
        // we send a tip within the same transaction as an additional output
        //
        // This does not take the fee into account.
        //
        // When we call `max_bitcoin_for_price`, it uses the `CONSERVATIVE_MONERO_FEE` constantdefined in `swap/src/monero.rs`
        // to take into account the fee.
        let unlocked_balance_piconero_after_accounting_for_tip =
            unlocked_balance_piconero / multiplier;

        // Get the sum of all the individual reserved amounts
        // This is the amount of Monero that is required for ongoing swaps
        //
        // Swaps where the Monero hasn't been locked yet but we know we will lock it soon
        //
        // Note: It is important that we subtract this AFTER accounting for the tip
        // as these other swaps will also require a tip output
        let total_reserved_piconero = Decimal::from(
            reserved_amounts
                .fold(Amount::ZERO, |acc, amount| acc + amount)
                .as_pico(),
        );

        // Unlocked balance after accounting for the tip and the reserved amounts
        let unreserved_unlocked_piconero_after_accounting_for_tip =
            unlocked_balance_piconero_after_accounting_for_tip
                .checked_sub(total_reserved_piconero)
                .unwrap_or(Decimal::ZERO)
                .to_u64()
                .unwrap_or(0);

        Amount::from_pico(unreserved_unlocked_piconero_after_accounting_for_tip)
    }

    /// This is how long we maximally wait for the wallet operation
    const MONERO_WALLET_OPERATION_TIMEOUT: Duration = Duration::from_secs(10);

    /// How long we keep retrying the Bitcoin wallet health check before failing the quote.
    const BITCOIN_WALLET_HEALTH_CHECK_MAX_ELAPSED: Duration = Duration::from_secs(60);

    /// Checks that the Bitcoin wallet can reach its Electrum backend, retrying on failure.
    pub async fn bitcoin_health_check_with_retry(
        wallet: Arc<dyn BitcoinWallet>,
    ) -> Result<(), anyhow::Error> {
        let backoff = backoff::ExponentialBackoffBuilder::new()
            .with_max_elapsed_time(Some(BITCOIN_WALLET_HEALTH_CHECK_MAX_ELAPSED))
            .with_max_interval(Duration::from_secs(15))
            .build();

        backoff::future::retry_notify(
            backoff,
            || async {
                wallet
                    .health_check()
                    .await
                    .map_err(backoff::Error::transient)
            },
            |e, wait_time: Duration| {
                tracing::warn!(
                    error = ?e,
                    "Bitcoin wallet health check failed. We will retry in {} seconds",
                    wait_time.as_secs()
                )
            },
        )
        .await
    }

    /// Returns the unlocked Monero balance from the wallet
    pub async fn unlocked_monero_balance_with_timeout(
        wallet: Arc<crate::monero::Wallet>,
    ) -> Result<Amount, anyhow::Error> {
        // First check if the wallet is synchronized
        // We cannot safely provide a balance if the wallet is not synchronized
        // We cannot be sure that the balance is accurate
        // It is dangerous to provide a balancer higher than the actual balance
        if !timeout(MONERO_WALLET_OPERATION_TIMEOUT, wallet.synchronized())
            .await?
            .context("Timeout while checking if wallet is synchronized")?
        {
            return Err(anyhow::anyhow!("Wallet is not synchronized"));
        }

        let balance = timeout(MONERO_WALLET_OPERATION_TIMEOUT, wallet.unlocked_balance())
            .await?
            .context("Timeout while getting unlocked balance from Monero wallet")?;

        Ok(balance.into())
    }

    /// Returns a reserve proof from the wallet with a timeout
    pub async fn reserve_proof_with_timeout(
        wallet: Arc<crate::monero::Wallet>,
        peer_id: libp2p::PeerId,
    ) -> Result<ReserveProofWithAddress, anyhow::Error> {
        let message = peer_id.to_string();

        let address = timeout(MONERO_WALLET_OPERATION_TIMEOUT, wallet.main_address())
            .await?
            .context("Timeout while getting main address from Monero wallet for reserve proof")?;

        let proof = timeout(
            MONERO_WALLET_OPERATION_TIMEOUT,
            wallet.get_reserve_proof(0, None, &message),
        )
        .await?
        .context("Timeout while generating reserve proof")?;

        Ok(ReserveProofWithAddress {
            address,
            proof,
            message,
        })
    }
}

#[allow(missing_debug_implementations)]
struct MpscChannels<T> {
    sender: mpsc::Sender<T>,
    receiver: mpsc::Receiver<T>,
}

impl<T> Default for MpscChannels<T> {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel(100);
        MpscChannels { sender, receiver }
    }
}

#[cfg(test)]
mod tests {
    use swap_feed::FixedRate;

    use crate::{
        asb::event_loop::quote::{make_quote, unreserved_monero_balance},
        protocol::alice::ReservesMonero,
    };

    use super::*;

    use crate::monero::{Amount, AmountExt};

    #[tokio::test]
    async fn test_unreserved_monero_balance_with_no_reserved_amounts() {
        let balance = Amount::parse_monero("10.0").unwrap();
        let reserved_amounts = vec![];

        let result =
            unreserved_monero_balance(balance, reserved_amounts.into_iter(), Decimal::ZERO);

        assert_eq!(result, balance);
    }

    #[tokio::test]
    async fn test_unreserved_monero_balance_with_reserved_amounts() {
        let balance = monero::Amount::parse_monero("10.0").unwrap();
        let reserved_amounts = vec![
            monero::Amount::parse_monero("2.0").unwrap(),
            monero::Amount::parse_monero("3.0").unwrap(),
        ];

        let result =
            unreserved_monero_balance(balance, reserved_amounts.into_iter(), Decimal::ZERO);

        let expected = monero::Amount::parse_monero("5.0").unwrap();
        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_unreserved_monero_balance_insufficient_balance() {
        let balance = monero::Amount::parse_monero("5.0").unwrap();
        let reserved_amounts = vec![
            monero::Amount::parse_monero("3.0").unwrap(),
            monero::Amount::parse_monero("4.0").unwrap(), // Total reserved > balance
        ];

        let result =
            unreserved_monero_balance(balance, reserved_amounts.into_iter(), Decimal::ZERO);

        // Should return zero when reserved > balance
        assert_eq!(result, monero::Amount::ZERO);
    }

    #[tokio::test]
    async fn test_unreserved_monero_balance_exact_match() {
        let balance = monero::Amount::parse_monero("10.0").unwrap();
        let reserved_amounts = vec![
            monero::Amount::parse_monero("4.0").unwrap(),
            monero::Amount::parse_monero("6.0").unwrap(), // Exactly equals balance
        ];

        let result =
            unreserved_monero_balance(balance, reserved_amounts.into_iter(), Decimal::ZERO);

        assert_eq!(result, monero::Amount::ZERO);
    }

    #[tokio::test]
    async fn test_unreserved_monero_balance_zero_balance() {
        let balance = monero::Amount::ZERO;
        let reserved_amounts = vec![monero::Amount::parse_monero("1.0").unwrap()];

        let result =
            unreserved_monero_balance(balance, reserved_amounts.into_iter(), Decimal::ZERO);

        assert_eq!(result, monero::Amount::ZERO);
    }

    #[tokio::test]
    async fn test_unreserved_monero_balance_empty_reserved_amounts() {
        let balance = monero::Amount::parse_monero("5.0").unwrap();
        let reserved_amounts: Vec<MockReservedItem> = vec![];

        let result = unreserved_monero_balance(
            balance,
            reserved_amounts.into_iter().map(|item| item.reserved),
            Decimal::ZERO,
        );

        assert_eq!(result, balance);
    }

    #[tokio::test]
    async fn test_unreserved_monero_balance_large_amounts() {
        let balance = monero::Amount::from_pico(1_000_000_000);
        let reserved_amounts = vec![monero::Amount::from_pico(300_000_000)];

        let result =
            unreserved_monero_balance(balance, reserved_amounts.into_iter(), Decimal::ZERO);

        let expected = monero::Amount::from_pico(700_000_000);
        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_make_quote_successful_within_limits() {
        let min_buy = bitcoin::Amount::from_sat(100_000);
        let max_buy = bitcoin::Amount::from_sat(500_000);
        let rate = FixedRate::default();
        let balance = monero::Amount::parse_monero("1.0").unwrap();
        let reserved_items: Vec<MockReservedItem> = vec![];

        let result = make_quote(
            min_buy,
            max_buy,
            rate.clone(),
            || async { Ok(balance) },
            || async { Ok(reserved_items) },
            || async { Err(anyhow::anyhow!("no reserve proof")) },
            Decimal::ZERO,
            RefundPolicyWire::FullRefund,
        )
        .await
        .unwrap();

        assert_eq!(result.price, rate.value().ask().unwrap());
        assert_eq!(result.min_quantity, min_buy);
        assert_eq!(result.max_quantity, max_buy);
    }

    #[tokio::test]
    async fn test_make_quote_with_reserved_amounts() {
        let min_buy = bitcoin::Amount::from_sat(50_000);
        let max_buy = bitcoin::Amount::from_sat(300_000);
        let rate = FixedRate::default();
        let balance = monero::Amount::parse_monero("1.0").unwrap();
        let reserved_items = vec![
            MockReservedItem {
                reserved: monero::Amount::parse_monero("0.2").unwrap(),
            },
            MockReservedItem {
                reserved: monero::Amount::parse_monero("0.3").unwrap(),
            },
        ];

        let result = make_quote(
            min_buy,
            max_buy,
            rate.clone(),
            || async { Ok(balance) },
            || async { Ok(reserved_items) },
            || async { Err(anyhow::anyhow!("no reserve proof")) },
            Decimal::ZERO,
            RefundPolicyWire::FullRefund,
        )
        .await
        .unwrap();

        // With 1.0 XMR balance and 0.5 XMR reserved, we have 0.5 XMR available
        // At rate 0.01, that's 0.005 BTC = 500,000 sats maximum
        let expected_max = bitcoin::Amount::from_sat(300_000); // Limited by max_buy
        assert_eq!(result.min_quantity, min_buy);
        assert_eq!(result.max_quantity, expected_max);
    }

    #[tokio::test]
    async fn test_make_quote_insufficient_balance_for_min() {
        let min_buy = bitcoin::Amount::from_sat(600_000); // More than available
        let max_buy = bitcoin::Amount::from_sat(800_000);
        let rate = FixedRate::default();
        let balance = monero::Amount::parse_monero("0.5").unwrap(); // Only 0.005 BTC worth at rate 0.01
        let reserved_items: Vec<MockReservedItem> = vec![];

        let result = make_quote(
            min_buy,
            max_buy,
            rate.clone(),
            || async { Ok(balance) },
            || async { Ok(reserved_items) },
            || async { Err(anyhow::anyhow!("no reserve proof")) },
            Decimal::ZERO,
            RefundPolicyWire::FullRefund,
        )
        .await
        .unwrap();

        // Should return zero quantities when min_buy exceeds available balance
        assert_eq!(result.min_quantity, bitcoin::Amount::ZERO);
        assert_eq!(result.max_quantity, bitcoin::Amount::ZERO);
    }

    #[tokio::test]
    async fn test_make_quote_limited_by_balance() {
        let min_buy = bitcoin::Amount::from_sat(100_000);
        let max_buy = bitcoin::Amount::from_sat(800_000); // More than available
        let rate = FixedRate::default();
        let balance = monero::Amount::parse_monero("0.6").unwrap(); // 0.006 BTC worth at rate 0.01
        let reserved_items: Vec<MockReservedItem> = vec![];

        let result = make_quote(
            min_buy,
            max_buy,
            rate.clone(),
            || async { Ok(balance) },
            || async { Ok(reserved_items) },
            || async { Err(anyhow::anyhow!("no reserve proof")) },
            Decimal::ZERO,
            RefundPolicyWire::FullRefund,
        )
        .await
        .unwrap();

        // Calculate the actual max bitcoin for the given balance and rate
        let expected_max = balance
            .max_bitcoin_for_price(rate.value().ask().unwrap())
            .unwrap();
        assert_eq!(result.min_quantity, min_buy);
        assert_eq!(result.max_quantity, expected_max);
    }

    #[tokio::test]
    async fn test_make_quote_all_balance_reserved() {
        let min_buy = bitcoin::Amount::from_sat(100_000);
        let max_buy = bitcoin::Amount::from_sat(500_000);
        let rate = FixedRate::default();
        let balance = monero::Amount::parse_monero("1.0").unwrap();
        let reserved_items = vec![MockReservedItem {
            reserved: monero::Amount::parse_monero("1.0").unwrap(), // All balance reserved
        }];

        let result = make_quote(
            min_buy,
            max_buy,
            rate.clone(),
            || async { Ok(balance) },
            || async { Ok(reserved_items) },
            || async { Err(anyhow::anyhow!("no reserve proof")) },
            Decimal::ZERO,
            RefundPolicyWire::FullRefund,
        )
        .await
        .unwrap();

        // Should return zero quantities when all balance is reserved
        assert_eq!(result.min_quantity, bitcoin::Amount::ZERO);
        assert_eq!(result.max_quantity, bitcoin::Amount::ZERO);
    }

    #[tokio::test]
    async fn test_make_quote_error_getting_balance() {
        let min_buy = bitcoin::Amount::from_sat(100_000);
        let max_buy = bitcoin::Amount::from_sat(500_000);
        let rate = FixedRate::default();
        let reserved_items: Vec<MockReservedItem> = vec![];

        let result = make_quote(
            min_buy,
            max_buy,
            rate.clone(),
            || async { Err(anyhow::anyhow!("Failed to get balance")) },
            || async { Ok(reserved_items) },
            || async { Err(anyhow::anyhow!("no reserve proof")) },
            Decimal::ZERO,
            RefundPolicyWire::FullRefund,
        )
        .await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to get unlocked Monero balance")
        );
    }

    #[tokio::test]
    async fn test_make_quote_empty_reserved_items() {
        let min_buy = bitcoin::Amount::from_sat(100_000);
        let max_buy = bitcoin::Amount::from_sat(500_000);
        let rate = FixedRate::default();
        let balance = monero::Amount::parse_monero("1.0").unwrap();
        let reserved_items: Vec<MockReservedItem> = vec![];

        let result = make_quote(
            min_buy,
            max_buy,
            rate.clone(),
            || async { Ok(balance) },
            || async { Ok(reserved_items) },
            || async { Err(anyhow::anyhow!("no reserve proof")) },
            Decimal::ZERO,
            RefundPolicyWire::FullRefund,
        )
        .await
        .unwrap();

        // Should work normally with empty reserved items
        assert_eq!(result.price, rate.value().ask().unwrap());
        assert_eq!(result.min_quantity, min_buy);
        assert_eq!(result.max_quantity, max_buy);
    }

    #[tokio::test]
    async fn test_make_quote_with_developer_tip() {
        let min_buy = bitcoin::Amount::from_sat(100_000);
        let max_buy = bitcoin::Amount::from_sat(5_000_000); // High enough to be balance-limited
        let rate = FixedRate::default();
        let balance = monero::Amount::parse_monero("1.0").unwrap();
        let reserved_items: Vec<MockReservedItem> = vec![];
        let developer_tip = Decimal::new(5, 2); // 0.05 = 5%

        let result = make_quote(
            min_buy,
            max_buy,
            rate.clone(),
            || async { Ok(balance) },
            || async { Ok(reserved_items) },
            || async { Err(anyhow::anyhow!("no reserve proof")) },
            developer_tip,
            RefundPolicyWire::FullRefund,
        )
        .await
        .unwrap();

        // Compute expected max: effective balance is reduced by the tip multiplier
        let unreserved = unreserved_monero_balance(balance, std::iter::empty(), developer_tip);
        let expected_max = unreserved
            .max_bitcoin_for_price(rate.value().ask().unwrap())
            .unwrap();

        assert_eq!(result.min_quantity, min_buy);
        assert_eq!(result.max_quantity, expected_max);
        // The tip should have reduced max_quantity below max_buy
        assert!(result.max_quantity < max_buy);
    }

    // Mock struct for testing
    #[derive(Debug, Clone)]
    struct MockReservedItem {
        reserved: Amount,
    }

    impl ReservesMonero for MockReservedItem {
        fn reserved_monero(&self) -> Amount {
            self.reserved
        }
    }
}
