pub mod harness;

use anyhow::bail;
use harness::TrustedDaemonConfig;
use swap::asb::FixedRate;
use swap::monero;
use swap::protocol::alice::AliceState;
use swap::protocol::{alice, bob};

/// Even with a trusted daemon and a confirmed double spend of the lock
/// transaction's inputs, the ASB must NOT rebuild the lock transaction if the
/// shared swap wallet (the lock address) has received outputs.
///
/// This is an anti-griefing guard: the lock address becomes known to Bob once
/// he receives the transfer proof, so anyone could send funds to it. If Alice
/// rebuilt the lock transaction while the shared wallet is not empty, she would
/// lock fresh XMR into a wallet whose balance no longer matches the agreed swap
/// amount.
#[tokio::test]
async fn alice_does_not_rebuild_xmr_lock_when_shared_wallet_not_empty() {
    harness::setup_test(TrustedDaemonConfig, None, None, |mut ctx| async move {
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

        let AliceState::XmrLockTransactionConstructed {
            xmr_lock_tx,
            state3,
            monero_wallet_restore_blockheight,
            ..
        } = &alice_state
        else {
            bail!("Expected XmrLockTransactionConstructed, got {alice_state}");
        };

        // Simulate a confirmed double spend: sweep all of Alice's outputs
        // (including the lock transaction's inputs) to a burn address
        ctx.sweep_alice_monero_wallet_to_burn().await;

        // Poison the shared lock wallet with dust. The regtest harness uses
        // mainnet address encoding (see `init_test_wallets`).
        let transfer_request = state3.lock_xmr_transfer_request();
        let (lock_address, _) = transfer_request.address_and_amount(monero::Network::Mainnet);
        ctx.monero
            .wallet("miner")?
            .transfer(&lock_address, 1_000_000)
            .await?;

        // Wait until the double spend is confirmed AND the dust is visible to
        // the exact emptiness check Alice will perform, so her decision is
        // deterministic
        harness::wait_until("lock transaction inputs spent in blockchain", || async {
            ctx.alice_monero_wallet
                .has_input_confirmed_spent(xmr_lock_tx)
                .await
        })
        .await?;

        harness::wait_until("shared lock wallet received outputs", || async {
            ctx.alice_monero_wallet
                .has_received_outputs(
                    transfer_request.public_spend_key,
                    state3.v,
                    *monero_wallet_restore_blockheight,
                    None,
                    None,
                )
                .await
        })
        .await?;

        ctx.restart_alice().await;
        let alice_swap = ctx.alice_next_swap().await;

        // `run_until` stops at the first matching state: if Alice ever rebuilds
        // (XmrReadyToLock), the assertion below fails. Because the shared lock
        // wallet is not empty she must not rebuild, and instead wait out the
        // cancel timelock.
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
            "Alice must not rebuild the lock transaction when the shared lock wallet received outputs, got {alice_state}"
        );

        Ok(())
    })
    .await;
}
