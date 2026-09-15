use crate::protocol::Database;
use crate::protocol::alice::AliceState;
use anyhow::{Result, bail};
use bitcoin_wallet::{BitcoinWallet, RpcErrorCode, Txid, parse_rpc_error_code};
use std::convert::TryInto;
use std::sync::Arc;
use uuid::Uuid;

pub async fn cancel(
    swap_id: Uuid,
    bitcoin_wallet: Arc<dyn BitcoinWallet>,
    db: Arc<dyn Database>,
) -> Result<(Txid, AliceState)> {
    let state = db.get_state(swap_id).await?.try_into()?;

    let (monero_wallet_restore_blockheight, transfer_proof, state3) = match state {

        // In case no XMR has been locked, move to Safely Aborted
        AliceState::Started { .. }
        | AliceState::BtcLockTransactionSeen { .. }
        | AliceState::BtcLocked { .. }
        | AliceState::XmrReadyToLock { .. } => bail!("Cannot cancel swap {} because it is in state {} where no XMR was locked.", swap_id, state),

        AliceState::XmrLockTransactionConstructed { monero_wallet_restore_blockheight, transfer_proof, state3, .. }
        | AliceState::XmrLockTransactionSent { monero_wallet_restore_blockheight, transfer_proof, state3,  }
        | AliceState::XmrLocked { monero_wallet_restore_blockheight, transfer_proof, state3 }
        | AliceState::XmrLockTransferProofSent { monero_wallet_restore_blockheight, transfer_proof, state3 }

        // in cancel mode we do not care about the fact that we could redeem, but always wait for cancellation (leading either refund or punish)
        | AliceState::EncSigLearned { monero_wallet_restore_blockheight, transfer_proof, state3, .. } => {
            (monero_wallet_restore_blockheight, Some(transfer_proof), state3)
        }
        AliceState::WaitingForCancelTimelockExpiration { monero_wallet_restore_blockheight, transfer_proof, state3}
        | AliceState::CancelTimelockExpired { monero_wallet_restore_blockheight, transfer_proof, state3}
        | AliceState::BtcCancelled { monero_wallet_restore_blockheight, transfer_proof, state3 }
        | AliceState::BtcRefunded { monero_wallet_restore_blockheight, transfer_proof,  state3 ,.. }
        | AliceState::BtcPartiallyRefunded { monero_wallet_restore_blockheight, transfer_proof,  state3 ,.. }
        | AliceState::XmrRefundable { monero_wallet_restore_blockheight, transfer_proof,  state3 ,.. }
        | AliceState::BtcPunishable { monero_wallet_restore_blockheight, transfer_proof, state3 }  => {
            (monero_wallet_restore_blockheight, transfer_proof, state3)
        }

        // The redeem transaction was already published, it is not safe to cancel anymore
        AliceState::BtcRedeemTransactionPublished { .. } => bail!(" The redeem transaction was already published, it is not safe to cancel anymore"),

        // Alice already in final state
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
        | AliceState::SafelyAborted => bail!("Swap is in state {} which is not cancelable", state),
    };

    let txid = match state3.submit_tx_cancel(bitcoin_wallet.as_ref()).await {
        Ok(txid) => txid,
        Err(err) => {
            if let Ok(code) = parse_rpc_error_code(&err) {
                if code == i64::from(RpcErrorCode::RpcVerifyAlreadyInChain) {
                    tracing::info!("Cancel transaction has already been confirmed on chain")
                }
            }
            bail!(err);
        }
    };

    let state = AliceState::BtcCancelled {
        monero_wallet_restore_blockheight,
        transfer_proof,
        state3,
    };
    db.insert_latest_state(swap_id, state.clone().into())
        .await?;

    Ok((txid, state))
}
