pub mod harness;

use anyhow::bail;
use harness::TrustedDaemonLongCancelConfig;
use swap::asb::FixedRate;
use swap::monero;
use swap::protocol::alice::AliceState;
use swap::protocol::{alice, bob};

/// With a trusted daemon, the ASB must rebuild the Monero lock transaction when
/// the daemon reports that the lock transaction's inputs were already spent by
/// a confirmed transaction while the lock transaction itself is unknown to the
/// daemon. The rebuilt lock transaction must allow the swap to complete
/// normally.
///
/// The double spend is simulated by sweeping Alice's whole wallet (including
/// the lock transaction's inputs) to a burn address after she constructed but
/// before she published the lock transaction.
#[tokio::test]
async fn alice_rebuilds_xmr_lock_after_confirmed_double_spend() {
    harness::setup_test(
        TrustedDaemonLongCancelConfig,
        None,
        None,
        |mut ctx| async move {
            let (bob_swap, _bob_event_loop) = ctx.bob_swap().await;
            let bob_handle = tokio::spawn(bob::run(bob_swap));

            // Run Alice until she constructed (but not yet published) the XMR
            // lock transaction
            let alice_swap = ctx.alice_next_swap().await;
            let alice_state = alice::run_until(
                alice_swap,
                |state| matches!(state, AliceState::XmrLockTransactionConstructed { .. }),
                FixedRate::default(),
            )
            .await?;

            let AliceState::XmrLockTransactionConstructed {
                xmr_lock_tx,
                monero_wallet_restore_blockheight: original_restore_height,
                ..
            } = &alice_state
            else {
                bail!("Expected XmrLockTransactionConstructed, got {alice_state}");
            };

            // Simulate a confirmed double spend: sweep all of Alice's outputs
            // (including the lock transaction's inputs) to a burn address
            let conflicting_spend = ctx.sweep_alice_monero_wallet_to_burn().await;

            harness::wait_until("lock transaction inputs spent in blockchain", || async {
                ctx.alice_monero_wallet
                    .has_input_confirmed_spent(xmr_lock_tx)
                    .await
            })
            .await?;

            // The lock transaction itself must still be unknown to the daemon,
            // otherwise there is nothing to rebuild
            assert!(
                !ctx.alice_monero_wallet
                    .is_transaction_present(&monero::TxHash::from_tx(xmr_lock_tx))
                    .await?,
                "Lock transaction must not be known to the daemon"
            );

            // Re-fund Alice so she can construct a new lock transaction
            ctx.fund_alice_monero_wallet(vec![ctx.xmr_amount(); 10])
                .await;

            // Resume Alice: she must detect the confirmed double spend, rebuild
            // the lock transaction from scratch and complete the swap. The
            // original lock transaction can never confirm (its inputs are
            // spent), so a completed swap proves the rebuild happened.
            ctx.restart_alice().await;
            let alice_swap = ctx.alice_next_swap().await;
            let rebuilt_state = alice::run_until(
                alice_swap,
                |state| {
                    matches!(
                        state,
                        AliceState::XmrReadyToLock { .. } | AliceState::BtcLocked { .. }
                    )
                },
                FixedRate::default(),
            )
            .await?;
            let AliceState::XmrReadyToLock {
                monero_wallet_restore_blockheight,
                ..
            } = rebuilt_state
            else {
                bail!("Expected rebuild to preserve restore height via XmrReadyToLock, got {rebuilt_state}");
            };
            assert_eq!(monero_wallet_restore_blockheight, *original_restore_height);

            // Independently check the real conflicting transaction's depth at rebuild time.
            use monero_wallet_ng::rpc::{ProvidesTransactionStatus, TransactionStatus};
            let daemon = ctx.alice_monero_wallet.rpc_client().await?;
            let mut conflict_hash = [0; 32];
            hex::decode_to_slice(conflicting_spend.txid, &mut conflict_hash)?;
            let TransactionStatus::InBlock { block_height } = daemon
                .transaction_status(conflict_hash)
                .await?
            else {
                bail!("Conflicting spend must be confirmed before rebuilding");
            };
            let tip = ctx.alice_monero_wallet.direct_rpc_block_height().await?;
            assert!(tip >= block_height + 9, "Rebuild requires ten confirmations of the conflicting spend");

            ctx.restart_alice().await;
            let alice_swap = ctx.alice_next_swap().await;
            let alice_state = alice::run(alice_swap, FixedRate::default()).await?;

            let bob_state = bob_handle.await??;

            ctx.assert_alice_redeemed(alice_state).await;
            ctx.assert_bob_redeemed(bob_state).await;

            Ok(())
        },
    )
    .await;
}
