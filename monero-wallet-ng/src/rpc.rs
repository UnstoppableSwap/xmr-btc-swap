//! RPC-related traits and implementations for monero-oxide.
//!
//! This module provides additional traits that extend monero-oxide's functionality,
//! particularly for querying transaction status information that isn't exposed
//! by the standard traits.

use core::future::Future;

use monero_daemon_rpc::{HttpTransport, MoneroDaemon};
use monero_interface::InterfaceError;
use monero_oxide_wallet::transaction::{Pruned, Transaction};

/// Spend status of a single key image, per the daemon's `is_key_image_spent` RPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyImageSpentStatus {
    /// The key image has not been spent.
    Unspent,
    /// The key image was spent by a transaction confirmed in the blockchain.
    SpentInBlockchain,
    /// The key image was spent by a transaction currently in the mempool.
    SpentInPool,
}

/// Query the spend status of key images directly, without submitting a transaction.
///
/// Unlike the `double_spend` flag from `send_raw_transaction`, this distinguishes a
/// confirmed spend (`SpentInBlockchain`) from a transient pool spend (`SpentInPool`).
pub trait IsKeyImageSpent: Sync {
    /// Returns the spend status of each key image, in the same order as the input.
    fn is_key_image_spent(
        &self,
        key_images: &[[u8; 32]],
    ) -> impl Send + Future<Output = Result<Vec<KeyImageSpentStatus>, InterfaceError>>;
}

impl<T: HttpTransport> IsKeyImageSpent for MoneroDaemon<T> {
    fn is_key_image_spent(
        &self,
        key_images: &[[u8; 32]],
    ) -> impl Send + Future<Output = Result<Vec<KeyImageSpentStatus>, InterfaceError>> {
        let key_images_hex: Vec<String> = key_images.iter().map(hex::encode).collect();

        async move {
            let expected_statuses = key_images_hex.len();
            let params = serde_json::json!({ "key_images": key_images_hex }).to_string();

            let response = self
                .rpc_call(
                    "is_key_image_spent",
                    Some(params),
                    // The response is a small array of integers.
                    65536,
                )
                .await?;

            parse_key_image_spent_response(&response, expected_statuses)
        }
    }
}

const MEMPOOL_HASHES_RESPONSE_SIZE_LIMIT: usize = 2 * 1024 * 1024;
const NON_MINER_TX_SIZE_UPPER_BOUND: usize = 1_000_000;

#[derive(Debug, Clone)]
pub struct MempoolTransaction {
    pub tx_id: [u8; 32],
    pub tx: Transaction<Pruned>,
}

#[derive(Debug, thiserror::Error)]
pub enum TransactionStatusError {
    #[error("Interface error: {0}")]
    Interface(#[from] InterfaceError),
}

#[derive(Debug, thiserror::Error)]
pub enum MempoolTransactionsError {
    #[error("Interface error: {0}")]
    Interface(#[from] InterfaceError),
}

#[derive(Debug, Clone)]
pub enum TransactionStatus {
    Unknown, // the daemon does not know about the transaction
    InPool,  // the transaction is in the mempool
    InBlock {
        // the transaction is included in a block which the daemon believes is part of the longest chain
        block_height: u64,
    },
}

/// Provides the ability to query transaction status
///
/// This trait is separate from `ProvidesTransactions` because monero-oxide's
/// `ProvidesTransactions` doesn't currently expose block_height/in_pool fields.
pub trait ProvidesTransactionStatus: Sync {
    /// Get the status of a transaction by its hash.
    ///
    /// Returns [`TransactionStatus::Unknown`] if the daemon reports exactly this transaction as
    /// missed.
    fn transaction_status(
        &self,
        tx_id: [u8; 32],
    ) -> impl Send + Future<Output = Result<TransactionStatus, TransactionStatusError>>;
}

pub trait ProvidesMempoolTransactions: Sync {
    fn mempool_transaction_hashes(
        &self,
    ) -> impl Send + Future<Output = Result<Vec<[u8; 32]>, MempoolTransactionsError>>;

    fn mempool_transactions(
        &self,
        hashes: &[[u8; 32]],
    ) -> impl Send + Future<Output = Result<Vec<MempoolTransaction>, MempoolTransactionsError>>;
}

/// Data structures we get back from the RPC server
///
/// See: https://github.com/monero-project/monero/blob/48ad374b0d6d6e045128729534dc2508e6999afe/src/rpc/core_rpc_server_commands_defs.h#L358-L439
mod monerod {
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub(crate) struct IsKeyImageSpentResponse {
        pub(crate) status: String,
        pub(crate) untrusted: bool,
        pub(crate) spent_status: Vec<u8>,
    }

    #[derive(Deserialize)]
    pub(crate) struct GetTransactionsResponse {
        pub(crate) status: String,
        pub(crate) untrusted: bool,
        #[serde(default)]
        pub(crate) missed_tx: Vec<String>,
        #[serde(default)]
        pub(crate) txs: Vec<TransactionInfo>,
    }

    // See: https://github.com/SNeedlewoods/seraphis_wallet/blob/dbbccecc89e1121762a4ad6b531638ece82aa0c7/src/rpc/core_rpc_server_commands_defs.h#L406-L428
    #[derive(Deserialize)]
    pub(crate) struct TransactionInfo {
        pub(crate) tx_hash: Option<String>,
        pub(crate) pruned_as_hex: Option<String>,
        // `block_height` is only present if `in_pool` is false
        pub(crate) block_height: Option<u64>,
        // `in_pool` is always present
        pub(crate) in_pool: bool,
    }

    #[derive(Deserialize)]
    pub(crate) struct GetTransactionPoolHashesResponse {
        pub(crate) status: String,
        pub(crate) untrusted: bool,
        #[serde(default)]
        pub(crate) tx_hashes: Vec<String>,
    }
}

impl<T: HttpTransport> ProvidesTransactionStatus for MoneroDaemon<T> {
    fn transaction_status(
        &self,
        tx_id: [u8; 32],
    ) -> impl Send + Future<Output = Result<TransactionStatus, TransactionStatusError>> {
        async move {
            let tx_hash_hex = hex::encode(tx_id);

            // Use rpc_call to get transaction
            let response = self
                .rpc_call(
                    "get_transactions",
                    Some(format!(
                        r#"{{ "txs_hashes": ["{}"], "prune": true }}"#,
                        tx_hash_hex
                    )),
                    // 64kb, fairly arbitrary, but should be enough
                    65536,
                )
                .await?;

            let response = parse_get_transactions_response(&response)?;
            transaction_status_from_response(response, tx_id).map_err(Into::into)
        }
    }
}

impl<T: HttpTransport> ProvidesMempoolTransactions for MoneroDaemon<T> {
    fn mempool_transaction_hashes(
        &self,
    ) -> impl Send + Future<Output = Result<Vec<[u8; 32]>, MempoolTransactionsError>> {
        async move {
            let response = self
                .rpc_call(
                    "get_transaction_pool_hashes",
                    Some("{}".to_string()),
                    MEMPOOL_HASHES_RESPONSE_SIZE_LIMIT,
                )
                .await?;

            parse_transaction_pool_hashes_response(&response)
        }
    }

    fn mempool_transactions(
        &self,
        hashes: &[[u8; 32]],
    ) -> impl Send + Future<Output = Result<Vec<MempoolTransaction>, MempoolTransactionsError>>
    {
        async move {
            if hashes.is_empty() {
                return Ok(Vec::new());
            }

            let hashes_json = hashes
                .iter()
                .map(hex::encode)
                .map(|hash| format!(r#""{}""#, hash))
                .collect::<Vec<_>>()
                .join(",");
            let response_size_limit = hashes.len().saturating_mul(NON_MINER_TX_SIZE_UPPER_BOUND);

            let response = self
                .rpc_call(
                    "get_transactions",
                    Some(format!(
                        r#"{{ "txs_hashes": [{}], "prune": true }}"#,
                        hashes_json
                    )),
                    response_size_limit,
                )
                .await?;

            let response = parse_get_transactions_response(&response)?;

            if !response.missed_tx.is_empty() {
                return Err(InterfaceError::InvalidInterface(format!(
                    "Daemon missed mempool transactions: {}",
                    response.missed_tx.join(", ")
                ))
                .into());
            }

            validate_mempool_transaction_hashes(&response.txs, hashes)?;

            response
                .txs
                .into_iter()
                .map(parse_pruned_mempool_transaction)
                .collect()
        }
    }
}

fn parse_key_image_spent_response(
    response: &str,
    expected_statuses: usize,
) -> Result<Vec<KeyImageSpentStatus>, InterfaceError> {
    let response: monerod::IsKeyImageSpentResponse =
        serde_json::from_str(response).map_err(|e| {
            InterfaceError::InvalidInterface(format!(
                "Failed to parse is_key_image_spent response: {e}"
            ))
        })?;
    validate_daemon_response("is_key_image_spent", &response.status, response.untrusted)?;

    if response.spent_status.len() != expected_statuses {
        return Err(InterfaceError::InvalidInterface(format!(
            "is_key_image_spent returned {} statuses for {expected_statuses} key images",
            response.spent_status.len()
        )));
    }

    response
        .spent_status
        .into_iter()
        .map(|status| match status {
            0 => Ok(KeyImageSpentStatus::Unspent),
            1 => Ok(KeyImageSpentStatus::SpentInBlockchain),
            2 => Ok(KeyImageSpentStatus::SpentInPool),
            other => Err(InterfaceError::InvalidInterface(format!(
                "Unknown key image spent status {other}"
            ))),
        })
        .collect()
}

fn parse_transaction_pool_hashes_response(
    response: &str,
) -> Result<Vec<[u8; 32]>, MempoolTransactionsError> {
    let response: monerod::GetTransactionPoolHashesResponse = serde_json::from_str(response)
        .map_err(|e| {
            InterfaceError::InvalidInterface(format!(
                "Failed to parse get_transaction_pool_hashes response: {e}"
            ))
        })?;
    validate_daemon_response(
        "get_transaction_pool_hashes",
        &response.status,
        response.untrusted,
    )?;

    response
        .tx_hashes
        .into_iter()
        .map(|hash| decode_hash(&hash).map_err(MempoolTransactionsError::from))
        .collect()
}

fn parse_get_transactions_response(
    response: &str,
) -> Result<monerod::GetTransactionsResponse, InterfaceError> {
    let response: monerod::GetTransactionsResponse =
        serde_json::from_str(response).map_err(|e| {
            InterfaceError::InvalidInterface(format!(
                "Failed to parse get_transactions response: {e}"
            ))
        })?;
    validate_daemon_response("get_transactions", &response.status, response.untrusted)?;

    Ok(response)
}

fn validate_daemon_response(
    method: &str,
    status: &str,
    untrusted: bool,
) -> Result<(), InterfaceError> {
    if status != "OK" {
        return Err(InterfaceError::InvalidInterface(format!(
            "{method} returned status {status}"
        )));
    }

    if untrusted {
        return Err(InterfaceError::InvalidInterface(format!(
            "{method} returned an untrusted response"
        )));
    }

    Ok(())
}

fn transaction_status_from_response(
    response: monerod::GetTransactionsResponse,
    requested_hash: [u8; 32],
) -> Result<TransactionStatus, InterfaceError> {
    if response.txs.is_empty() && response.missed_tx.len() == 1 {
        let missed_hash = decode_hash(&response.missed_tx[0])?;
        if missed_hash == requested_hash {
            return Ok(TransactionStatus::Unknown);
        }
    } else if response.missed_tx.is_empty() && response.txs.len() == 1 {
        let tx_info = &response.txs[0];
        let returned_hash = decode_hash(tx_info.tx_hash.as_deref().ok_or_else(|| {
            InterfaceError::InvalidInterface(
                "Transaction status response missing tx_hash".to_string(),
            )
        })?)?;

        if returned_hash != requested_hash {
            return Err(InterfaceError::InvalidInterface(
                "Transaction status response returned an unexpected tx_hash".to_string(),
            ));
        }

        if tx_info.in_pool {
            return Ok(TransactionStatus::InPool);
        }

        return Ok(TransactionStatus::InBlock {
            block_height: tx_info.block_height.ok_or_else(|| {
                InterfaceError::InvalidInterface(
                    "Transaction has in_pool=false but has no block_height".to_string(),
                )
            })?,
        });
    }

    Err(InterfaceError::InvalidInterface(
        "get_transactions response did not contain exactly the requested transaction".to_string(),
    ))
}

fn validate_mempool_transaction_hashes(
    transactions: &[monerod::TransactionInfo],
    requested_hashes: &[[u8; 32]],
) -> Result<(), InterfaceError> {
    if transactions.len() != requested_hashes.len() {
        return Err(InterfaceError::InvalidInterface(
            "Daemon returned an unexpected number of mempool transactions".to_string(),
        ));
    }

    for (transaction, requested_hash) in transactions.iter().zip(requested_hashes) {
        let returned_hash = decode_hash(transaction.tx_hash.as_deref().ok_or_else(|| {
            InterfaceError::InvalidInterface(
                "Mempool transaction response missing tx_hash".to_string(),
            )
        })?)?;

        if returned_hash != *requested_hash {
            return Err(InterfaceError::InvalidInterface(
                "Daemon returned unexpected or out-of-order mempool transaction".to_string(),
            ));
        }
    }

    Ok(())
}

fn parse_pruned_mempool_transaction(
    info: monerod::TransactionInfo,
) -> Result<MempoolTransaction, MempoolTransactionsError> {
    let tx_id = decode_hash(info.tx_hash.as_deref().ok_or_else(|| {
        InterfaceError::InvalidInterface("Mempool transaction response missing tx_hash".to_string())
    })?)?;

    let blob = hex::decode(info.pruned_as_hex.as_deref().ok_or_else(|| {
        InterfaceError::InvalidInterface(
            "Mempool transaction response missing pruned_as_hex".to_string(),
        )
    })?)
    .map_err(|e| {
        InterfaceError::InvalidInterface(format!("Failed to decode mempool tx blob hex: {}", e))
    })?;

    let mut reader = blob.as_slice();
    let tx = Transaction::<Pruned>::read(&mut reader).map_err(|e| {
        InterfaceError::InvalidInterface(format!("Failed to parse mempool transaction: {}", e))
    })?;

    if !reader.is_empty() {
        return Err(InterfaceError::InvalidInterface(
            "Mempool transaction blob has trailing bytes".to_string(),
        )
        .into());
    }

    Ok(MempoolTransaction { tx_id, tx })
}

fn decode_hash(hash: &str) -> Result<[u8; 32], InterfaceError> {
    hex::decode(hash)
        .map_err(|e| {
            InterfaceError::InvalidInterface(format!("Failed to decode transaction hash: {}", e))
        })?
        .try_into()
        .map_err(|_| {
            InterfaceError::InvalidInterface("Transaction hash was not 32 bytes".to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_image_spent_response() {
        let response = r#"{
            "status": "OK",
            "untrusted": false,
            "spent_status": [0, 1, 2]
        }"#;

        assert_eq!(
            parse_key_image_spent_response(response, 3).unwrap(),
            vec![
                KeyImageSpentStatus::Unspent,
                KeyImageSpentStatus::SpentInBlockchain,
                KeyImageSpentStatus::SpentInPool,
            ]
        );
    }

    #[test]
    fn rejects_untrusted_key_image_spent_response() {
        let response = r#"{
            "status": "OK",
            "untrusted": true,
            "spent_status": [0]
        }"#;

        assert_eq!(
            parse_key_image_spent_response(response, 1).unwrap_err(),
            InterfaceError::InvalidInterface(
                "is_key_image_spent returned an untrusted response".to_string()
            )
        );
    }

    #[test]
    fn rejects_unknown_key_image_spent_status() {
        let response = r#"{
            "status": "OK",
            "untrusted": false,
            "spent_status": [3]
        }"#;

        assert_eq!(
            parse_key_image_spent_response(response, 1).unwrap_err(),
            InterfaceError::InvalidInterface("Unknown key image spent status 3".to_string())
        );
    }

    #[test]
    fn rejects_wrong_number_of_key_image_spent_statuses() {
        let response = r#"{
            "status": "OK",
            "untrusted": false,
            "spent_status": [0]
        }"#;

        assert_eq!(
            parse_key_image_spent_response(response, 2).unwrap_err(),
            InterfaceError::InvalidInterface(
                "is_key_image_spent returned 1 statuses for 2 key images".to_string()
            )
        );
    }

    #[test]
    fn parses_trusted_transaction_pool_hashes_response_without_hashes() {
        let response = r#"{
            "status": "OK",
            "untrusted": false
        }"#;

        assert_eq!(
            parse_transaction_pool_hashes_response(response).unwrap(),
            Vec::<[u8; 32]>::new()
        );
    }

    #[test]
    fn rejects_failed_transaction_pool_hashes_response_without_hashes() {
        let response = r#"{
            "status": "BUSY",
            "untrusted": false
        }"#;

        assert!(matches!(
            parse_transaction_pool_hashes_response(response),
            Err(MempoolTransactionsError::Interface(
                InterfaceError::InvalidInterface(message)
            )) if message == "get_transaction_pool_hashes returned status BUSY"
        ));
    }

    #[test]
    fn rejects_untrusted_transaction_pool_hashes_response_without_hashes() {
        let response = r#"{
            "status": "OK",
            "untrusted": true
        }"#;

        assert!(matches!(
            parse_transaction_pool_hashes_response(response),
            Err(MempoolTransactionsError::Interface(
                InterfaceError::InvalidInterface(message)
            )) if message == "get_transaction_pool_hashes returned an untrusted response"
        ));
    }

    #[test]
    fn rejects_transaction_pool_hashes_response_without_status_or_untrusted() {
        let responses = [r#"{ "untrusted": false }"#, r#"{ "status": "OK" }"#];

        for response in responses {
            assert!(matches!(
                parse_transaction_pool_hashes_response(response),
                Err(MempoolTransactionsError::Interface(
                    InterfaceError::InvalidInterface(message)
                )) if message.starts_with("Failed to parse get_transaction_pool_hashes response:")
            ));
        }
    }

    #[test]
    fn parses_unknown_transaction_response_without_txs() {
        let requested_hash = [1; 32];
        let response = format!(
            r#"{{
                "status": "OK",
                "untrusted": false,
                "missed_tx": ["{}"]
            }}"#,
            hex::encode(requested_hash)
        );

        let response = parse_get_transactions_response(&response).unwrap();
        assert!(matches!(
            transaction_status_from_response(response, requested_hash),
            Ok(TransactionStatus::Unknown)
        ));
    }

    #[test]
    fn parses_known_transaction_response_without_missed_tx() {
        let requested_hash = [1; 32];
        let response = format!(
            r#"{{
                "status": "OK",
                "untrusted": false,
                "txs": [{{
                    "tx_hash": "{}",
                    "in_pool": true
                }}]
            }}"#,
            hex::encode(requested_hash)
        );

        let response = parse_get_transactions_response(&response).unwrap();
        assert!(matches!(
            transaction_status_from_response(response, requested_hash),
            Ok(TransactionStatus::InPool)
        ));
    }

    #[test]
    fn rejects_failed_get_transactions_response_without_payload_arrays() {
        let response = r#"{
            "status": "BUSY",
            "untrusted": false
        }"#;

        assert!(matches!(
            parse_get_transactions_response(response),
            Err(InterfaceError::InvalidInterface(message))
                if message == "get_transactions returned status BUSY"
        ));
    }

    #[test]
    fn rejects_untrusted_get_transactions_response_without_payload_arrays() {
        let response = r#"{
            "status": "OK",
            "untrusted": true
        }"#;

        assert!(matches!(
            parse_get_transactions_response(response),
            Err(InterfaceError::InvalidInterface(message))
                if message == "get_transactions returned an untrusted response"
        ));
    }

    #[test]
    fn rejects_get_transactions_response_without_status_or_untrusted() {
        let responses = [r#"{ "untrusted": false }"#, r#"{ "status": "OK" }"#];

        for response in responses {
            assert!(matches!(
                parse_get_transactions_response(response),
                Err(InterfaceError::InvalidInterface(message))
                    if message.starts_with("Failed to parse get_transactions response:")
            ));
        }
    }

    #[test]
    fn transaction_status_is_unknown_only_for_exact_missed_hash() {
        let requested_hash = [1; 32];
        let response = get_transactions_response(vec![hex::encode(requested_hash)], vec![]);

        assert!(matches!(
            transaction_status_from_response(response, requested_hash),
            Ok(TransactionStatus::Unknown)
        ));
    }

    #[test]
    fn transaction_status_rejects_unrelated_missed_hash() {
        let response = get_transactions_response(vec![hex::encode([2; 32])], vec![]);

        assert!(transaction_status_from_response(response, [1; 32]).is_err());
    }

    #[test]
    fn transaction_status_rejects_found_and_missed_hashes() {
        let requested_hash = [1; 32];
        let response = get_transactions_response(
            vec![hex::encode(requested_hash)],
            vec![transaction_info(Some(requested_hash))],
        );

        assert!(transaction_status_from_response(response, requested_hash).is_err());
    }

    #[test]
    fn transaction_status_rejects_duplicate_found_or_missed_hashes() {
        let requested_hash = [1; 32];
        let duplicate_found = get_transactions_response(
            vec![],
            vec![
                transaction_info(Some(requested_hash)),
                transaction_info(Some(requested_hash)),
            ],
        );
        let duplicate_missed = get_transactions_response(
            vec![hex::encode(requested_hash), hex::encode(requested_hash)],
            vec![],
        );

        assert!(transaction_status_from_response(duplicate_found, requested_hash).is_err());
        assert!(transaction_status_from_response(duplicate_missed, requested_hash).is_err());
    }

    #[test]
    fn transaction_status_rejects_substituted_or_missing_hash() {
        let substituted = get_transactions_response(vec![], vec![transaction_info(Some([2; 32]))]);
        let missing = get_transactions_response(vec![], vec![transaction_info(None)]);

        assert!(transaction_status_from_response(substituted, [1; 32]).is_err());
        assert!(transaction_status_from_response(missing, [1; 32]).is_err());
    }

    #[test]
    fn mempool_transactions_require_hashes_in_requested_order() {
        let requested_hashes = [[1; 32], [2; 32]];
        let transactions = vec![
            transaction_info(Some(requested_hashes[1])),
            transaction_info(Some(requested_hashes[0])),
        ];

        assert!(validate_mempool_transaction_hashes(&transactions, &requested_hashes).is_err());
    }

    #[test]
    fn mempool_transactions_reject_duplicate_substitution() {
        let requested_hashes = [[1; 32], [2; 32]];
        let transactions = vec![
            transaction_info(Some(requested_hashes[0])),
            transaction_info(Some(requested_hashes[0])),
        ];

        assert!(validate_mempool_transaction_hashes(&transactions, &requested_hashes).is_err());
    }

    #[test]
    fn mempool_transactions_preserve_duplicate_request_semantics() {
        let requested_hashes = [[1; 32], [1; 32]];
        let exact_transactions = vec![
            transaction_info(Some(requested_hashes[0])),
            transaction_info(Some(requested_hashes[1])),
        ];
        let missing_transaction = vec![transaction_info(Some(requested_hashes[0]))];

        assert!(
            validate_mempool_transaction_hashes(&exact_transactions, &requested_hashes).is_ok()
        );
        assert!(
            validate_mempool_transaction_hashes(&missing_transaction, &requested_hashes).is_err()
        );
    }

    #[test]
    fn mempool_transactions_reject_missing_hash() {
        let transactions = vec![transaction_info(None)];

        assert!(validate_mempool_transaction_hashes(&transactions, &[[1; 32]]).is_err());
    }

    fn get_transactions_response(
        missed_tx: Vec<String>,
        txs: Vec<monerod::TransactionInfo>,
    ) -> monerod::GetTransactionsResponse {
        monerod::GetTransactionsResponse {
            status: "OK".to_string(),
            untrusted: false,
            missed_tx,
            txs,
        }
    }

    fn transaction_info(tx_hash: Option<[u8; 32]>) -> monerod::TransactionInfo {
        monerod::TransactionInfo {
            tx_hash: tx_hash.map(hex::encode),
            pruned_as_hex: None,
            block_height: None,
            in_pool: true,
        }
    }
}
