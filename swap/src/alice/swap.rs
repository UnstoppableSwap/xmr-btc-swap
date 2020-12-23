//! Run an XMR/BTC swap in the role of Alice.
//! Alice holds XMR and wishes receive BTC.
use crate::{
    alice::{
        event_loop::EventLoopHandle,
        steps::{
            build_bitcoin_punish_transaction, build_bitcoin_redeem_transaction,
            extract_monero_private_key, lock_xmr, negotiate, publish_bitcoin_punish_transaction,
            publish_bitcoin_redeem_transaction, publish_cancel_transaction,
            wait_for_bitcoin_encrypted_signature, wait_for_bitcoin_refund, wait_for_locked_bitcoin,
        },
    },
    bitcoin,
    bitcoin::EncryptedSignature,
    network::request_response::AliceToBob,
    state,
    state::{Alice, AliceEndState, Swap},
    storage::Database,
    SwapAmounts,
};
use anyhow::Result;
use async_recursion::async_recursion;
use futures::{
    future::{select, Either},
    pin_mut,
};
use libp2p::request_response::ResponseChannel;
use rand::{CryptoRng, RngCore};
use std::{fmt, sync::Arc};
use tracing::info;
use uuid::Uuid;
use xmr_btc::{
    alice::{State0, State3},
    bitcoin::{TransactionBlockHeight, TxCancel, TxRefund, WatchForRawTransaction},
    config::Config,
    monero::CreateWalletForOutput,
    ExpiredTimelocks,
};

trait Rng: RngCore + CryptoRng + Send {}

impl<T> Rng for T where T: RngCore + CryptoRng + Send {}

#[allow(clippy::large_enum_variant)]
pub enum AliceState {
    Started {
        amounts: SwapAmounts,
        state0: State0,
    },
    Negotiated {
        channel: Option<ResponseChannel<AliceToBob>>,
        amounts: SwapAmounts,
        state3: State3,
    },
    BtcLocked {
        channel: Option<ResponseChannel<AliceToBob>>,
        amounts: SwapAmounts,
        state3: State3,
    },
    XmrLocked {
        state3: State3,
    },
    EncSigLearned {
        state3: State3,
        encrypted_signature: EncryptedSignature,
    },
    BtcRedeemed,
    BtcCancelled {
        state3: State3,
        tx_cancel: TxCancel,
    },
    BtcRefunded {
        spend_key: monero::PrivateKey,
        state3: State3,
    },
    BtcPunishable {
        tx_refund: TxRefund,
        state3: State3,
    },
    XmrRefunded,
    CancelTimelockExpired {
        state3: State3,
    },
    BtcPunished,
    SafelyAborted,
}

impl fmt::Display for AliceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AliceState::Started { .. } => write!(f, "started"),
            AliceState::Negotiated { .. } => write!(f, "negotiated"),
            AliceState::BtcLocked { .. } => write!(f, "btc is locked"),
            AliceState::XmrLocked { .. } => write!(f, "xmr is locked"),
            AliceState::EncSigLearned { .. } => write!(f, "encrypted signature is learned"),
            AliceState::BtcRedeemed => write!(f, "btc is redeemed"),
            AliceState::BtcCancelled { .. } => write!(f, "btc is cancelled"),
            AliceState::BtcRefunded { .. } => write!(f, "btc is refunded"),
            AliceState::BtcPunished => write!(f, "btc is punished"),
            AliceState::SafelyAborted => write!(f, "safely aborted"),
            AliceState::BtcPunishable { .. } => write!(f, "btc is punishable"),
            AliceState::XmrRefunded => write!(f, "xmr is refunded"),
            AliceState::CancelTimelockExpired { .. } => write!(f, "cancel timelock is expired"),
        }
    }
}

impl From<&AliceState> for state::Alice {
    fn from(alice_state: &AliceState) -> Self {
        match alice_state {
            AliceState::Negotiated { state3, .. } => Alice::Negotiated(state3.clone()),
            AliceState::BtcLocked { state3, .. } => Alice::BtcLocked(state3.clone()),
            AliceState::XmrLocked { state3 } => Alice::XmrLocked(state3.clone()),
            AliceState::EncSigLearned {
                state3,
                encrypted_signature,
            } => Alice::EncSigLearned {
                state: state3.clone(),
                encrypted_signature: encrypted_signature.clone(),
            },
            AliceState::BtcRedeemed => Alice::Done(AliceEndState::BtcRedeemed),
            AliceState::BtcCancelled { state3, .. } => Alice::BtcCancelled(state3.clone()),
            AliceState::BtcRefunded { spend_key, state3 } => Alice::BtcRefunded {
                spend_key: *spend_key,
                state3: state3.clone(),
            },
            AliceState::BtcPunishable { state3, .. } => Alice::BtcPunishable(state3.clone()),
            AliceState::XmrRefunded => Alice::Done(AliceEndState::XmrRefunded),
            AliceState::CancelTimelockExpired { state3 } => {
                Alice::CancelTimelockExpired(state3.clone())
            }
            AliceState::BtcPunished => Alice::Done(AliceEndState::BtcPunished),
            AliceState::SafelyAborted => Alice::Done(AliceEndState::SafelyAborted),
            AliceState::Started { amounts, state0 } => Alice::Started {
                amounts: *amounts,
                state0: state0.clone(),
            },
        }
    }
}

impl From<state::Alice> for AliceState {
    fn from(db_state: state::Alice) -> Self {
        use AliceState::*;
        match db_state {
            Alice::Started { amounts, state0 } => Started { amounts, state0 },
            Alice::Negotiated(state3) => Negotiated {
                channel: None,
                amounts: SwapAmounts {
                    btc: state3.btc,
                    xmr: state3.xmr,
                },
                state3,
            },
            Alice::BtcLocked(state3) => BtcLocked {
                channel: None,
                amounts: SwapAmounts {
                    btc: state3.btc,
                    xmr: state3.xmr,
                },
                state3,
            },
            Alice::XmrLocked(state3) => XmrLocked { state3 },
            Alice::EncSigLearned {
                state,
                encrypted_signature,
            } => EncSigLearned {
                state3: state,
                encrypted_signature,
            },
            Alice::CancelTimelockExpired(state3) => AliceState::CancelTimelockExpired { state3 },
            Alice::BtcCancelled(state) => {
                let tx_cancel = bitcoin::TxCancel::new(
                    &state.tx_lock,
                    state.cancel_timelock,
                    state.a.public(),
                    state.B,
                );

                BtcCancelled {
                    state3: state,
                    tx_cancel,
                }
            }
            Alice::BtcPunishable(state) => {
                let tx_cancel = bitcoin::TxCancel::new(
                    &state.tx_lock,
                    state.cancel_timelock,
                    state.a.public(),
                    state.B,
                );
                let tx_refund = bitcoin::TxRefund::new(&tx_cancel, &state.refund_address);
                BtcPunishable {
                    tx_refund,
                    state3: state,
                }
            }
            Alice::BtcRefunded {
                state3: state,
                spend_key,
                ..
            } => BtcRefunded {
                spend_key,
                state3: state,
            },
            Alice::Done(end_state) => match end_state {
                AliceEndState::SafelyAborted => SafelyAborted,
                AliceEndState::BtcRedeemed => BtcRedeemed,
                AliceEndState::XmrRefunded => XmrRefunded,
                AliceEndState::BtcPunished => BtcPunished,
            },
        }
    }
}

pub async fn swap(
    state: AliceState,
    event_loop_handle: EventLoopHandle,
    bitcoin_wallet: Arc<crate::bitcoin::Wallet>,
    monero_wallet: Arc<crate::monero::Wallet>,
    config: Config,
    swap_id: Uuid,
    db: Database,
) -> Result<AliceState> {
    run_until(
        state,
        is_complete,
        event_loop_handle,
        bitcoin_wallet,
        monero_wallet,
        config,
        swap_id,
        db,
    )
    .await
}

pub fn is_complete(state: &AliceState) -> bool {
    matches!(
        state,
        AliceState::XmrRefunded
            | AliceState::BtcRedeemed
            | AliceState::BtcPunished
            | AliceState::SafelyAborted
    )
}

pub fn is_xmr_locked(state: &AliceState) -> bool {
    matches!(
        state,
        AliceState::XmrLocked{..}
    )
}

pub fn is_encsig_learned(state: &AliceState) -> bool {
    matches!(
        state,
        AliceState::EncSigLearned{..}
    )
}

// State machine driver for swap execution
#[async_recursion]
#[allow(clippy::too_many_arguments)]
pub async fn run_until(
    state: AliceState,
    is_target_state: fn(&AliceState) -> bool,
    mut event_loop_handle: EventLoopHandle,
    bitcoin_wallet: Arc<crate::bitcoin::Wallet>,
    monero_wallet: Arc<crate::monero::Wallet>,
    config: Config,
    swap_id: Uuid,
    db: Database,
    // TODO: Remove EventLoopHandle!
) -> Result<AliceState> {
    info!("Current state:{}", state);
    if is_target_state(&state) {
        Ok(state)
    } else {
        match state {
            AliceState::Started { amounts, state0 } => {
                let (channel, state3) =
                    negotiate(state0, amounts, &mut event_loop_handle, config).await?;

                let state = AliceState::Negotiated {
                    channel: Some(channel),
                    amounts,
                    state3,
                };

                let db_state = (&state).into();
                db.insert_latest_state(swap_id, Swap::Alice(db_state))
                    .await?;
                run_until(
                    state,
                    is_target_state,
                    event_loop_handle,
                    bitcoin_wallet,
                    monero_wallet,
                    config,
                    swap_id,
                    db,
                )
                .await
            }
            AliceState::Negotiated {
                state3,
                channel,
                amounts,
            } => {
                let state = match channel {
                    Some(channel) => {
                        let _ = wait_for_locked_bitcoin(
                            state3.tx_lock.txid(),
                            bitcoin_wallet.clone(),
                            config,
                        )
                        .await?;

                        AliceState::BtcLocked {
                            channel: Some(channel),
                            amounts,
                            state3,
                        }
                    }
                    None => {
                        tracing::info!("Cannot resume swap from negotiated state, aborting");

                        // Alice did not lock Xmr yet
                        AliceState::SafelyAborted
                    }
                };

                let db_state = (&state).into();
                db.insert_latest_state(swap_id, Swap::Alice(db_state))
                    .await?;
                run_until(
                    state,
                    is_target_state,
                    event_loop_handle,
                    bitcoin_wallet,
                    monero_wallet,
                    config,
                    swap_id,
                    db,
                )
                .await
            }
            AliceState::BtcLocked {
                channel,
                amounts,
                state3,
            } => {
                let state = match channel {
                    Some(channel) => {
                        lock_xmr(
                            channel,
                            amounts,
                            state3.clone(),
                            &mut event_loop_handle,
                            monero_wallet.clone(),
                        )
                        .await?;

                        AliceState::XmrLocked { state3 }
                    }
                    None => {
                        tracing::info!("Cannot resume swap from BTC locked state, aborting");

                        // Alice did not lock Xmr yet
                        AliceState::SafelyAborted
                    }
                };

                let db_state = (&state).into();
                db.insert_latest_state(swap_id, Swap::Alice(db_state))
                    .await?;
                run_until(
                    state,
                    is_target_state,
                    event_loop_handle,
                    bitcoin_wallet,
                    monero_wallet,
                    config,
                    swap_id,
                    db,
                )
                .await
            }
            AliceState::XmrLocked { state3 } => {
                // todo: match statement and wait for cancel timelock to expire can probably be
                // expressed more cleanly
                let state = match state3.expired_timelocks(bitcoin_wallet.as_ref()).await? {
                    ExpiredTimelocks::None => {
                        let wait_for_enc_sig = wait_for_bitcoin_encrypted_signature(
                            &mut event_loop_handle,
                            config.monero_max_finality_time,
                        );
                        let state3_clone = state3.clone();
                        let cancel_timelock_expires = state3_clone
                            .wait_for_cancel_timelock_to_expire(bitcoin_wallet.as_ref());

                        pin_mut!(wait_for_enc_sig);
                        pin_mut!(cancel_timelock_expires);

                        match select(cancel_timelock_expires, wait_for_enc_sig).await {
                            Either::Left(_) => AliceState::CancelTimelockExpired { state3 },
                            Either::Right((enc_sig, _)) => AliceState::EncSigLearned {
                                state3,
                                encrypted_signature: enc_sig?,
                            },
                        }
                    }
                    _ => AliceState::CancelTimelockExpired { state3 },
                };

                let db_state = (&state).into();
                db.insert_latest_state(swap_id, Swap::Alice(db_state))
                    .await?;
                run_until(
                    state,
                    is_target_state,
                    event_loop_handle,
                    bitcoin_wallet.clone(),
                    monero_wallet,
                    config,
                    swap_id,
                    db,
                )
                .await
            }
            AliceState::EncSigLearned {
                state3,
                encrypted_signature,
            } => {
                // TODO: Evaluate if it is correct for Alice to Redeem no matter what.
                //  If cancel timelock expired she should potentially not try redeem. (The
                // implementation  gives her an advantage.)

                let signed_tx_redeem = match build_bitcoin_redeem_transaction(
                    encrypted_signature,
                    &state3.tx_lock,
                    state3.a.clone(),
                    state3.s_a,
                    state3.B,
                    &state3.redeem_address,
                ) {
                    Ok(tx) => tx,
                    Err(_) => {
                        state3
                            .wait_for_cancel_timelock_to_expire(bitcoin_wallet.as_ref())
                            .await?;

                        let state = AliceState::CancelTimelockExpired { state3 };
                        let db_state = (&state).into();
                        db.insert_latest_state(swap_id, Swap::Alice(db_state))
                            .await?;
                        return run_until(
                            state,
                            is_target_state,
                            event_loop_handle,
                            bitcoin_wallet,
                            monero_wallet,
                            config,
                            swap_id,
                            db,
                        )
                        .await;
                    }
                };

                // TODO(Franck): Error handling is delicate here.
                // If Bob sees this transaction he can redeem Monero
                // e.g. If the Bitcoin node is down then the user needs to take action.
                publish_bitcoin_redeem_transaction(
                    signed_tx_redeem,
                    bitcoin_wallet.clone(),
                    config,
                )
                .await?;

                let state = AliceState::BtcRedeemed;
                let db_state = (&state).into();
                db.insert_latest_state(swap_id, Swap::Alice(db_state))
                    .await?;
                run_until(
                    state,
                    is_target_state,
                    event_loop_handle,
                    bitcoin_wallet,
                    monero_wallet,
                    config,
                    swap_id,
                    db,
                )
                .await
            }
            AliceState::CancelTimelockExpired { state3 } => {
                let tx_cancel = publish_cancel_transaction(
                    state3.tx_lock.clone(),
                    state3.a.clone(),
                    state3.B,
                    state3.cancel_timelock,
                    state3.tx_cancel_sig_bob.clone(),
                    bitcoin_wallet.clone(),
                )
                .await?;

                let state = AliceState::BtcCancelled { state3, tx_cancel };
                let db_state = (&state).into();
                db.insert_latest_state(swap_id, Swap::Alice(db_state))
                    .await?;
                run_until(
                    state,
                    is_target_state,
                    event_loop_handle,
                    bitcoin_wallet,
                    monero_wallet,
                    config,
                    swap_id,
                    db,
                )
                .await
            }
            AliceState::BtcCancelled { state3, tx_cancel } => {
                let tx_cancel_height = bitcoin_wallet
                    .transaction_block_height(tx_cancel.txid())
                    .await;

                let (tx_refund, published_refund_tx) = wait_for_bitcoin_refund(
                    &tx_cancel,
                    tx_cancel_height,
                    state3.punish_timelock,
                    &state3.refund_address,
                    bitcoin_wallet.clone(),
                )
                .await?;

                // TODO(Franck): Review error handling
                match published_refund_tx {
                    None => {
                        let state = AliceState::BtcPunishable { tx_refund, state3 };
                        let db_state = (&state).into();
                        db.insert_latest_state(swap_id, Swap::Alice(db_state))
                            .await?;
                        swap(
                            state,
                            event_loop_handle,
                            bitcoin_wallet.clone(),
                            monero_wallet,
                            config,
                            swap_id,
                            db,
                        )
                        .await
                    }
                    Some(published_refund_tx) => {
                        let spend_key = extract_monero_private_key(
                            published_refund_tx,
                            tx_refund,
                            state3.s_a,
                            state3.a.clone(),
                            state3.S_b_bitcoin,
                        )?;

                        let state = AliceState::BtcRefunded { spend_key, state3 };
                        let db_state = (&state).into();
                        db.insert_latest_state(swap_id, Swap::Alice(db_state))
                            .await?;
                        run_until(
                            state,
                            is_target_state,
                            event_loop_handle,
                            bitcoin_wallet.clone(),
                            monero_wallet,
                            config,
                            swap_id,
                            db,
                        )
                        .await
                    }
                }
            }
            AliceState::BtcRefunded { spend_key, state3 } => {
                let view_key = state3.v;

                monero_wallet
                    .create_and_load_wallet_for_output(spend_key, view_key)
                    .await?;

                let state = AliceState::XmrRefunded;
                let db_state = (&state).into();
                db.insert_latest_state(swap_id, Swap::Alice(db_state))
                    .await?;
                Ok(state)
            }
            AliceState::BtcPunishable { tx_refund, state3 } => {
                let signed_tx_punish = build_bitcoin_punish_transaction(
                    &state3.tx_lock,
                    state3.cancel_timelock,
                    &state3.punish_address,
                    state3.punish_timelock,
                    state3.tx_punish_sig_bob.clone(),
                    state3.a.clone(),
                    state3.B,
                )?;

                let punish_tx_finalised = publish_bitcoin_punish_transaction(
                    signed_tx_punish,
                    bitcoin_wallet.clone(),
                    config,
                );

                let refund_tx_seen = bitcoin_wallet.watch_for_raw_transaction(tx_refund.txid());

                pin_mut!(punish_tx_finalised);
                pin_mut!(refund_tx_seen);

                match select(punish_tx_finalised, refund_tx_seen).await {
                    Either::Left(_) => {
                        let state = AliceState::BtcPunished;
                        let db_state = (&state).into();
                        db.insert_latest_state(swap_id, Swap::Alice(db_state))
                            .await?;
                        run_until(
                            state,
                            is_target_state,
                            event_loop_handle,
                            bitcoin_wallet.clone(),
                            monero_wallet,
                            config,
                            swap_id,
                            db,
                        )
                        .await
                    }
                    Either::Right((published_refund_tx, _)) => {
                        let spend_key = extract_monero_private_key(
                            published_refund_tx,
                            tx_refund,
                            state3.s_a,
                            state3.a.clone(),
                            state3.S_b_bitcoin,
                        )?;
                        let state = AliceState::BtcRefunded { spend_key, state3 };
                        let db_state = (&state).into();
                        db.insert_latest_state(swap_id, Swap::Alice(db_state))
                            .await?;
                        run_until(
                            state,
                            is_target_state,
                            event_loop_handle,
                            bitcoin_wallet.clone(),
                            monero_wallet,
                            config,
                            swap_id,
                            db,
                        )
                        .await
                    }
                }
            }
            AliceState::XmrRefunded => Ok(AliceState::XmrRefunded),
            AliceState::BtcRedeemed => Ok(AliceState::BtcRedeemed),
            AliceState::BtcPunished => Ok(AliceState::BtcPunished),
            AliceState::SafelyAborted => Ok(AliceState::SafelyAborted),
        }
    }
}