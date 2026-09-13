//! Run an XMR/BTC swap in the role of Alice.
//! Alice holds XMR and wishes receive BTC.
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use crate::asb::{EventLoopHandle, LatestRate};
use crate::common::retry;
use crate::monero;
use crate::monero::TransferProof;
use crate::protocol::alice::lock_phase::MoneroLockPhase;
use crate::protocol::alice::{AliceState, HermesFundingPolicy, Swap, TipConfig};
use ::bitcoin::consensus::encode::serialize_hex;
use anyhow::{Context, Result, bail};
use bitcoin_wallet::BitcoinWallet;
use monero_interface::PublishTransaction;
use monero_oxide_wallet::transaction::{NotPruned, Transaction};
use rust_decimal::Decimal;
use swap_core::bitcoin::ExpiredTimelocks;
use swap_core::monero::BlockHeight;
use swap_env::env::Config;
use swap_machine::alice::State3;
use tokio::select;
use tokio::time::timeout;
use uuid::Uuid;

/// Serializes the Monero lock phase (output selection in `BtcLocked` through the first
/// relay in `XmrLockTransactionConstructed`) across all swaps in the process. wallet2 only
/// marks an output spent once its lock transaction is relayed and monero-sys has no reserve
/// API, so without this two overlapping swaps pick the same output and the loser's lock
/// transaction is a permanent double-spend that monerod rejects forever. Each swap tracks
/// its participation through a session held on `run_until`'s stack because construct and
/// publish are separate states.
static MONERO_LOCK_PHASE: LazyLock<MoneroLockPhase> =
    LazyLock::new(|| MoneroLockPhase::new(MONERO_LOCK_PHASE_MAX_HOLD));

/// Maximum time a swap may hold [`MONERO_LOCK_PHASE`]. Past it a swap wedged on a rejected
/// publish releases the guard and continues unserialized instead of starving the others.
const MONERO_LOCK_PHASE_MAX_HOLD: Duration = Duration::from_secs(20 * 60);

/// `.expect` message for the persist retry loop, which never stops retrying.
const PERSIST_EXPECT: &str = "we never stop retrying to persist the latest Alice state";

pub async fn run<LR>(swap: Swap, rate_service: LR) -> Result<AliceState>
where
    LR: LatestRate + Clone,
{
    run_until(swap, |_| false, rate_service).await
}

#[tracing::instrument(name = "swap", skip(swap,exit_early,rate_service), fields(id = %swap.swap_id), err)]
pub async fn run_until<LR>(
    mut swap: Swap,
    exit_early: fn(&AliceState) -> bool,
    rate_service: LR,
) -> Result<AliceState>
where
    LR: LatestRate + Clone,
{
    let mut current_state = swap.state;

    // Tracks this swap's participation in the serialized Monero lock phase, across the
    // separate `next_state` calls and the persist between them. See [`MONERO_LOCK_PHASE`].
    let mut lock_phase = MONERO_LOCK_PHASE.session();

    while !swap_machine::alice::is_complete(&current_state) && !exit_early(&current_state) {
        lock_phase
            .sync_to(in_monero_lock_phase(&current_state))
            .await;

        // While holding the permit, bound each step: the publish arm retries without a limit,
        // and a wedged swap must not keep others from locking Monero. Cancelling is safe:
        // no state is persisted and both arms are re-entrant.
        let step_deadline = lock_phase.deadline();

        let step = next_state(
            swap.swap_id,
            current_state.clone(),
            &mut swap.event_loop_handle,
            swap.bitcoin_wallet.clone(),
            swap.monero_wallet.clone(),
            &swap.env_config,
            swap.developer_tip.clone(),
            swap.hermes_funding_policy,
            rate_service.clone(),
        );

        current_state = match step_deadline {
            Some(deadline) => match timeout(deadline, step).await {
                Ok(next) => next?,
                Err(_) => {
                    abandon_lock_phase(&current_state, &swap.monero_wallet).await;
                    lock_phase.abandon();
                    continue;
                }
            },
            None => step.await?,
        };

        // Release before the persist: the persist retries without a limit and must not pin
        // the process-wide permit.
        if !in_monero_lock_phase(&current_state) {
            lock_phase.release();
        }

        let persist = retry(
            "Persisting latest Alice state",
            || {
                let db = swap.db.clone();
                let state = current_state.clone();

                async move {
                    db.insert_latest_state(swap.swap_id, state.into())
                        .await
                        .map_err(backoff::Error::transient)
                }
            },
            None,
            None,
        );

        // A persist of an in-phase state counts against the same deadline; past it we release
        // the guard and finish the persist unserialized (the persist itself is never dropped).
        match lock_phase.deadline() {
            Some(remaining) => {
                tokio::pin!(persist);
                match timeout(remaining, &mut persist).await {
                    Ok(persisted) => persisted.expect(PERSIST_EXPECT),
                    Err(_) => {
                        abandon_lock_phase(&current_state, &swap.monero_wallet).await;
                        lock_phase.abandon();
                        persist.await.expect(PERSIST_EXPECT);
                    }
                }
            }
            None => persist.await.expect(PERSIST_EXPECT),
        }
    }

    Ok(current_state)
}

async fn next_state<LR>(
    swap_id: Uuid,
    state: AliceState,
    event_loop_handle: &mut EventLoopHandle,
    bitcoin_wallet: Arc<dyn BitcoinWallet>,
    monero_wallet: Arc<monero::Wallets>,
    env_config: &Config,
    developer_tip: TipConfig,
    hermes_funding_policy: HermesFundingPolicy,
    mut rate_service: LR,
) -> Result<AliceState>
where
    LR: LatestRate,
{
    let rate = rate_service
        .latest_rate()
        .map_or("NaN".to_string(), |rate| format!("{}", rate));

    tracing::info!(%state, %rate, "Advancing state");

    Ok(match state {
        AliceState::Started { state3 } => {
            let tx_lock_status = bitcoin_wallet
                .subscribe_to(Box::new(state3.tx_lock.clone()))
                .await;

            match timeout(
                env_config.bitcoin_lock_mempool_timeout,
                tx_lock_status.wait_until_seen(),
            )
            .await
            {
                Err(_) => {
                    tracing::info!(
                        minutes = %env_config.bitcoin_lock_mempool_timeout.as_secs_f64() / 60.0,
                        "TxLock lock was not seen in mempool in time. Alice might have denied our offer.",
                    );
                    AliceState::SafelyAborted
                }
                Ok(res) => {
                    res?;
                    AliceState::BtcLockTransactionSeen { state3 }
                }
            }
        }
        AliceState::BtcLockTransactionSeen { state3 } => {
            let tx_lock_status = bitcoin_wallet
                .subscribe_to(Box::new(state3.tx_lock.clone()))
                .await;

            match timeout(
                env_config.bitcoin_lock_confirmed_timeout,
                tx_lock_status.wait_until_final(),
            )
            .await
            {
                Err(_) => {
                    tracing::info!(
                        confirmations_needed = %env_config.bitcoin_finality_confirmations,
                        minutes = %env_config.bitcoin_lock_confirmed_timeout.as_secs_f64() / 60.0,
                        "TxLock lock did not get enough confirmations in time",
                    );

                    AliceState::BtcEarlyRefundable { state3 }
                }
                Ok(res) => {
                    res?;
                    AliceState::BtcLocked { state3 }
                }
            }
        }
        AliceState::BtcLocked { state3 } => {
            // Sometimes locking the Monero can fail e.g due to the daemon not being fully synced
            // We will retry indefinitely to lock the Monero funds, until either:
            // - the cancel timelock expires
            // - we do not manage to lock the Monero funds within the timeout
            let backoff = backoff::ExponentialBackoffBuilder::new()
                .with_max_elapsed_time(Some(env_config.monero_lock_retry_timeout))
                .with_max_interval(Duration::from_secs(30))
                .build();

            let constructed = backoff::future::retry_notify(
                backoff,
                || async {
                    // We check the status of the Bitcoin lock transaction
                    // If the swap is cancelled, there is no need to lock the Monero funds anymore
                    // because there is no way for the swap to succeed.
                    if !cancel_timelock_not_expired(&state3, &*bitcoin_wallet)
                        .await
                        .context("Failed to check for expired timelocks before locking Monero")
                        .map_err(backoff::Error::transient)?
                    {
                        return Ok(None);
                    }

                    // Record the current monero wallet block height so we don't have to scan from
                    // block 0 for scenarios where we create a refund wallet.
                    let monero_wallet_restore_blockheight = monero_wallet
                        .direct_rpc_block_height()
                        .await
                        .context("Failed to get Monero wallet block height")
                        .map_err(backoff::Error::transient)?;

                    let (lock_address, amount) = state3
                        .lock_xmr_transfer_request()
                        .address_and_amount(env_config.monero_network);

                    let hermes_funding_amount = hermes_funding_policy.funding_amount(state3.btc);

                    let hermes_funding = state3
                        .hermes_funding_transfer_request(hermes_funding_amount)
                        .address_and_amount(env_config.monero_network);

                    let destinations = build_transfer_destinations(
                        lock_address,
                        amount,
                        hermes_funding,
                        developer_tip.clone(),
                    )?;

                    // Under MONERO_LOCK_PHASE, refuse to construct a lock the wallet cannot
                    // fund. A concurrent swap that already locked its Monero has reduced the
                    // spendable balance; without this guard wallet2 can still reselect the
                    // sibling's freshly spent outputs, build a lock that double-spends, and then
                    // wedge on a permanently rejected publish. A permanent error here routes to an
                    // early Bitcoin refund (no Monero was locked, so it is safe) instead. This is a
                    // best-effort guard bounded by how promptly the wallet reflects the sibling
                    // spend; a fully reliable check would query the daemon for spent key images.
                    let needed_pico = destinations
                        .iter()
                        .map(|(_, amount)| amount.as_pico())
                        .sum::<u64>()
                        .saturating_add(swap_core::monero::CONSERVATIVE_MONERO_FEE.as_pico());
                    let unlocked_pico = monero_wallet
                        .main_wallet()
                        .await
                        .unlocked_balance()
                        .await
                        .context("Failed to read unlocked Monero balance before constructing the lock transaction")
                        .map_err(backoff::Error::transient)?
                        .as_pico();
                    if unlocked_pico < needed_pico {
                        return Err(backoff::Error::permanent(anyhow::anyhow!(
                            "Insufficient unlocked Monero to fund the lock transaction \
                             ({unlocked_pico} < {needed_pico} piconero); a concurrent swap consumed \
                             the shared balance, refunding this swap early"
                        )));
                    }

                    let constructed = monero_wallet
                        .construct_multi_destination_tx(&destinations)
                        .await
                        .map_err(|e| tracing::error!(err=%e, "Failed to construct Monero lock transaction"))
                        .ok();

                    let Some((xmr_lock_tx, receipt)) = constructed else {
                        return Err(backoff::Error::transient(anyhow::anyhow!(
                            "Failed to construct Monero lock transaction"
                        )));
                    };

                    let tx_key = receipt.tx_keys.get(&lock_address.to_string()).expect("monero-sys guarantees that the address has a valid tx key or the tx isn't published");

                    Ok(Some((
                        monero_wallet_restore_blockheight,
                        TransferProof::new(
                            monero::TxHash(receipt.txid),
                            *tx_key,
                        ),
                        xmr_lock_tx,
                    )))
                },
                |e, wait_time: Duration| {
                    tracing::warn!(
                        swap_id = %swap_id,
                        error = ?e,
                        "Failed to construct Monero lock transaction. We will retry in {} seconds",
                        wait_time.as_secs()
                    )
                },
            )
            .await;

            match constructed {
                // If the construction was successful, we transition to the next state
                Ok(Some((monero_wallet_restore_blockheight, transfer_proof, xmr_lock_tx))) => {
                    AliceState::XmrLockTransactionConstructed {
                        monero_wallet_restore_blockheight: BlockHeight {
                            height: monero_wallet_restore_blockheight,
                        },
                        xmr_lock_tx,
                        transfer_proof,
                        state3,
                    }
                }
                // If we were not able to lock the Monero funds before the timelock expired,
                // we can safely abort the swap because we did not lock any funds
                // We do not do an early refund because Bob can refund himself (timelock expired)
                Ok(None) => {
                    tracing::info!(
                        swap_id = %swap_id,
                        "We did not manage to lock the Monero funds before the timelock expired. Aborting swap."
                    );

                    AliceState::SafelyAborted
                }
                Err(e) => {
                    tracing::error!(
                        swap_id = %swap_id,
                        error = ?e,
                        "Failed to lock Monero within {} seconds. We will do an early refund of the Bitcoin. We didn't lock any Monero funds so this is safe.",
                        env_config.monero_lock_retry_timeout.as_secs()
                    );

                    AliceState::BtcEarlyRefundable { state3 }
                }
            }
        }
        AliceState::BtcEarlyRefundable { state3 } => {
            if let Some(tx_early_refund) = state3.signed_early_refund_transaction() {
                let tx_early_refund = tx_early_refund?;
                let tx_early_refund_txid = tx_early_refund.compute_txid();

                // Bob might cancel the swap and refund for himself. We won't need to early refund anymore.
                let tx_cancel_status = bitcoin_wallet
                    .subscribe_to(Box::new(state3.tx_cancel()))
                    .await;

                let backoff = backoff::ExponentialBackoffBuilder::new()
                    // We give up after 6 hours
                    // (Most likely Bob the a Replace-by-Fee on the tx_lock transaction)
                    .with_max_elapsed_time(Some(Duration::from_secs(6 * 60 * 60)))
                    // We wait a while between retries
                    .with_max_interval(Duration::from_secs(10 * 60))
                    .build();

                // Concurrently retry to broadcast the early refund transaction
                // and wait for the cancel transaction to be broadcasted.
                tokio::select! {
                    // If Bob cancels the swap, he can refund himself.
                    // Nothing for us to do anymore.
                    result = tx_cancel_status.wait_until_seen() => {
                        result?;
                        AliceState::SafelyAborted
                    }

                    // Retry repeatedly to broadcast tx_early_refund
                    result = async {
                        backoff::future::retry_notify(backoff, || async {
                            bitcoin_wallet.ensure_broadcasted(tx_early_refund.clone(), "early_refund").await.map_err(backoff::Error::transient)
                        }, |e, wait_time: Duration| {
                            tracing::warn!(
                                %tx_early_refund_txid,
                                error = ?e,
                                "Failed to broadcast early refund transaction. We will retry in {} seconds",
                                wait_time.as_secs()
                            )
                        })
                        .await
                    } => {
                        match result {
                            Ok((_txid, _subscription)) => {
                                tracing::info!(
                                    %tx_early_refund_txid,
                                    "Refunded Bitcoin early for Bob"
                                );

                                AliceState::BtcEarlyRefunded(state3)
                            }
                            Err(e) => {
                                tracing::error!(
                                    %tx_early_refund_txid,
                                    error = ?e,
                                    "Failed to broadcast early refund transaction after retries exhausted. Bob will have to wait for the timelock to expire then refund himself."
                                );
                                AliceState::SafelyAborted
                            }
                        }
                    }
                }
            } else {
                // We do not have Bob's signature for the early refund transaction
                // Therefore we cannot do an early refund.
                // We abort the swap on our side.
                // Bob will have to wait for the timelock to expire then refund himself.
                AliceState::SafelyAborted
            }
        }
        AliceState::XmrLockTransactionConstructed {
            monero_wallet_restore_blockheight,
            xmr_lock_tx,
            transfer_proof,
            state3,
        } => {
            let xmr_lock_tx_hash = monero::TxHash::from_tx(&xmr_lock_tx);

            retry::<AliceState, _, _>(
                "Publishing Monero lock transaction",
                || async {
                    let is_present = monero_wallet
                        .is_transaction_present(&xmr_lock_tx_hash)
                        .await
                        .context("Failed to check whether Monero lock transaction is already present on chain")
                        .map_err(backoff::Error::transient)?;

                    if !is_present {
                        if !cancel_timelock_not_expired(&state3, &*bitcoin_wallet)
                            .await
                            .context("Failed to check for expired timelocks before publishing Monero lock transaction")
                            .map_err(backoff::Error::transient)?
                        {
                            tracing::warn!(
                                %swap_id,
                                "Cancel timelock expired before we confirmed the Monero lock transaction was published. Publishing is not atomic, so the Monero may already be locked; waiting for the cancel timelock to recover safely instead of risking an early Bitcoin refund while the Monero is locked."
                            );
                            return Ok(AliceState::WaitingForCancelTimelockExpiration {
                                monero_wallet_restore_blockheight,
                                transfer_proof: transfer_proof.clone(),
                                state3: state3.clone(),
                            });
                        }

                        monero_wallet
                            .rpc_client()
                            .await
                            .map_err(backoff::Error::transient)?
                            .publish_transaction(&xmr_lock_tx)
                            .await
                            .context("Failed to publish Monero lock transaction")
                            .map_err(backoff::Error::transient)?;

                        tracing::info!(%swap_id, %xmr_lock_tx_hash, "Published Monero lock transaction");
                    }

                    monero_wallet
                        .main_wallet()
                        .await
                        .scan_transaction(xmr_lock_tx_hash.0.clone())
                        .await
                        .context("Failed to scan Monero lock transaction into the wallet")
                        .map_err(backoff::Error::transient)?;

                    Ok(AliceState::XmrLockTransactionSent {
                        monero_wallet_restore_blockheight,
                        transfer_proof: transfer_proof.clone(),
                        state3: state3.clone(),
                    })
                },
                None,
                None,
            )
            .await
            .context("Failed to publish Monero lock transaction")?
        }
        AliceState::XmrLockTransactionSent {
            monero_wallet_restore_blockheight,
            transfer_proof,
            state3,
        } => match state3.expired_timelocks(&*bitcoin_wallet).await? {
            ExpiredTimelocks::None { .. } => {
                tracing::info!("Locked Monero, waiting for confirmations");

                monero_wallet
                    .wait_until_confirmed(
                        &transfer_proof.tx_hash(),
                        1,
                        Some(|(xmr_lock_txid, confirmations, target_confirmations)| {
                            tracing::debug!(
                                %xmr_lock_txid,
                                %confirmations,
                                %target_confirmations,
                                "Monero lock tx got new confirmation"
                            )
                        }),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to wait until Monero transaction was confirmed ({})",
                            transfer_proof.tx_hash()
                        )
                    })?;

                AliceState::XmrLocked {
                    monero_wallet_restore_blockheight,
                    transfer_proof,
                    state3,
                }
            }
            _ => AliceState::CancelTimelockExpired {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            },
        },
        AliceState::XmrLocked {
            monero_wallet_restore_blockheight,
            transfer_proof,
            state3,
        } => {
            let tx_lock_status = bitcoin_wallet
                .subscribe_to(Box::new(state3.tx_lock.clone()))
                .await;

            tokio::select! {
                result = event_loop_handle.send_transfer_proof(transfer_proof.clone()) => {
                   result?;

                   AliceState::XmrLockTransferProofSent {
                       monero_wallet_restore_blockheight,
                       transfer_proof,
                       state3,
                   }
                },
                // If we send Bob the transfer proof, but for whatever reason we do not receive an acknowledgement from him
                // we would be stuck in this state forever until the timelock expires.
                //
                // By listening for the encrypted signature here we can still proceed to the next state
                // even if Bob does not respond with an acknowledgement but sends us the encrypted signature immediately.
                enc_sig = event_loop_handle.recv_encrypted_signature() => {
                    tracing::info!("Received encrypted signature via p2p channel. We haven't verified it yet.");

                    AliceState::EncSigLearned {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        encrypted_signature: Box::new(enc_sig?),
                        state3,
                    }
                }
                enc_sig = infallible_watch_for_encrypted_signature_via_hermes(&monero_wallet, &state3, monero_wallet_restore_blockheight) => {
                    tracing::info!("Received valid encrypted signature via Hermes");

                    AliceState::EncSigLearned {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        encrypted_signature: Box::new(enc_sig),
                        state3,
                    }
                }
                result = tx_lock_status.wait_until_confirmed_with(state3.cancel_timelock) => {
                    result?;
                    AliceState::CancelTimelockExpired {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        state3,
                    }
                }
            }
        }
        AliceState::XmrLockTransferProofSent {
            monero_wallet_restore_blockheight,
            transfer_proof,
            state3,
        } => {
            let tx_lock_status_subscription = bitcoin_wallet
                .subscribe_to(Box::new(state3.tx_lock.clone()))
                .await;

            select! {
                biased;
                result = tx_lock_status_subscription.wait_until_confirmed_with(state3.cancel_timelock) => {
                    result?;
                    AliceState::CancelTimelockExpired {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        state3,
                    }
                }
                enc_sig = event_loop_handle.recv_encrypted_signature() => {
                    tracing::info!("Received encrypted signature");

                    AliceState::EncSigLearned {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        encrypted_signature: Box::new(enc_sig?),
                        state3,
                    }
                }
                enc_sig = infallible_watch_for_encrypted_signature_via_hermes(&monero_wallet, &state3, monero_wallet_restore_blockheight) => {
                    tracing::info!("Received encrypted signature via Hermes");

                    AliceState::EncSigLearned {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        encrypted_signature: Box::new(enc_sig),
                        state3,
                    }
                }
                burn_instruction = event_loop_handle.wait_for_burn_on_refund_instruction() => {
                    let burn = burn_instruction.context("Failed to receive burn instruction")?;
                    let mut updated_state3 = (*state3).clone();
                    updated_state3.should_publish_tx_withhold = Some(burn);

                    AliceState::XmrLockTransferProofSent {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        state3: Box::new(updated_state3),
                    }
                }
            }
        }
        AliceState::EncSigLearned {
            monero_wallet_restore_blockheight,
            transfer_proof,
            encrypted_signature,
            state3,
        } => {
            // Try to sign the Bitcoin redeem transactions
            let tx_redeem = match state3.signed_redeem_transaction(*encrypted_signature) {
                Ok(tx_redeem) => tx_redeem,
                // If we cannot sign the transaction there must be something wrong
                // We just wait for the cancel timelock to expire and then refund
                Err(error) => {
                    tracing::error!(
                        "Failed to construct redeem transaction: {:#}, we will wait for the cancel timelock expiration to refund",
                        error
                    );

                    return Ok(AliceState::WaitingForCancelTimelockExpiration {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        state3,
                    });
                }
            };

            // Retry indefinitely to publish the redeem transaction, until the cancel timelock expires
            // Publishing the redeem transaction might fail on the first try due to any number of reasons
            let backoff = backoff::ExponentialBackoffBuilder::new()
                .with_max_elapsed_time(None)
                .with_max_interval(Duration::from_secs(60))
                .build();

            match backoff::future::retry_notify(backoff.clone(), || async {
                let tx_lock_status = bitcoin_wallet
                    .status_of_script(&state3.tx_lock.clone())
                    .await?;

                // If the cancel timelock is expired, it it not safe to publish the Bitcoin redeem transaction anymore
                //
                // TODO: In practice this should be redundant because the logic above will trigger for a superset of the cases where this is true
                if tx_lock_status.is_confirmed_with(state3.cancel_timelock) {
                    return Ok(None);
                }

                // We can only redeem the Bitcoin if we are fairly sure that our Bitcoin redeem transaction
                // will be confirmed before the cancel timelock expires
                //
                // We make an assumption that it will take at most `env_config.bitcoin_blocks_till_confirmed_upper_bound_assumption` blocks
                // until our transaction is included in a block. If this assumption is not satisfied, we will not publish the transaction.
                //
                // We will instead wait for the cancel timelock to expire and then refund.
                if tx_lock_status.blocks_left_until(state3.cancel_timelock) < env_config.bitcoin_blocks_till_confirmed_upper_bound_assumption {
                    return Ok(None);
                }

                bitcoin_wallet
                    .ensure_broadcasted(tx_redeem.clone(), "redeem")
                    .await
                    .map(Some)
                    .map_err(backoff::Error::transient)
            }, |e, wait_time: Duration| {
                tracing::warn!(
                    swap_id = %swap_id,
                    error = ?e,
                    "Failed to broadcast Bitcoin redeem transaction. We will retry in {} seconds",
                    wait_time.as_secs()
                )
            })
            .await
            .expect("We should never run out of retries while publishing the Bitcoin redeem transaction")
            {
                // We successfully published the redeem transaction
                // We wait until we see the transaction in the mempool before transitioning to the next state
                Some((txid, subscription)) => match subscription.wait_until_seen().await {
                    Ok(_) => AliceState::BtcRedeemTransactionPublished { state3, transfer_proof },
                    // TODO: No need to bail here, we should just retry?
                    Err(e) => {
                        // We extract the txid and the hex representation of the transaction
                        // this'll allow the user to manually re-publish the transaction
                        let tx_hex = serialize_hex(&tx_redeem);

                        bail!("Waiting for Bitcoin redeem transaction to be in mempool failed with {}! The redeem transaction was published, but it is not ensured that the transaction was included! You might be screwed. You can try to manually re-publish the transaction (TxID: {}, Tx Hex: {})", e, txid, tx_hex)
                    }
                },

                // It is not safe to publish the Bitcoin redeem transaction anymore
                // We wait for the cancel timelock to expire and then refund
                None => {
                    tracing::error!("We were unable to publish the Bitcoin redeem transaction before the timelock expired.");

                    AliceState::WaitingForCancelTimelockExpiration {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        state3,
                    }
                }
            }
        }
        AliceState::BtcRedeemTransactionPublished { state3, .. } => {
            let subscription = bitcoin_wallet
                .subscribe_to(Box::new(state3.tx_redeem()))
                .await;

            match subscription.wait_until_final().await {
                Ok(_) => AliceState::BtcRedeemed,
                Err(e) => {
                    bail!(
                        "The Bitcoin redeem transaction was seen in mempool, but waiting for finality timed out with {}. Manual investigation might be needed to ensure that the transaction was included.",
                        e
                    )
                }
            }
        }
        AliceState::WaitingForCancelTimelockExpiration {
            monero_wallet_restore_blockheight,
            transfer_proof,
            state3,
        } => {
            let tx_lock_status_subscription = bitcoin_wallet
                .subscribe_to(Box::new(state3.tx_lock.clone()))
                .await;

            select! {
                result = tx_lock_status_subscription.wait_until_confirmed_with(state3.cancel_timelock) => {
                    result?;
                    AliceState::CancelTimelockExpired {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        state3,
                    }
                }
                burn_instruction = event_loop_handle.wait_for_burn_on_refund_instruction() => {
                    let burn = burn_instruction.context("Failed to receive burn instruction")?;
                    let mut updated_state3 = (*state3).clone();
                    updated_state3.should_publish_tx_withhold = Some(burn);

                    AliceState::WaitingForCancelTimelockExpiration {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        state3: Box::new(updated_state3),
                    }
                }
            }
        }
        AliceState::CancelTimelockExpired {
            monero_wallet_restore_blockheight,
            transfer_proof,
            state3,
        } => {
            let backoff = backoff::ExponentialBackoffBuilder::new()
                .with_max_elapsed_time(None)
                // No need to be super aggressive here
                .with_max_interval(Duration::from_secs(60 * 10))
                .build();

            backoff::future::retry_notify::<_, anyhow::Error, _, _, _, _>(
                backoff,
                || async {
                    if state3
                        .check_for_tx_cancel(&*bitcoin_wallet)
                        .await
                        .context("Failed to check for existence of Bitcoin cancel transaction on chain")
                        .map_err(backoff::Error::transient)?
                        .is_some()
                    {
                        return Ok(());
                    }

                    state3
                        .submit_tx_cancel(&*bitcoin_wallet)
                        .await
                        .context("Failed to submit cancel transaction")
                        .map_err(backoff::Error::transient)?;

                    Ok(())
                },
                |e: anyhow::Error, wait_time: Duration| {
                    tracing::warn!(
                        swap_id = %swap_id,
                        error = ?e,
                        "Failed to ensure cancel transaction is published. We will retry in {} seconds",
                        wait_time.as_secs()
                    )
                },
            )
            .await
            .expect("We should never run out of retries while ensuring the cancel transaction is published");

            AliceState::BtcCancelled {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            }
        }
        AliceState::BtcCancelled {
            monero_wallet_restore_blockheight,
            transfer_proof,
            state3,
        } => {
            let tx_cancel_status = bitcoin_wallet
                .subscribe_to(Box::new(state3.tx_cancel()))
                .await;

            // We wait for either TxFullRefund or TxPartialRefund to be published
            // - both allow us to extract the Monero refund key.
            // Otherwise we punish, once that timelock expired.

            select! {
                spend_key = state3.watch_for_btc_tx_full_refund(&*bitcoin_wallet) => {
                    let spend_key = spend_key?;

                    AliceState::BtcRefunded {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        spend_key,
                        state3,
                    }
                }
                spend_key = state3.watch_for_btc_tx_partial_refund(&*bitcoin_wallet), if state3.btc_amnesty_amount.is_some() => {
                    let spend_key = spend_key?;

                    AliceState::BtcRefunded {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        spend_key,
                        state3,
                    }
                }
                result = tx_cancel_status.wait_until_confirmed_with(state3.punish_timelock) => {
                    result?;

                    AliceState::BtcPunishable {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        state3,
                    }
                }
                burn_instruction = event_loop_handle.wait_for_burn_on_refund_instruction() => {
                    let burn = burn_instruction.context("Failed to receive burn instruction")?;
                    let mut updated_state3 = (*state3).clone();
                    updated_state3.should_publish_tx_withhold = Some(burn);

                    tracing::info!(withhold=%burn, "Received withhold decision");

                    AliceState::BtcCancelled {
                        monero_wallet_restore_blockheight,
                        transfer_proof,
                        state3: Box::new(updated_state3),
                    }
                }
            }
        }
        AliceState::BtcRefunded {
            transfer_proof,
            spend_key,
            state3,
            monero_wallet_restore_blockheight,
        } => AliceState::XmrRefundable {
            monero_wallet_restore_blockheight,
            transfer_proof,
            spend_key,
            state3,
        },
        AliceState::BtcPartiallyRefunded {
            transfer_proof,
            spend_key,
            state3,
            monero_wallet_restore_blockheight,
        } => AliceState::XmrRefundable {
            monero_wallet_restore_blockheight,
            transfer_proof,
            spend_key,
            state3,
        },
        AliceState::XmrRefundable {
            monero_wallet_restore_blockheight: _,
            transfer_proof,
            spend_key,
            state3,
        } => {
            let xmr_refund_tx = retry(
                "Refund Monero",
                || async {
                    state3
                        .construct_xmr_refund_transaction(
                            monero_wallet.clone(),
                            swap_id,
                            spend_key,
                            transfer_proof.clone(),
                        )
                        .await
                        .map_err(backoff::Error::transient)
                },
                None,
                Duration::from_secs(60),
            )
            .await
            .expect("We should never run out of retries while refunding Monero");

            AliceState::XmrRefundTxConstructed {
                state3,
                xmr_refund_tx,
            }
        }
        AliceState::XmrRefundTxConstructed {
            state3,
            xmr_refund_tx,
        } => {
            let xmr_refund_tx_hash = monero::TxHash::from_tx(&xmr_refund_tx);

            retry(
                "Publishing Monero refund transaction",
                || async {
                    let is_present = monero_wallet
                        .is_transaction_present(&xmr_refund_tx_hash)
                        .await
                        .context("Failed to check whether Monero refund transaction is already present on chain")
                        .map_err(backoff::Error::transient)?;

                    if is_present {
                        tracing::info!(%swap_id, %xmr_refund_tx_hash, "Monero refund transaction is already present on chain, skipping publish");
                        return Ok(());
                    }

                    monero_wallet
                        .rpc_client()
                        .await
                        .map_err(backoff::Error::transient)?
                        .publish_transaction(&xmr_refund_tx)
                        .await
                        .context("Failed to publish Monero refund transaction")
                        .map_err(backoff::Error::transient)
                },
                None,
                None,
            )
            .await
            .context("Failed to publish Monero refund transaction")?;

            tracing::info!(%swap_id, %xmr_refund_tx_hash, "Published Monero refund transaction");

            AliceState::XmrRefundTxPublished {
                state3,
                xmr_refund_tx,
            }
        }
        AliceState::XmrRefundTxPublished {
            state3,
            xmr_refund_tx,
        } => {
            let xmr_refund_tx_hash = monero::TxHash::from_tx(&xmr_refund_tx);

            monero_wallet
                .wait_until_confirmed(
                    &xmr_refund_tx_hash,
                    1,
                    None::<fn((monero::TxHash, u64, u64))>,
                )
                .await
                .context("Failed to wait for Monero refund transaction confirmation")?;

            AliceState::XmrRefunded {
                state3: Some(state3),
            }
        }
        AliceState::BtcPunishable {
            monero_wallet_restore_blockheight,
            transfer_proof,
            state3,
        } => {
            retry(
                "Punish Bitcoin",
                || async {
                    // Before punishing, we explicitly check for the refund transaction as we prefer refunds over punishments
                    let spend_key_from_btc_refund = state3.refund_btc(&*bitcoin_wallet).await.context("Failed to check for existence of Bitcoin refund transaction before punishing").map_err(backoff::Error::transient)?;

                    // If we find the Bitcoin refund transaction, we go ahead and refund the Monero
                    if let Some(spend_key_from_btc_refund) = spend_key_from_btc_refund {
                        return Ok::<AliceState, backoff::Error<anyhow::Error>>(AliceState::BtcRefunded {
                            monero_wallet_restore_blockheight,
                            transfer_proof: transfer_proof.clone(),
                            spend_key: spend_key_from_btc_refund,
                            state3: state3.clone(),
                        });
                    }

                    state3.punish_btc(&*bitcoin_wallet).await.context("Failed to construct and publish Bitcoin punish transaction").map_err(backoff::Error::transient)?;

                    Ok::<AliceState, backoff::Error<anyhow::Error>>(AliceState::BtcPunished {
                        state3: state3.clone(),
                        transfer_proof: transfer_proof.clone(),
                    })
                },
                None,
                // We can take our time when punishing
                Duration::from_secs(60 * 5),
            )
            .await
            .expect("We should never run out of retries while publishing the punish transaction")
        }
        AliceState::XmrRefunded { state3 } => {
            // Only publish TxWithhold for swaps which have an anti-spam deposit.
            let Some(mut state3) = state3 else {
                tracing::info!(
                    "Running a pre-partial refund swap, there is no anti-spam deposit to withhold"
                );
                return Ok(AliceState::XmrRefunded { state3: None });
            };

            // Fetch the burn decision again, incase it was updated via the controller
            if let Some(burn_decision) = event_loop_handle.get_burn_on_refund_instruction().await {
                state3.should_publish_tx_withhold = Some(burn_decision);
            }

            // Skip publishing TxWithhold unless we were specifically instructed
            if !state3.should_publish_tx_withhold.unwrap_or(false) {
                tracing::info!("Not instructed to withhold the anti-spam deposit. Finishing");
                return Ok(AliceState::XmrRefunded {
                    state3: Some(state3),
                });
            }

            retry("Publish TxWithhold", || {
                let state3 = state3.clone();
                let bitcoin_wallet = bitcoin_wallet.clone();

                async move {
                    let signed_tx = state3.signed_withhold_transaction()
                        .context("Can't withhold the anti-spam deposit after Bob refunded because we couldn't construct the transaction")
                        .map_err(backoff::Error::transient)?;

                    bitcoin_wallet
                        .ensure_broadcasted(signed_tx, "withhold")
                        .await
                        .context("Couldn't publish TxWithhold")
                        .map_err(backoff::Error::transient)?;

                    Ok(AliceState::BtcWithholdPublished { state3 })
                }
            }, None, None).await?
        }
        AliceState::BtcWithholdPublished { state3 } => {
            retry(
                "Wait for TxWithhold confirmation",
                || {
                    let state3 = state3.clone();
                    let bitcoin_wallet = bitcoin_wallet.clone();

                    async move {
                        let tx_withhold = state3
                            .tx_withhold()
                            .context("Can't construct TxWithhold even though we published it")
                            .map_err(backoff::Error::transient)?;

                        let subscription = bitcoin_wallet.subscribe_to(Box::new(tx_withhold)).await;

                        subscription
                            .wait_until_final()
                            .await
                            .context("Failed to wait for TxWithhold to be confirmed")
                            .map_err(backoff::Error::transient)?;

                        Ok(AliceState::BtcWithholdConfirmed { state3 })
                    }
                },
                None,
                None,
            )
            .await?
        }
        AliceState::BtcWithholdConfirmed { state3 } => {
            // Nothing to do here. Mercy is triggered manually.
            AliceState::BtcWithholdConfirmed { state3 }
        }
        AliceState::BtcMercyGranted { state3 } => {
            retry(
                "Publish TxMercy",
                || {
                    let state3 = state3.clone();
                    let bitcoin_wallet = bitcoin_wallet.clone();

                    async move {
                        let signed_tx = state3
                            .signed_mercy_transaction()
                            .context("Failed to construct signed TxMercy")
                            .map_err(backoff::Error::transient)?;

                        bitcoin_wallet
                            .ensure_broadcasted(signed_tx, "mercy")
                            .await
                            .context("Failed to publish TxMercy")
                            .map_err(backoff::Error::transient)?;

                        tracing::info!("TxMercy published successfully");

                        Ok(AliceState::BtcMercyPublished { state3 })
                    }
                },
                None,
                None,
            )
            .await?
        }
        AliceState::BtcMercyPublished { state3 } => {
            retry(
                "Wait for TxMercy confirmation",
                || {
                    let state3 = state3.clone();
                    let bitcoin_wallet = bitcoin_wallet.clone();

                    async move {
                        let tx_mercy = state3
                            .tx_mercy()
                            .context("Couldn't construct TxMercy even though we have published it")
                            .map_err(backoff::Error::transient)?;

                        let subscription = bitcoin_wallet.subscribe_to(Box::new(tx_mercy)).await;

                        subscription
                            .wait_until_final()
                            .await
                            .context("Failed to wait for TxMercy to be confirmed")
                            .map_err(backoff::Error::transient)?;

                        Ok(AliceState::BtcMercyConfirmed { state3 })
                    }
                },
                None,
                None,
            )
            .await?
        }
        AliceState::BtcMercyConfirmed { state3 } => AliceState::BtcMercyConfirmed { state3 },
        AliceState::BtcRedeemed => AliceState::BtcRedeemed,
        AliceState::BtcPunished {
            state3,
            transfer_proof,
        } => AliceState::BtcPunished {
            state3,
            transfer_proof,
        },
        AliceState::BtcEarlyRefunded(state3) => AliceState::BtcEarlyRefunded(state3),
        AliceState::SafelyAborted => AliceState::SafelyAborted,
    })
}

#[allow(async_fn_in_trait)]
pub trait XmrRefundable {
    async fn construct_xmr_refund_transaction(
        &self,
        monero_wallet: Arc<monero::Wallets>,
        swap_id: Uuid,
        spend_key: monero::PrivateKey,
        transfer_proof: TransferProof,
    ) -> Result<Transaction<NotPruned>>;
}

impl XmrRefundable for State3 {
    async fn construct_xmr_refund_transaction(
        &self,
        monero_wallet: Arc<monero::Wallets>,
        swap_id: Uuid,
        spend_key: monero::PrivateKey,
        transfer_proof: TransferProof,
    ) -> Result<Transaction<NotPruned>> {
        let view_key = self.v;

        // Ensure that the XMR to be refunded are spendable by awaiting 10 confirmations
        // on the lock transaction.
        tracing::info!("Waiting for Monero lock transaction to be confirmed before refunding");

        monero_wallet
            .wait_until_confirmed(
                &transfer_proof.tx_hash(),
                10,
                Some(
                    move |(xmr_lock_txid, confirmations, target_confirmations)| {
                        tracing::debug!(
                            %xmr_lock_txid,
                            %confirmations,
                            %target_confirmations,
                            "Monero lock transaction got a confirmation"
                        );
                    },
                ),
            )
            .await
            .context("Failed to wait for Monero lock transaction to be confirmed")?;

        let main_address = monero_wallet.main_wallet().await.main_address().await?;

        tracing::debug!(%swap_id, %main_address, "Sweeping lock output to redeem address");

        let tx = monero_wallet
            .construct_sweep_to_single(
                &transfer_proof.tx_hash(),
                spend_key,
                view_key,
                main_address,
                None,
            )
            .await
            .context("Failed to construct Monero refund transaction")?;

        tracing::info!(%swap_id, tx_hash = %monero::TxHash::from_tx(&tx), "Constructed Monero refund transaction");

        Ok(tx)
    }
}

impl XmrRefundable for Box<State3> {
    async fn construct_xmr_refund_transaction(
        &self,
        monero_wallet: Arc<monero::Wallets>,
        swap_id: Uuid,
        spend_key: monero::PrivateKey,
        transfer_proof: TransferProof,
    ) -> Result<Transaction<NotPruned>> {
        (**self)
            .construct_xmr_refund_transaction(monero_wallet, swap_id, spend_key, transfer_proof)
            .await
    }
}

/// Watch the Hermes wallet for the encrypted signature Bob transmits on-chain.
/// Retries indefinitely on transient errors.
async fn infallible_watch_for_encrypted_signature_via_hermes(
    monero_wallet: &monero::Wallets,
    state3: &State3,
    monero_wallet_restore_blockheight: BlockHeight,
) -> swap_core::bitcoin::EncryptedSignature {
    retry(
        "Watching for the encrypted signature via Hermes",
        || async {
            monero_wallet
                .wait_for_hermes_message(
                    state3.hermes_wallet_public_spend_key(),
                    state3.v,
                    monero_wallet_restore_blockheight,
                    |message| {
                        let enc_sig = crate::protocol::hermes::decode_encrypted_signature(message)
                            .context("Failed to decode the encrypted signature")?;

                        if !state3.verify_tx_redeem_encsig(&enc_sig) {
                            anyhow::bail!("Encrypted signature does not verify against tx_redeem");
                        }

                        Ok(enc_sig)
                    },
                )
                .await
                .context("Failed to wait for the encrypted signature via Hermes")
                .map_err(backoff::Error::transient)
        },
        None,
        Duration::from_secs(60),
    )
    .await
    .expect("we never stop retrying to watch for the encrypted signature via Hermes")
}

/// Build transfer destinations for the Monero lock transaction: the lock
/// output, optionally a developer tip, and the Hermes funding output which Bob
/// sweeps to transmit the encrypted signature on-chain.
///
/// The tip output is only included if tip.ratio > 0 and the effective tip is
/// >= MIN_USEFUL_TIP_AMOUNT_PICONERO.
fn build_transfer_destinations(
    lock_address: monero_address::MoneroAddress,
    lock_amount: monero_oxide_ext::Amount,
    hermes_funding: (monero_address::MoneroAddress, monero_oxide_ext::Amount),
    tip: TipConfig,
) -> anyhow::Result<Vec<(monero_address::MoneroAddress, monero_oxide_ext::Amount)>> {
    use rust_decimal::prelude::ToPrimitive;

    // If the effective tip is less than this amount, we do not include the tip output
    // Any values below `MIN_USEFUL_TIP_AMOUNT_PICONERO` are clamped to zero
    //
    // At $300/XMR, this is around one cent
    const MIN_USEFUL_TIP_AMOUNT_PICONERO: u64 = 30_000_000;

    // TODO: Move this code into the impl of TipConfig
    let tip_amount_piconero = tip
        .ratio
        .saturating_mul(Decimal::from(lock_amount.as_pico()))
        .floor()
        .to_u64()
        .context("Developer tip amount should not overflow")?;

    let mut destinations = vec![(lock_address, lock_amount)];

    if tip_amount_piconero >= MIN_USEFUL_TIP_AMOUNT_PICONERO {
        let tip_amount = monero_oxide_ext::Amount::from_pico(tip_amount_piconero);
        destinations.push((tip.address, tip_amount));
    }

    // A zero Hermes funding disables the on-chain encrypted signature channel
    if hermes_funding.1.as_pico() > 0 {
        destinations.push(hermes_funding);
    }

    Ok(destinations)
}

/// This function is used to check if Alice is in a state where it is clear that she has already received the encrypted signature from Bob.
/// This allows us to acknowledge the encrypted signature multiple times
/// If our acknowledgement does not reach Bob, he might send the encrypted signature again.
pub(crate) fn has_already_processed_enc_sig(state: &AliceState) -> bool {
    matches!(
        state,
        AliceState::EncSigLearned { .. }
            | AliceState::BtcRedeemTransactionPublished { .. }
            | AliceState::BtcRedeemed
    )
}

async fn cancel_timelock_not_expired(
    state3: &State3,
    bitcoin_wallet: &dyn BitcoinWallet,
) -> Result<bool> {
    Ok(matches!(
        state3.expired_timelocks(bitcoin_wallet).await?,
        ExpiredTimelocks::None { .. }
    ))
}

/// Whether `state` is inside the serialized Monero lock phase (see [`MONERO_LOCK_PHASE`]).
fn in_monero_lock_phase(state: &AliceState) -> bool {
    matches!(
        state,
        AliceState::BtcLocked { .. } | AliceState::XmrLockTransactionConstructed { .. }
    )
}

/// Releases [`MONERO_LOCK_PHASE`] after a swap overstays its deadline while still holding a
/// constructed lock transaction. If that transaction already reached the chain, scan it so
/// wallet2 marks its outputs spent before the next swap constructs; otherwise the
/// unserialized race returns for this one wedged swap. Bounded so an unresponsive daemon
/// cannot extend the hold.
async fn abandon_lock_phase(state: &AliceState, monero_wallet: &monero::Wallets) {
    let AliceState::XmrLockTransactionConstructed { xmr_lock_tx, .. } = state else {
        tracing::warn!("Monero lock phase exceeded its deadline; releasing the lock");
        return;
    };

    let tx_hash = monero::TxHash::from_tx(xmr_lock_tx);
    tracing::warn!(%tx_hash, "Monero lock phase exceeded its deadline; releasing the lock");

    let scanned = timeout(Duration::from_secs(60), async {
        if monero_wallet.is_transaction_present(&tx_hash).await? {
            monero_wallet
                .main_wallet()
                .await
                .scan_transaction(tx_hash.0.clone())
                .await?;
        }
        anyhow::Ok(())
    })
    .await;

    match scanned {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(%tx_hash, %error, "Failed to scan the lock transaction on release")
        }
        Err(_) => tracing::warn!(%tx_hash, "Timed out scanning the lock transaction on release"),
    }
}

#[cfg(test)]
mod tests {
    use super::build_transfer_destinations;
    use crate::protocol::alice::TipConfig;
    use rust_decimal::Decimal;

    const TEST_ADDRESS_STR: &str = "53gEuGZUhP9JMEBZoGaFNzhwEgiG7hwQdMCqFxiyiTeFPmkbt1mAoNybEUvYBKHcnrSgxnVWgZsTvRBaHBNXPa8tHiCU51a";

    fn test_address() -> monero_address::MoneroAddress {
        monero_address::MoneroAddress::from_str_with_unchecked_network(TEST_ADDRESS_STR).unwrap()
    }

    fn test_hermes_funding() -> (monero_address::MoneroAddress, monero_oxide_ext::Amount) {
        (
            test_address(),
            monero_oxide_ext::Amount::from_pico(20_000_000_000),
        )
    }

    #[test]
    fn test_build_transfer_destinations_without_tip() {
        let lock_amount = monero_oxide_ext::Amount::from_pico(1_000_000_000_000); // 1 XMR
        let tip = TipConfig {
            ratio: Decimal::ZERO,
            address: test_address(),
        };

        let result =
            build_transfer_destinations(test_address(), lock_amount, test_hermes_funding(), tip)
                .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].1, lock_amount);
        assert_eq!(*result.last().unwrap(), test_hermes_funding());
    }

    #[test]
    fn test_build_transfer_destinations_omits_zero_hermes_funding() {
        let lock_amount = monero_oxide_ext::Amount::from_pico(1_000_000_000_000); // 1 XMR
        let tip = TipConfig {
            ratio: Decimal::ZERO,
            address: test_address(),
        };
        let hermes_funding = (test_address(), monero_oxide_ext::Amount::ZERO);

        let result =
            build_transfer_destinations(test_address(), lock_amount, hermes_funding, tip).unwrap();

        assert_eq!(result, vec![(test_address(), lock_amount)]);
    }

    #[test]
    fn test_build_transfer_destinations_with_tip() {
        let lock_amount = monero_oxide_ext::Amount::from_pico(10_000_000_000_000); // 10 XMR
        let tip = TipConfig {
            ratio: Decimal::new(1, 2), // 0.01 = 1%
            address: test_address(),
        };

        let result =
            build_transfer_destinations(test_address(), lock_amount, test_hermes_funding(), tip)
                .unwrap();

        // Tip = 10 XMR * 0.01 = 0.1 XMR = 100_000_000_000 pico >> 30_000_000 threshold
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].1, lock_amount);
        assert_eq!(
            result[1].1,
            monero_oxide_ext::Amount::from_pico(100_000_000_000)
        );
        assert_eq!(*result.last().unwrap(), test_hermes_funding());
    }

    #[test]
    fn test_build_transfer_destinations_with_small_tip() {
        // ratio * amount < 30_000_000 piconero threshold
        let lock_amount = monero_oxide_ext::Amount::from_pico(2_000_000_000); // 0.002 XMR
        let tip = TipConfig {
            ratio: Decimal::new(1, 2), // 0.01
            address: test_address(),
        };

        let result =
            build_transfer_destinations(test_address(), lock_amount, test_hermes_funding(), tip)
                .unwrap();

        // Tip = 0.002 XMR * 0.01 = 20_000_000 piconero < 30_000_000 threshold
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].1, lock_amount);
        assert_eq!(*result.last().unwrap(), test_hermes_funding());
    }

    #[test]
    fn test_build_transfer_destinations_with_zero_tip() {
        // Nonzero ratio but tiny lock amount -> effective tip rounds to near-zero
        let lock_amount = monero_oxide_ext::Amount::from_pico(100);
        let tip = TipConfig {
            ratio: Decimal::new(1, 1), // 0.1 = 10%
            address: test_address(),
        };

        let result =
            build_transfer_destinations(test_address(), lock_amount, test_hermes_funding(), tip)
                .unwrap();

        // Tip = 100 * 0.1 = 10 piconero << 30_000_000 threshold
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].1, lock_amount);
        assert_eq!(*result.last().unwrap(), test_hermes_funding());
    }

    #[test]
    fn test_build_transfer_destinations_with_fractional_tip() {
        let lock_amount = monero_oxide_ext::Amount::from_pico(1_000_000_000_000); // 1 XMR
        let tip = TipConfig {
            ratio: Decimal::new(5, 3), // 0.005 = 0.5%
            address: test_address(),
        };

        let result =
            build_transfer_destinations(test_address(), lock_amount, test_hermes_funding(), tip)
                .unwrap();

        // Tip = 1 XMR * 0.005 = 0.005 XMR = 5_000_000_000 pico >> 30_000_000 threshold
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].1, lock_amount);
        assert_eq!(
            result[1].1,
            monero_oxide_ext::Amount::from_pico(5_000_000_000)
        );
        assert_eq!(*result.last().unwrap(), test_hermes_funding());
    }
}
