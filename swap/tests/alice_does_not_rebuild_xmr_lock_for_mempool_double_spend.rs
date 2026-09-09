pub mod harness;

use anyhow::bail;
use harness::TrustedDaemonLongCancelConfig;
use std::time::Duration;
use swap::asb::FixedRate;
use swap::monero;
use swap::protocol::alice::AliceState;
use swap::protocol::{alice, bob};

/// A double spend that only exists in the mempool must NEVER trigger a rebuild
/// of the Monero lock transaction — the conflicting transaction may still be
/// displaced by our own. Only a double spend confirmed in the blockchain may
/// trigger a rebuild.
///
/// The miner is stopped so the double-spending sweep transaction stays in the
/// mempool while Alice resumes. After verifying she does not rebuild, the miner
/// is restarted: once the double spend confirms, rebuilding becomes the correct
/// decision and the swap must complete.
#[tokio::test]
async fn alice_does_not_rebuild_xmr_lock_for_mempool_double_spend() {
    harness::setup_test(
        TrustedDaemonLongCancelConfig,
        None,
        None,
        |mut ctx| async move {
            // Freeze the Monero blockchain so the double spend cannot confirm
            ctx.monero.stop_miner().await;

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

            let AliceState::XmrLockTransactionConstructed { xmr_lock_tx, .. } = &alice_state
            else {
                bail!("Expected XmrLockTransactionConstructed, got {alice_state}");
            };

            // Double spend the lock transaction's inputs, but only into the
            // mempool (the miner is stopped)
            let sweep_receipt = ctx.sweep_alice_monero_wallet_to_burn().await;

            harness::wait_until("sweep transaction known to the daemon", || async {
                ctx.alice_monero_wallet
                    .is_transaction_present(&monero::TxHash(sweep_receipt.txid.clone()))
                    .await
            })
            .await?;

            assert!(
                !ctx.alice_monero_wallet
                    .has_input_confirmed_spent(xmr_lock_tx)
                    .await?,
                "A mempool double spend must not count as a confirmed spend"
            );

            ctx.restart_alice().await;
            let alice_swap = ctx.alice_next_swap().await;

            // While the double spend is unconfirmed, Alice must not rebuild.
            // Her attempts to publish the original lock transaction are
            // rejected by the daemon (pool double spend), so she keeps retrying
            // in XmrLockTransactionConstructed.
            let rebuild = tokio::time::timeout(
                Duration::from_secs(45),
                alice::run_until(
                    alice_swap,
                    |state| matches!(state, AliceState::XmrReadyToLock { .. }),
                    FixedRate::default(),
                ),
            )
            .await;
            assert!(
                rebuild.is_err(),
                "Alice rebuilt the lock transaction although the double spend was only in the mempool"
            );

            // Confirm the double spend: now rebuilding is the correct decision
            ctx.monero.start_miner().await?;

            harness::wait_until("lock transaction inputs spent in blockchain", || async {
                ctx.alice_monero_wallet
                    .has_input_confirmed_spent(xmr_lock_tx)
                    .await
            })
            .await?;

            // Re-fund Alice so she can construct a new lock transaction
            ctx.fund_alice_monero_wallet(vec![ctx.xmr_amount(); 10]).await;

            // Resume Alice: she must now rebuild and complete the swap
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
