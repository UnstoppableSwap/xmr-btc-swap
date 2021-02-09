pub mod testutils;

use swap::protocol::{alice, bob, bob::BobState};
use testutils::{bob_run_until::is_lock_proof_received, SlowCancelConfig};

#[tokio::test]
async fn given_bob_restarts_after_lock_proof_received_resume_swap() {
    testutils::setup_test(SlowCancelConfig, |mut ctx| async move {
        let (alice_swap, _) = ctx.new_swap_as_alice().await;
        let (bob_swap, bob_join_handle) = ctx.new_swap_as_bob().await;

        let alice_handle = alice::run(alice_swap);
        let alice_swap_handle = tokio::spawn(alice_handle);

        let bob_state = bob::run_until(bob_swap, is_lock_proof_received)
            .await
            .unwrap();

        assert!(matches!(bob_state, BobState::XmrLockProofReceived { .. }));

        let (bob_swap, _) = ctx.stop_and_resume_bob_from_db(bob_join_handle).await;
        assert!(matches!(
            bob_swap.state,
            BobState::XmrLockProofReceived { .. }
        ));

        let bob_state = bob::run(bob_swap).await.unwrap();

        ctx.assert_bob_redeemed(bob_state).await;

        let alice_state = alice_swap_handle.await.unwrap().unwrap();
        ctx.assert_alice_redeemed(alice_state).await;
    })
    .await;
}