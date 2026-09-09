pub mod harness;

use anyhow::bail;
use harness::SlowCancelConfig;
use swap::asb::FixedRate;
use swap::protocol::alice::AliceState;
use swap::protocol::{alice, bob};

/// Without a trusted daemon, the ASB must NEVER rebuild the Monero lock
/// transaction, even if the daemon reports the lock transaction's inputs as
/// spent by a confirmed transaction. A malicious daemon could otherwise trick
/// the ASB into abandoning a valid lock transaction (or grief it into
/// rebuilding indefinitely).
///
/// The setup is identical to
/// `alice_rebuilds_xmr_lock_after_confirmed_double_spend` — the only difference
/// is the missing `trusted_daemon` flag. Instead of rebuilding, Alice must keep
/// trying to publish the original lock transaction until the cancel timelock
/// expires.
#[tokio::test]
async fn alice_does_not_rebuild_xmr_lock_with_untrusted_daemon() {
    harness::setup_test(SlowCancelConfig, None, None, |mut ctx| async move {
        let (bob_swap, _bob_event_loop) = ctx.bob_swap().await;
        let _bob_handle = tokio::spawn(bob::run(bob_swap));

        // Run Alice until she constructed (but not yet published) the XMR lock
        // transaction
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

        // Simulate a confirmed double spend: sweep all of Alice's outputs
        // (including the lock transaction's inputs) to a burn address
        ctx.sweep_alice_monero_wallet_to_burn().await;

        harness::wait_until("lock transaction inputs spent in blockchain", || async {
            ctx.alice_monero_wallet
                .has_input_confirmed_spent(xmr_lock_tx)
                .await
        })
        .await?;

        ctx.restart_alice().await;
        let alice_swap = ctx.alice_next_swap().await;

        // `run_until` stops at the first matching state: if Alice ever rebuilds
        // (XmrReadyToLock), the assertion below fails. With an untrusted daemon she
        // must ignore the reported double spend and keep trying to publish the
        // original lock transaction until the cancel timelock expires.
        let alice_state = alice::run_until(
            alice_swap,
            |state| {
                matches!(
                    state,
                    AliceState::WaitingForCancelTimelockExpiration { .. }
                        | AliceState::XmrReadyToLock { .. }
                )
            },
            FixedRate::default(),
        )
        .await?;

        assert!(
            matches!(
                alice_state,
                AliceState::WaitingForCancelTimelockExpiration { .. }
            ),
            "Alice must not rebuild the lock transaction with an untrusted daemon, got {alice_state}"
        );

        Ok(())
    })
    .await;
}
