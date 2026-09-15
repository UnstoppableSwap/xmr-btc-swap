use serde::{Deserialize, Serialize};
use std::fmt;
use swap_core::bitcoin::EncryptedSignature;
use swap_core::monero;
use swap_core::monero::{BlockHeight, TransferProof};
use swap_machine::alice;
use swap_machine::alice::AliceState;

// Large enum variant is fine because this is only used for database
// and is dropped once written in DB.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum Alice {
    Started {
        state3: alice::State3,
    },
    BtcLockTransactionSeen {
        state3: alice::State3,
    },
    BtcLocked {
        state3: alice::State3,
    },
    XmrLockTransactionConstructed {
        monero_wallet_restore_blockheight: BlockHeight,
        #[serde(with = "swap_serde::monero::transaction")]
        xmr_lock_tx: monero_oxide_wallet::transaction::Transaction,
        transfer_proof: TransferProof,
        state3: alice::State3,
    },
    XmrReadyToLock {
        state3: alice::State3,
        monero_wallet_restore_blockheight: BlockHeight,
    },
    XmrLockTransactionSent {
        monero_wallet_restore_blockheight: BlockHeight,
        transfer_proof: TransferProof,
        state3: alice::State3,
    },
    XmrLocked {
        monero_wallet_restore_blockheight: BlockHeight,
        transfer_proof: TransferProof,
        state3: alice::State3,
    },
    XmrLockTransferProofSent {
        monero_wallet_restore_blockheight: BlockHeight,
        transfer_proof: TransferProof,
        state3: alice::State3,
    },
    EncSigLearned {
        monero_wallet_restore_blockheight: BlockHeight,
        transfer_proof: TransferProof,
        encrypted_signature: EncryptedSignature,
        state3: alice::State3,
    },
    BtcRedeemTransactionPublished {
        state3: alice::State3,
        transfer_proof: TransferProof,
    },
    WaitingForCancelTimelockExpiration {
        monero_wallet_restore_blockheight: BlockHeight,
        transfer_proof: Option<TransferProof>,
        state3: alice::State3,
    },
    CancelTimelockExpired {
        monero_wallet_restore_blockheight: BlockHeight,
        transfer_proof: Option<TransferProof>,
        state3: alice::State3,
    },
    BtcCancelled {
        monero_wallet_restore_blockheight: BlockHeight,
        transfer_proof: Option<TransferProof>,
        state3: alice::State3,
    },
    BtcPunishable {
        monero_wallet_restore_blockheight: BlockHeight,
        transfer_proof: Option<TransferProof>,
        state3: alice::State3,
    },
    BtcEarlyRefundable {
        state3: alice::State3,
    },
    BtcRefunded {
        monero_wallet_restore_blockheight: BlockHeight,
        transfer_proof: Option<TransferProof>,
        state3: alice::State3,
        #[serde(with = "swap_serde::monero::private_key")]
        spend_key: monero::PrivateKey,
    },
    BtcPartiallyRefunded {
        monero_wallet_restore_blockheight: BlockHeight,
        transfer_proof: Option<TransferProof>,
        state3: alice::State3,
        #[serde(with = "swap_serde::monero::private_key")]
        spend_key: monero::PrivateKey,
    },
    XmrRefundable {
        monero_wallet_restore_blockheight: BlockHeight,
        transfer_proof: Option<TransferProof>,
        state3: alice::State3,
        #[serde(with = "swap_serde::monero::private_key")]
        spend_key: monero::PrivateKey,
    },
    XmrRefundTxConstructed {
        state3: alice::State3,
        #[serde(with = "swap_serde::monero::transaction")]
        xmr_refund_tx: monero_oxide_wallet::transaction::Transaction,
    },
    XmrRefundTxPublished {
        state3: alice::State3,
        #[serde(with = "swap_serde::monero::transaction")]
        xmr_refund_tx: monero_oxide_wallet::transaction::Transaction,
    },
    BtcWithholdPublished {
        state3: alice::State3,
    },
    BtcMercyGranted {
        state3: alice::State3,
    },
    BtcMercyPublished {
        state3: alice::State3,
    },
    Done(#[serde(deserialize_with = "deserialize_end_state_compat")] AliceEndState),
}

#[derive(Clone, strum::Display, Debug, Deserialize, Serialize, PartialEq)]
pub enum AliceEndState {
    SafelyAborted,
    BtcRedeemed,
    XmrRefunded {
        #[serde(default)]
        state3: Option<alice::State3>,
    },
    BtcEarlyRefunded {
        state3: alice::State3,
    },
    BtcPunished {
        state3: alice::State3,
        transfer_proof: Option<TransferProof>,
    },
    BtcWithheld {
        state3: alice::State3,
    },
    BtcMercyConfirmed {
        state3: alice::State3,
    },
}

/// Deserializes `AliceEndState` with backwards compatibility for pre-4.0.0 databases
/// where `XmrRefunded` was a unit variant (`"XmrRefunded"`) instead of a struct variant
/// (`{"XmrRefunded": {"state3": ...}}`).
fn deserialize_end_state_compat<'de, D>(deserializer: D) -> Result<AliceEndState, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Compat {
        Current(AliceEndState),
        LegacyXmrRefunded(LegacyXmrRefunded),
    }

    #[derive(Deserialize)]
    enum LegacyXmrRefunded {
        XmrRefunded,
    }

    match Compat::deserialize(deserializer)? {
        Compat::Current(state) => Ok(state),
        Compat::LegacyXmrRefunded(LegacyXmrRefunded::XmrRefunded) => {
            Ok(AliceEndState::XmrRefunded { state3: None })
        }
    }
}

impl From<AliceState> for Alice {
    fn from(alice_state: AliceState) -> Self {
        match alice_state {
            AliceState::Started { state3 } => Alice::Started { state3: *state3 },
            AliceState::BtcLockTransactionSeen { state3 } => {
                Alice::BtcLockTransactionSeen { state3: *state3 }
            }
            AliceState::BtcLocked { state3 } => Alice::BtcLocked { state3: *state3 },
            AliceState::XmrLockTransactionConstructed {
                monero_wallet_restore_blockheight,
                xmr_lock_tx,
                transfer_proof,
                state3,
            } => Alice::XmrLockTransactionConstructed {
                monero_wallet_restore_blockheight,
                xmr_lock_tx,
                transfer_proof,
                state3: *state3,
            },
            AliceState::XmrReadyToLock {
                state3,
                monero_wallet_restore_blockheight,
            } => Alice::XmrReadyToLock {
                state3: *state3,
                monero_wallet_restore_blockheight,
            },
            AliceState::XmrLockTransactionSent {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            } => Alice::XmrLockTransactionSent {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: *state3,
            },
            AliceState::XmrLocked {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            } => Alice::XmrLocked {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: *state3,
            },
            AliceState::XmrLockTransferProofSent {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            } => Alice::XmrLockTransferProofSent {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: *state3,
            },
            AliceState::EncSigLearned {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
                encrypted_signature,
            } => Alice::EncSigLearned {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: *state3,
                encrypted_signature: encrypted_signature.as_ref().clone(),
            },
            AliceState::BtcRedeemTransactionPublished {
                state3,
                transfer_proof,
            } => Alice::BtcRedeemTransactionPublished {
                state3: *state3,
                transfer_proof,
            },
            AliceState::BtcRedeemed => Alice::Done(AliceEndState::BtcRedeemed),
            AliceState::BtcCancelled {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            } => Alice::BtcCancelled {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: *state3,
            },
            AliceState::BtcRefunded {
                monero_wallet_restore_blockheight,
                transfer_proof,
                spend_key,
                state3,
            } => Alice::BtcRefunded {
                monero_wallet_restore_blockheight,
                transfer_proof,
                spend_key,
                state3: *state3,
            },
            AliceState::BtcPartiallyRefunded {
                monero_wallet_restore_blockheight,
                transfer_proof,
                spend_key,
                state3,
            } => Alice::BtcPartiallyRefunded {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: *state3,
                spend_key,
            },
            AliceState::XmrRefundable {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
                spend_key,
            } => Alice::XmrRefundable {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: *state3,
                spend_key,
            },
            AliceState::XmrRefundTxConstructed {
                state3,
                xmr_refund_tx,
            } => Alice::XmrRefundTxConstructed {
                state3: *state3,
                xmr_refund_tx,
            },
            AliceState::XmrRefundTxPublished {
                state3,
                xmr_refund_tx,
            } => Alice::XmrRefundTxPublished {
                state3: *state3,
                xmr_refund_tx,
            },
            AliceState::BtcEarlyRefundable { state3 } => {
                Alice::BtcEarlyRefundable { state3: *state3 }
            }
            AliceState::BtcEarlyRefunded(state3) => {
                Alice::Done(AliceEndState::BtcEarlyRefunded { state3: *state3 })
            }
            AliceState::BtcPunishable {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            } => Alice::BtcPunishable {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: *state3,
            },
            AliceState::XmrRefunded { state3 } => Alice::Done(AliceEndState::XmrRefunded {
                state3: state3.map(|s| s.as_ref().clone()),
            }),
            AliceState::BtcWithholdPublished { state3 } => {
                Alice::BtcWithholdPublished { state3: *state3 }
            }
            AliceState::BtcWithholdConfirmed { state3 } => {
                Alice::Done(AliceEndState::BtcWithheld { state3: *state3 })
            }
            AliceState::BtcMercyGranted { state3 } => Alice::BtcMercyGranted { state3: *state3 },
            AliceState::BtcMercyPublished { state3 } => {
                Alice::BtcMercyPublished { state3: *state3 }
            }
            AliceState::BtcMercyConfirmed { state3 } => {
                Alice::Done(AliceEndState::BtcMercyConfirmed { state3: *state3 })
            }
            AliceState::WaitingForCancelTimelockExpiration {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            } => Alice::WaitingForCancelTimelockExpiration {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: *state3,
            },
            AliceState::CancelTimelockExpired {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            } => Alice::CancelTimelockExpired {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: *state3,
            },
            AliceState::BtcPunished {
                state3,
                transfer_proof,
            } => Alice::Done(AliceEndState::BtcPunished {
                state3: *state3,
                transfer_proof,
            }),
            AliceState::SafelyAborted => Alice::Done(AliceEndState::SafelyAborted),
        }
    }
}

impl From<Alice> for AliceState {
    fn from(db_state: Alice) -> Self {
        match db_state {
            Alice::Started { state3 } => AliceState::Started {
                state3: Box::new(state3),
            },
            Alice::BtcLockTransactionSeen { state3 } => AliceState::BtcLockTransactionSeen {
                state3: Box::new(state3),
            },
            Alice::BtcLocked { state3 } => AliceState::BtcLocked {
                state3: Box::new(state3),
            },
            Alice::XmrLockTransactionConstructed {
                monero_wallet_restore_blockheight,
                xmr_lock_tx,
                transfer_proof,
                state3,
            } => AliceState::XmrLockTransactionConstructed {
                monero_wallet_restore_blockheight,
                xmr_lock_tx,
                transfer_proof,
                state3: Box::new(state3),
            },
            Alice::XmrReadyToLock {
                state3,
                monero_wallet_restore_blockheight,
            } => AliceState::XmrReadyToLock {
                state3: Box::new(state3),
                monero_wallet_restore_blockheight,
            },
            Alice::XmrLockTransactionSent {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            } => AliceState::XmrLockTransactionSent {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: Box::new(state3),
            },
            Alice::XmrLocked {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            } => AliceState::XmrLocked {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: Box::new(state3),
            },
            Alice::XmrLockTransferProofSent {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            } => AliceState::XmrLockTransferProofSent {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: Box::new(state3),
            },
            Alice::EncSigLearned {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: state,
                encrypted_signature,
            } => AliceState::EncSigLearned {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: Box::new(state),
                encrypted_signature: Box::new(encrypted_signature),
            },
            Alice::BtcRedeemTransactionPublished {
                state3,
                transfer_proof,
            } => AliceState::BtcRedeemTransactionPublished {
                state3: Box::new(state3),
                transfer_proof,
            },
            Alice::WaitingForCancelTimelockExpiration {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            } => AliceState::WaitingForCancelTimelockExpiration {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: Box::new(state3),
            },
            Alice::CancelTimelockExpired {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            } => AliceState::CancelTimelockExpired {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: Box::new(state3),
            },
            Alice::BtcCancelled {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            } => AliceState::BtcCancelled {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: Box::new(state3),
            },
            Alice::BtcPunishable {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
            } => AliceState::BtcPunishable {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3: Box::new(state3),
            },
            Alice::BtcRefunded {
                monero_wallet_restore_blockheight,
                transfer_proof,
                spend_key,
                state3,
            } => AliceState::BtcRefunded {
                monero_wallet_restore_blockheight,
                transfer_proof,
                spend_key,
                state3: Box::new(state3),
            },
            Alice::BtcPartiallyRefunded {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
                spend_key,
            } => AliceState::BtcPartiallyRefunded {
                monero_wallet_restore_blockheight,
                transfer_proof,
                spend_key,
                state3: Box::new(state3),
            },
            Alice::BtcEarlyRefundable { state3 } => AliceState::BtcEarlyRefundable {
                state3: Box::new(state3),
            },
            Alice::XmrRefundable {
                monero_wallet_restore_blockheight,
                transfer_proof,
                state3,
                spend_key,
            } => AliceState::XmrRefundable {
                monero_wallet_restore_blockheight,
                transfer_proof,
                spend_key,
                state3: Box::new(state3),
            },
            Alice::XmrRefundTxConstructed {
                state3,
                xmr_refund_tx,
            } => AliceState::XmrRefundTxConstructed {
                state3: Box::new(state3),
                xmr_refund_tx,
            },
            Alice::XmrRefundTxPublished {
                state3,
                xmr_refund_tx,
            } => AliceState::XmrRefundTxPublished {
                state3: Box::new(state3),
                xmr_refund_tx,
            },
            Alice::BtcWithholdPublished { state3 } => AliceState::BtcWithholdPublished {
                state3: Box::new(state3),
            },
            Alice::BtcMercyGranted { state3 } => AliceState::BtcMercyGranted {
                state3: Box::new(state3),
            },
            Alice::BtcMercyPublished { state3 } => AliceState::BtcMercyPublished {
                state3: Box::new(state3),
            },
            Alice::Done(end_state) => match end_state {
                AliceEndState::SafelyAborted => AliceState::SafelyAborted,
                AliceEndState::BtcRedeemed => AliceState::BtcRedeemed,
                AliceEndState::XmrRefunded { state3 } => AliceState::XmrRefunded {
                    state3: state3.map(Box::new),
                },
                AliceEndState::BtcPunished {
                    state3,
                    transfer_proof,
                } => AliceState::BtcPunished {
                    state3: Box::new(state3),
                    transfer_proof,
                },
                AliceEndState::BtcEarlyRefunded { state3 } => {
                    AliceState::BtcEarlyRefunded(Box::new(state3))
                }
                AliceEndState::BtcWithheld { state3 } => AliceState::BtcWithholdConfirmed {
                    state3: Box::new(state3),
                },
                AliceEndState::BtcMercyConfirmed { state3 } => AliceState::BtcMercyConfirmed {
                    state3: Box::new(state3),
                },
            },
        }
    }
}

impl fmt::Display for Alice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Alice::Started { .. } => write!(f, "Started"),
            Alice::BtcLockTransactionSeen { .. } => {
                write!(f, "Bitcoin lock transaction in mempool")
            }
            Alice::BtcLocked { .. } => f.write_str("Bitcoin locked"),
            Alice::XmrLockTransactionConstructed { .. } => {
                f.write_str("Monero lock transaction constructed")
            }
            Alice::XmrReadyToLock { .. } => f.write_str("Monero ready to lock"),
            Alice::XmrLockTransactionSent { .. } => f.write_str("Monero lock transaction sent"),
            Alice::XmrLocked { .. } => f.write_str("Monero locked"),
            Alice::XmrLockTransferProofSent { .. } => {
                f.write_str("Monero lock transfer proof sent")
            }
            Alice::EncSigLearned { .. } => f.write_str("Encrypted signature learned"),
            Alice::BtcRedeemTransactionPublished { .. } => {
                f.write_str("Bitcoin redeem transaction published")
            }
            Alice::WaitingForCancelTimelockExpiration { .. } => {
                f.write_str("Waiting for cancel timelock to expire")
            }
            Alice::CancelTimelockExpired { .. } => f.write_str("Cancel timelock is expired"),
            Alice::BtcCancelled { .. } => f.write_str("Bitcoin cancel transaction published"),
            Alice::BtcPunishable { .. } => f.write_str("Bitcoin punishable"),
            Alice::BtcRefunded { .. } => f.write_str("Monero refundable"),
            Alice::BtcPartiallyRefunded { .. } => f.write_str("Monero refundable"),
            Alice::BtcEarlyRefundable { .. } => f.write_str("Bitcoin early refundable"),
            Alice::XmrRefundable { .. } => f.write_str("Bitcoin early refundable"),
            Alice::XmrRefundTxConstructed { .. } => {
                f.write_str("Monero refund transaction constructed")
            }
            Alice::XmrRefundTxPublished { .. } => {
                f.write_str("Monero refund transaction published")
            }
            Alice::BtcWithholdPublished { .. } => {
                f.write_str("Bitcoin withhold transaction published")
            }
            Alice::BtcMercyGranted { .. } => f.write_str("Bitcoin mercy initiated"),
            Alice::BtcMercyPublished { .. } => f.write_str("Bitcoin mercy published"),
            Alice::Done(end_state) => write!(f, "Done: {}", end_state),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_xmr_refunded_unit_variant_deserializes() {
        // Pre-4.0.0: XmrRefunded was a unit variant in AliceEndState
        let old_json = r#"{"Done":"XmrRefunded"}"#;
        let alice: Alice =
            serde_json::from_str(old_json).expect("legacy XmrRefunded should deserialize");

        let Alice::Done(AliceEndState::XmrRefunded { state3 }) = alice else {
            panic!("expected Alice::Done(XmrRefunded), got: {alice:?}");
        };
        assert_eq!(state3, None);
    }

    #[test]
    fn current_xmr_refunded_struct_variant_deserializes() {
        // 4.0.0+: XmrRefunded is a struct variant with optional state3
        let new_json = r#"{"Done":{"XmrRefunded":{"state3":null}}}"#;
        let alice: Alice =
            serde_json::from_str(new_json).expect("current XmrRefunded should deserialize");

        let Alice::Done(AliceEndState::XmrRefunded { state3 }) = alice else {
            panic!("expected Alice::Done(XmrRefunded), got: {alice:?}");
        };
        assert_eq!(state3, None);
    }
}
