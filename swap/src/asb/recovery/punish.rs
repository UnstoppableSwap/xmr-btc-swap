use crate::protocol::Database;
use crate::protocol::alice::AliceState;
use anyhow::{Result, bail};
use bitcoin_wallet::BitcoinWallet;
use std::convert::TryInto;
use std::sync::Arc;
use swap_core::bitcoin::Txid;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Cannot punish swap because it is in state {0} which is not punishable")]
    SwapNotPunishable(AliceState),
}

pub async fn punish(
    swap_id: Uuid,
    bitcoin_wallet: Arc<dyn BitcoinWallet>,
    db: Arc<dyn Database>,
) -> Result<(Txid, AliceState)> {
    let state = db.get_state(swap_id).await?.try_into()?;

    let (state3, transfer_proof) = match state {
        // Punish potentially possible (no knowledge of cancel transaction)
        AliceState::XmrLockTransactionConstructed {state3, transfer_proof, ..}
        | AliceState::XmrLockTransactionSent {state3, transfer_proof, ..}
        | AliceState::XmrLocked {state3, transfer_proof, ..}
        | AliceState::XmrLockTransferProofSent {state3, transfer_proof, ..}
        | AliceState::EncSigLearned {state3, transfer_proof, ..}
        | AliceState::BtcRedeemTransactionPublished { state3, transfer_proof, .. } => {
            (state3, Some(transfer_proof))
        }
        AliceState::WaitingForCancelTimelockExpiration {state3, transfer_proof, ..}
        | AliceState::CancelTimelockExpired {state3, transfer_proof, ..}
        // Punish possible due to cancel transaction already being published
        | AliceState::BtcCancelled {state3, transfer_proof, ..}
        | AliceState::BtcPunishable {state3, transfer_proof, ..}
        // The state machine is in a state where punish is theoretically impossible but we try and punish anyway as this is what the user wants
        | AliceState::BtcRefunded { state3, transfer_proof,.. } => { (state3, transfer_proof) }
        | AliceState::BtcPartiallyRefunded { state3, transfer_proof,.. } => { (state3, transfer_proof) }
        | AliceState::XmrRefundable { state3, transfer_proof,.. } => { (state3, transfer_proof) }

        // Alice already in final state or at the start of the swap so we can't punish
        | AliceState::Started { .. }
        | AliceState::BtcLockTransactionSeen { .. }
        | AliceState::BtcLocked { .. }
        | AliceState::XmrReadyToLock { .. }
        | AliceState::BtcRedeemed { .. }
        | AliceState::XmrRefundTxConstructed { .. }
        | AliceState::XmrRefundTxPublished { .. }
        | AliceState::XmrRefunded { .. }
        | AliceState::BtcWithholdPublished { .. }
        | AliceState::BtcWithholdConfirmed { .. }
        | AliceState::BtcMercyGranted { .. }
        | AliceState::BtcMercyPublished { .. }
        | AliceState::BtcMercyConfirmed { .. }
        | AliceState::BtcPunished { .. }
        | AliceState::BtcEarlyRefundable { .. }
        | AliceState::BtcEarlyRefunded(_)
        | AliceState::SafelyAborted => bail!(Error::SwapNotPunishable(state)),
    };

    tracing::info!(%swap_id, "Trying to manually punish swap");

    let txid = state3.punish_btc(bitcoin_wallet.as_ref()).await?;

    let state = AliceState::BtcPunished {
        state3: state3.clone(),
        transfer_proof,
    };
    db.insert_latest_state(swap_id, state.clone().into())
        .await?;

    Ok((txid, state))
}
