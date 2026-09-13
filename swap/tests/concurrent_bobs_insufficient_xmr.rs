pub mod harness;

use harness::SlowCancelConfig;
use swap::asb::FixedRate;
use swap::protocol::alice::AliceState;
use swap::protocol::bob::BobState;
use swap::protocol::{alice, bob};

/// Two swaps run at once but the maker only holds enough Monero to fund ONE of
/// them (two 1-XMR outputs, each swap needs ~1.02 XMR). The serialized lock
/// phase must let the winner lock and make the loser fail CLEANLY at construct
/// time (an early Bitcoin refund) instead of building a lock that double-spends
/// the winner's inputs and then wedging on a permanently rejected publish.
#[tokio::test]
async fn concurrent_bobs_insufficient_xmr() {
    harness::setup_test_funded(SlowCancelConfig, None, None, 2, |mut ctx| async move {
        let (bob_swap_1, bob_join_handle_1) = ctx.bob_swap().await;
        let bob_swap_1 = tokio::spawn(bob::run(bob_swap_1));
        let alice_swap_1 = ctx.alice_next_swap().await;
        let alice_swap_1 = tokio::spawn(alice::run(alice_swap_1, FixedRate::default()));

        let (bob_swap_2, bob_join_handle_2) = ctx.bob_swap().await;
        let bob_swap_2 = tokio::spawn(bob::run(bob_swap_2));
        let alice_swap_2 = ctx.alice_next_swap().await;
        let alice_swap_2 = tokio::spawn(alice::run(alice_swap_2, FixedRate::default()));

        let bob_state_1 = bob_swap_1.await??;
        let bob_state_2 = bob_swap_2.await??;
        let alice_state_1 = alice_swap_1.await??;
        let alice_state_2 = alice_swap_2.await??;

        bob_join_handle_1.abort();
        bob_join_handle_2.abort();

        let alices = [&alice_state_1, &alice_state_2];
        let redeemed = alices
            .iter()
            .filter(|s| matches!(s, AliceState::BtcRedeemed))
            .count();
        // The un-fundable swap must refund cleanly. An early refund (construct
        // failed, no Monero was locked) is the intended outcome; a plain refund
        // is also acceptable, but the swap must reach a terminal refund state
        // rather than wedge on a rejected double-spend publish.
        let refunded = alices
            .iter()
            .filter(|s| {
                matches!(
                    s,
                    AliceState::BtcEarlyRefunded(_)
                        | AliceState::SafelyAborted
                        | AliceState::XmrRefunded { .. }
                )
            })
            .count();
        assert_eq!(redeemed, 1, "exactly one maker swap should lock and redeem");
        assert_eq!(
            refunded, 1,
            "the un-fundable maker swap must refund cleanly, not wedge; got {alice_state_1} / {alice_state_2}"
        );

        let bobs = [&bob_state_1, &bob_state_2];
        assert_eq!(
            bobs.iter()
                .filter(|s| matches!(s, BobState::XmrRedeemed { .. }))
                .count(),
            1,
            "one taker should redeem XMR"
        );
        assert_eq!(
            bobs.iter()
                .filter(|s| {
                    matches!(
                        s,
                        BobState::BtcEarlyRefunded { .. }
                            | BobState::BtcEarlyRefundPublished { .. }
                            | BobState::BtcRefunded { .. }
                    )
                })
                .count(),
            1,
            "the other taker should get its Bitcoin refunded (early refund, since the maker never locked Monero)"
        );

        Ok(())
    })
    .await;
}
