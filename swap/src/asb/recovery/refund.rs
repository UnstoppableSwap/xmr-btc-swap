use crate::common::retry;
use crate::monero;
use crate::protocol::Database;
use crate::protocol::alice::AliceState;
use crate::protocol::alice::swap::XmrRefundable;
use anyhow::{Context, Result, bail};
use bitcoin_wallet::BitcoinWallet;
use libp2p::PeerId;
use monero_interface::PublishTransaction;
use std::convert::TryInto;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "Counterparty {0} did not refund the BTC yet. You can try again later or try to punish."
    )]
    RefundTransactionNotPublishedYet(PeerId),

    // Errors indicating that the swap cannot be refunded because because it is in a abort/final
    // state
    #[error("Swap is in state {0} where no XMR was locked. Try aborting instead.")]
    NoXmrLocked(AliceState),
    #[error("Swap is in state {0} which is not refundable")]
    SwapNotRefundable(AliceState),
}

pub async fn refund(
    swap_id: Uuid,
    bitcoin_wallet: Arc<dyn BitcoinWallet>,
    monero_wallet: Arc<monero::Wallets>,
    db: Arc<dyn Database + Send + Sync>,
) -> Result<AliceState> {
    let state = db.get_state(swap_id).await?.try_into()?;

    let (transfer_proof, state3) = match state {
        // In case no XMR has been locked, move to Safely Aborted
        AliceState::Started { .. }
        | AliceState::BtcLockTransactionSeen { .. }
        | AliceState::BtcLocked { .. }
        | AliceState::XmrReadyToLock { .. } => bail!(Error::NoXmrLocked(state)),

        // Refund potentially possible (no knowledge of cancel transaction)
        AliceState::XmrLockTransactionConstructed { transfer_proof, state3, .. }
        | AliceState::XmrLockTransactionSent { transfer_proof, state3, .. }
        | AliceState::XmrLocked { transfer_proof, state3, .. }
        | AliceState::XmrLockTransferProofSent { transfer_proof, state3, .. }
        | AliceState::EncSigLearned { transfer_proof, state3, .. } => {
            (Some(transfer_proof), state3)
        }
        AliceState::WaitingForCancelTimelockExpiration { transfer_proof, state3, .. }
        | AliceState::CancelTimelockExpired { transfer_proof, state3, .. }

        // Refund possible due to cancel transaction already being published
        | AliceState::BtcCancelled { transfer_proof, state3, .. }
        | AliceState::BtcRefunded { transfer_proof, state3, .. }
        | AliceState::BtcPartiallyRefunded { transfer_proof, state3, .. }
        | AliceState::XmrRefundable { transfer_proof, state3, .. }
        | AliceState::BtcPunishable { transfer_proof, state3, .. } => {
            (transfer_proof, state3)
        }

        // Alice already in final state
        AliceState::BtcRedeemTransactionPublished { .. }
        | AliceState::BtcRedeemed
        | AliceState::XmrRefundTxConstructed { .. }
        | AliceState::XmrRefundTxPublished { .. }
        | AliceState::XmrRefunded { .. }
        | AliceState::BtcWithholdPublished { .. }
        | AliceState::BtcWithholdConfirmed { .. }
        | AliceState::BtcMercyGranted { .. }
        | AliceState::BtcMercyPublished { .. }
        | AliceState::BtcMercyConfirmed { .. }
        | AliceState::BtcEarlyRefundable { .. }
        | AliceState::BtcEarlyRefunded(_)
        | AliceState::BtcPunished { .. }
        | AliceState::SafelyAborted => bail!(Error::SwapNotRefundable(state)),
    };

    tracing::info!(%swap_id, "Trying to manually refund swap");

    let spend_key = if let Some(spend_key) = state3.refund_btc(bitcoin_wallet.as_ref()).await? {
        tracing::debug!(%swap_id, "Bitcoin refund transaction found, extracting key to refund Monero");
        spend_key
    } else {
        let bob_peer_id = db.get_peer_id(swap_id).await?;
        bail!(Error::RefundTransactionNotPublishedYet(bob_peer_id),);
    };

    let transfer_proof = transfer_proof.context(
        "We have the refund key, but recovery of unknown funds is not yet implemented. Funds are safe.",
    )?;

    retry(
        "Refund Monero",
        || async {
            let xmr_refund_tx = state3
                .construct_xmr_refund_transaction(
                    monero_wallet.clone(),
                    swap_id,
                    spend_key,
                    transfer_proof.clone(),
                )
                .await
                .map_err(backoff::Error::transient)?;

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
        Duration::from_secs(60),
    )
    .await?;

    let mut state = AliceState::XmrRefunded {
        state3: Some(state3.clone()),
    };

    db.insert_latest_state(swap_id, state.clone().into())
        .await?;

    if state3.should_publish_tx_withhold.unwrap_or(false) {
        let timelocks = state3.expired_timelocks(bitcoin_wallet.as_ref()).await?;

        if matches!(
            timelocks,
            swap_core::bitcoin::ExpiredTimelocks::RemainingRefund
        ) {
            tracing::warn!(%swap_id, "Remaining refund timelock already expired, Bob may have already reclaimed. Attempting TxWithhold anyway");
        }

        tracing::info!(%swap_id, "Publishing TxWithhold to withhold anti-spam deposit");

        let signed_tx = state3
            .signed_withhold_transaction()
            .context("Failed to construct signed TxWithhold")?;

        let (_txid, subscription) = bitcoin_wallet
            .ensure_broadcasted(signed_tx, "withhold")
            .await
            .context("Failed to broadcast TxWithhold")?;

        state = AliceState::BtcWithholdPublished {
            state3: state3.clone(),
        };
        db.insert_latest_state(swap_id, state.clone().into())
            .await?;

        subscription
            .wait_until_final()
            .await
            .context("Failed to wait for TxWithhold confirmation")?;

        state = AliceState::BtcWithholdConfirmed { state3 };
        db.insert_latest_state(swap_id, state.clone().into())
            .await?;
    }

    Ok(state)
}
