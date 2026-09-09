use monero_interface::{InterfaceError, ProvidesBlockchainMeta, ProvidesScannableBlocks};
use monero_oxide::ed25519::{Point, Scalar};
use monero_oxide_wallet::{ScanError, Scanner, ViewPair, ViewPairError};
use zeroize::Zeroizing;

use crate::retry::with_retry;
use crate::rpc::{MempoolTransactionsError, ProvidesMempoolTransactions};
use crate::util::create_scannable_block_for_tx;

const BLOCKS_PER_BATCH: usize = 10;
const MEMPOOL_TXS_PER_BATCH: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum EmptyError {
    #[error(
        "Invalid block range: start height {start_height} is greater than end height {end_height}"
    )]
    InvalidRange {
        start_height: usize,
        end_height: usize,
    },
    // Handled internally by restarting scan
    #[error("Reorganization detected while scanning blocks")]
    ReorgDetected,
    #[error("Interface error: {0}")]
    Interface(#[from] InterfaceError),
    #[error("Mempool transaction error: {0}")]
    Mempool(#[from] MempoolTransactionsError),
    #[error("Scan error: {0}")]
    Scan(#[from] ScanError),
    #[error("Failed to create view pair: {0}")]
    ViewPair(#[from] ViewPairError),
}

#[derive(Debug)]
struct ScanOutcome {
    /// Hash of the currently scanned block. If
    /// found_funds is false we continue to the tip, therefore this is
    /// equivalent to the tip hash.
    /// If found funds is true, this is the hash of the block that contains
    /// the funds.
    pub current_block_hash: [u8; 32],
    pub found_funds: bool,
}

/// Scan the wallet for incoming funds both on chain and in mempool.
///
/// If `target_tip` is set, scans exactly through that block and then scans the mempool. Otherwise,
/// continues to scan until the currently latest block is scanned and then the mempool.
pub async fn has_received_outputs<P>(
    provider: &P,
    public_spend_key: Point,
    private_view_key: Zeroizing<Scalar>,
    start_height: usize,
    target_tip: Option<usize>,
    inner_retry: Option<backoff::ExponentialBackoff>,
) -> Result<bool, EmptyError>
where
    P: ProvidesBlockchainMeta + ProvidesScannableBlocks + ProvidesMempoolTransactions,
{
    let mut end_height = match target_tip {
        Some(end_height) => end_height,
        None => latest_block_number(provider, inner_retry.clone()).await?,
    };

    if start_height > end_height {
        return Err(EmptyError::InvalidRange {
            start_height,
            end_height,
        });
    }

    let view_pair = ViewPair::new(public_spend_key, private_view_key)?;
    let mut scanner = Scanner::new(view_pair.clone());
    let mut next_height = start_height;
    let mut previous_hash = None;

    loop {
        let outcome = match scan_blocks(
            provider,
            &mut scanner,
            &mut next_height,
            end_height,
            &mut previous_hash,
            inner_retry.clone(),
        )
        .await
        {
            Ok(result) => result,
            Err(EmptyError::ReorgDetected) => {
                scanner = Scanner::new(view_pair.clone());
                next_height = start_height;
                previous_hash = None;
                continue;
            }
            Err(error) => return Err(error),
        };

        if outcome.found_funds {
            return Ok(true);
        }

        if scan_mempool(provider, &mut scanner, inner_retry.clone()).await? {
            return Ok(true);
        }

        let refreshed_tip = match target_tip {
            Some(target_tip) => target_tip,
            None => latest_block_number(provider, inner_retry.clone()).await?,
        };

        if start_height > refreshed_tip {
            return Err(EmptyError::InvalidRange {
                start_height,
                end_height: refreshed_tip,
            });
        }

        if refreshed_tip >= end_height {
            let canonical_hash = block_hash(provider, end_height, inner_retry.clone()).await?;
            if outcome.current_block_hash == canonical_hash {
                if refreshed_tip == end_height {
                    return Ok(false);
                }

                end_height = refreshed_tip;
                continue;
            }
        }

        // The scanned checkpoint is no longer canonical, or the chain is now shorter. Start over
        // because continuity within each provider batch alone does not connect separate batches.
        scanner = Scanner::new(view_pair.clone());
        next_height = start_height;
        previous_hash = None;
        end_height = refreshed_tip;
    }
}

async fn latest_block_number<P>(
    provider: &P,
    inner_retry: Option<backoff::ExponentialBackoff>,
) -> Result<usize, EmptyError>
where
    P: ProvidesBlockchainMeta,
{
    Ok(with_retry(
        inner_retry,
        "Received-output latest-block-number lookup",
        || async { provider.latest_block_number().await },
    )
    .await?)
}

async fn block_hash<P>(
    provider: &P,
    height: usize,
    inner_retry: Option<backoff::ExponentialBackoff>,
) -> Result<[u8; 32], EmptyError>
where
    P: ProvidesScannableBlocks,
{
    let block = with_retry(
        inner_retry,
        "Received-output canonical-block fetch",
        || async { provider.scannable_block_by_number(height).await },
    )
    .await?;

    Ok(block.block.hash())
}

/// Returns the last scanned block hash and whether funds were found.
async fn scan_blocks<P>(
    provider: &P,
    scanner: &mut Scanner,
    next_height: &mut usize,
    end_height: usize,
    previous_hash: &mut Option<[u8; 32]>,
    inner_retry: Option<backoff::ExponentialBackoff>,
) -> Result<ScanOutcome, EmptyError>
where
    P: ProvidesScannableBlocks,
{
    loop {
        let end = next_height
            .saturating_add(BLOCKS_PER_BATCH.saturating_sub(1))
            .min(end_height);
        let start = *next_height;

        let blocks = with_retry(
            inner_retry.clone(),
            "Received-output scannable-block fetch",
            || async { provider.contiguous_scannable_blocks(start..=end).await },
        )
        .await?;

        let expected_count = end + 1 - start;
        if blocks.len() != expected_count {
            return Err(InterfaceError::InvalidInterface(format!(
                "Received {} scannable blocks for {start}..={end}, expected {expected_count}",
                blocks.len()
            ))
            .into());
        }

        for (offset, block) in blocks.into_iter().enumerate() {
            let expected_height = start + offset;
            if block.block.number() != expected_height {
                return Err(InterfaceError::InvalidInterface(format!(
                    "Received scannable block {} at height {expected_height}",
                    block.block.number()
                ))
                .into());
            }

            if previous_hash.is_some_and(|hash| block.block.header.previous != hash) {
                return Err(EmptyError::ReorgDetected);
            }

            let last_scanned_block_hash = block.block.hash();
            *previous_hash = Some(last_scanned_block_hash);
            if !scanner.scan(block)?.ignore_additional_timelock().is_empty() {
                return Ok(ScanOutcome {
                    current_block_hash: last_scanned_block_hash,
                    found_funds: true,
                });
            }
        }

        *next_height = end.saturating_add(1);
        if end == end_height {
            return Ok(ScanOutcome {
                current_block_hash: previous_hash.expect("the non-empty batch set a block hash"),
                found_funds: false,
            });
        }
    }
}

async fn scan_mempool<P>(
    provider: &P,
    scanner: &mut Scanner,
    inner_retry: Option<backoff::ExponentialBackoff>,
) -> Result<bool, EmptyError>
where
    P: ProvidesMempoolTransactions,
{
    let mempool_tx_hashes = with_retry(
        inner_retry.clone(),
        "Received-output mempool transaction hash fetch",
        || async { provider.mempool_transaction_hashes().await },
    )
    .await?;

    for batch in mempool_tx_hashes.chunks(MEMPOOL_TXS_PER_BATCH) {
        let mempool_txs = with_retry(
            inner_retry.clone(),
            "Received-output mempool transaction fetch",
            || async { provider.mempool_transactions(batch).await },
        )
        .await?;

        for mempool_tx in mempool_txs {
            let block = create_scannable_block_for_tx(vec![(mempool_tx.tx_id, mempool_tx.tx)]);
            if !scanner.scan(block)?.ignore_additional_timelock().is_empty() {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::ops::RangeInclusive;
    use std::sync::Mutex;

    use monero_interface::ScannableBlock;
    use monero_oxide::block::{Block, BlockHeader};
    use monero_oxide::transaction::{Input, Pruned, Timelock, Transaction, TransactionPrefix};

    use super::*;
    use crate::rpc::MempoolTransaction;
    use crate::util::public_key;

    const SPEND_KEY: &str = "ccf0ea10e1ea64354f42fa710c2b318e581969cf49046d809d1f0aadb3fc7a02";
    const VIEW_KEY: &str = "a28b4b2085592881df94ee95da332c16b5bb773eb8bb74730208cbb236c73806";

    #[rustfmt::skip]
    const RECEIVED_TX: &str = "020001020003060101cf60390bb71aa15eb24037772012d59dc68cb4b6211e1c93206db09a6c346261020002ee8ca293511571c0005e1c144e49d09b8ff03046dbafb3e064a34cb9fc1994b600029e2e5cd08c8681dbcf2ce66071467e835f7e86613fbfed3c4fb170127b94e1072c01d3ce2a622c6e06ed465f81017dd6188c3a6e3d8e65a846f9c98416da0e150a82020901c553d35e54111bd001e0bbcbf289d701ce90e309ead2b487ec1d4d8af5d649543eb99a7620f6b54e532898527be29704f050e6f06de61e5967b2ddd506b4d6d36546065d6aae156ac7bec18c99580c07867fb98cb29853edbafec91af2df605c12f9aaa81a9165625afb6649f5a652012c5ba6612351140e1fb4a8463cc765d0a9bb7d999ba35750f365c5285d77230b76c7a612784f4845812a2899f2ca6a304fee61362db59b263115c27d2ce78af6b1d9e939c1f4036c7707851f41abe6458cf1c748353e593469ebf43536a939f7";

    #[derive(Default)]
    struct MockChain {
        transactions: BTreeMap<usize, Vec<MempoolTransaction>>,
        mine_before_next_mempool_snapshot: Option<(usize, MempoolTransaction)>,
        mine_before_range: Option<(usize, usize, MempoolTransaction)>,
    }

    #[derive(Default)]
    struct MockProvider {
        latest: Mutex<VecDeque<usize>>,
        fail_block_fetch: bool,
        requested_ranges: Mutex<Vec<RangeInclusive<usize>>>,
        mempool_hash_calls: Mutex<usize>,
        mempool_transactions: Mutex<Vec<MempoolTransaction>>,
        chain: Mutex<MockChain>,
    }

    impl ProvidesBlockchainMeta for MockProvider {
        async fn latest_block_number(&self) -> Result<usize, InterfaceError> {
            self.latest
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| InterfaceError::InternalError("unexpected tip lookup".to_owned()))
        }
    }

    impl ProvidesScannableBlocks for MockProvider {
        async fn contiguous_scannable_blocks(
            &self,
            range: RangeInclusive<usize>,
        ) -> Result<Vec<ScannableBlock>, InterfaceError> {
            self.requested_ranges.lock().unwrap().push(range.clone());
            if self.fail_block_fetch {
                return Err(InterfaceError::InternalError(
                    "block fetch failed".to_owned(),
                ));
            }
            let mut chain = self.chain.lock().unwrap();
            if chain
                .mine_before_range
                .as_ref()
                .is_some_and(|(request_start, _, _)| request_start == range.start())
            {
                let (_, height, transaction) = chain.mine_before_range.take().unwrap();
                chain
                    .transactions
                    .entry(height)
                    .or_default()
                    .push(transaction);
            }
            Ok(mock_chain_blocks(&chain, range))
        }

        async fn scannable_block(&self, _hash: [u8; 32]) -> Result<ScannableBlock, InterfaceError> {
            panic!("unexpected block-by-hash lookup")
        }

        async fn scannable_block_by_number(
            &self,
            number: usize,
        ) -> Result<ScannableBlock, InterfaceError> {
            Ok(
                mock_chain_blocks(&self.chain.lock().unwrap(), number..=number)
                    .pop()
                    .unwrap(),
            )
        }
    }

    impl ProvidesMempoolTransactions for MockProvider {
        async fn mempool_transaction_hashes(
            &self,
        ) -> Result<Vec<[u8; 32]>, MempoolTransactionsError> {
            *self.mempool_hash_calls.lock().unwrap() += 1;

            let mined_tx_id = {
                let mut chain = self.chain.lock().unwrap();
                chain
                    .mine_before_next_mempool_snapshot
                    .take()
                    .map(|(height, transaction)| {
                        let tx_id = transaction.tx_id;
                        chain
                            .transactions
                            .entry(height)
                            .or_default()
                            .push(transaction);
                        tx_id
                    })
            };

            let mut mempool = self.mempool_transactions.lock().unwrap();
            if let Some(mined_tx_id) = mined_tx_id {
                mempool.retain(|transaction| transaction.tx_id != mined_tx_id);
            }
            Ok(mempool.iter().map(|tx| tx.tx_id).collect())
        }

        async fn mempool_transactions(
            &self,
            _hashes: &[[u8; 32]],
        ) -> Result<Vec<MempoolTransaction>, MempoolTransactionsError> {
            Ok(self.mempool_transactions.lock().unwrap().clone())
        }
    }

    fn mock_chain_blocks(chain: &MockChain, range: RangeInclusive<usize>) -> Vec<ScannableBlock> {
        let mut previous = [0; 32];
        let mut requested = Vec::new();

        for height in 0..=*range.end() {
            let transactions = chain.transactions.get(&height).cloned().unwrap_or_default();
            let (txids, transactions) = transactions
                .into_iter()
                .map(|transaction| (transaction.tx_id, transaction.tx))
                .unzip();
            let miner_transaction = Transaction::V1 {
                prefix: TransactionPrefix {
                    additional_timelock: Timelock::None,
                    inputs: vec![Input::Gen(height)],
                    outputs: vec![],
                    extra: vec![],
                },
                signatures: Vec::new(),
            };
            let block = Block::new(
                BlockHeader {
                    hardfork_version: crate::HARDFORK_VERSION,
                    hardfork_signal: 0,
                    timestamp: 0,
                    previous,
                    nonce: 0,
                },
                miner_transaction,
                txids,
            )
            .unwrap();
            previous = block.hash();

            if range.contains(&height) {
                requested.push(ScannableBlock {
                    block,
                    transactions,
                    output_index_for_first_ringct_output: Some(0),
                });
            }
        }

        requested
    }

    #[tokio::test]
    async fn scan_blocks_returns_last_hash_and_found_funds() {
        for found_funds_expected in [false, true] {
            let provider = MockProvider::default();
            if found_funds_expected {
                provider
                    .chain
                    .lock()
                    .unwrap()
                    .transactions
                    .insert(3, vec![received_transaction()]);
            }
            let (public_spend_key, private_view_key) = view_pair_keys();
            let mut scanner =
                Scanner::new(ViewPair::new(public_spend_key, private_view_key).unwrap());
            let mut next_height = 0;
            let mut previous_hash = None;

            let outcome = scan_blocks(
                &provider,
                &mut scanner,
                &mut next_height,
                12,
                &mut previous_hash,
                None,
            )
            .await
            .unwrap();

            let last_scanned_height = if found_funds_expected { 3 } else { 12 };
            assert_eq!(outcome.found_funds, found_funds_expected);
            assert_eq!(
                outcome.current_block_hash,
                block_hash(&provider, last_scanned_height, None)
                    .await
                    .unwrap()
            );
            assert_eq!(previous_hash, Some(outcome.current_block_hash));
            assert_eq!(
                *provider.requested_ranges.lock().unwrap(),
                if found_funds_expected {
                    vec![0..=9]
                } else {
                    vec![0..=9, 10..=12]
                }
            );
        }
    }

    #[tokio::test]
    async fn scan_blocks_reports_reorg_only_for_continuity_mismatch() {
        for fail_block_fetch in [false, true] {
            let provider = MockProvider {
                fail_block_fetch,
                ..MockProvider::default()
            };
            let (public_spend_key, private_view_key) = view_pair_keys();
            let mut scanner =
                Scanner::new(ViewPair::new(public_spend_key, private_view_key).unwrap());
            let mut next_height = 1;
            let mut previous_hash = Some([255; 32]);

            let error = scan_blocks(
                &provider,
                &mut scanner,
                &mut next_height,
                1,
                &mut previous_hash,
                None,
            )
            .await
            .unwrap_err();

            if fail_block_fetch {
                assert!(matches!(error, EmptyError::Interface(_)));
            } else {
                assert!(matches!(error, EmptyError::ReorgDetected));
            }
            assert_eq!(next_height, 1);
            assert_eq!(previous_hash, Some([255; 32]));
        }
    }

    #[tokio::test]
    async fn received_output_scan_propagates_block_fetch_errors_without_restart() {
        let provider = MockProvider {
            fail_block_fetch: true,
            ..MockProvider::default()
        };
        let (public_spend_key, private_view_key) = view_pair_keys();

        let error = has_received_outputs(
            &provider,
            public_spend_key,
            private_view_key,
            0,
            Some(1),
            None,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, EmptyError::Interface(_)));
        assert_eq!(*provider.requested_ranges.lock().unwrap(), vec![0..=1]);
        assert_eq!(*provider.mempool_hash_calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn fixed_tip_scans_exactly_through_target_without_tip_lookup() {
        let provider = MockProvider::default();
        let (public_spend_key, private_view_key) = view_pair_keys();

        let received = has_received_outputs(
            &provider,
            public_spend_key,
            private_view_key,
            7,
            Some(18),
            None,
        )
        .await
        .unwrap();

        assert!(!received);
        assert_eq!(
            *provider.requested_ranges.lock().unwrap(),
            vec![7..=16, 17..=18]
        );
        assert_eq!(*provider.mempool_hash_calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn live_scan_detects_output_mined_during_mempool_transition() {
        let transaction = received_transaction();
        let provider = MockProvider {
            latest: Mutex::new(VecDeque::from([10, 11])),
            mempool_transactions: Mutex::new(vec![transaction.clone()]),
            chain: Mutex::new(MockChain {
                mine_before_next_mempool_snapshot: Some((11, transaction)),
                ..MockChain::default()
            }),
            ..MockProvider::default()
        };
        let (public_spend_key, private_view_key) = view_pair_keys();

        let received = has_received_outputs(
            &provider,
            public_spend_key,
            private_view_key,
            10,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(received);
        assert_eq!(
            *provider.requested_ranges.lock().unwrap(),
            vec![10..=10, 11..=11]
        );
        assert_eq!(*provider.mempool_hash_calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn live_scan_catches_up_after_more_than_three_advancing_rounds() {
        let provider = MockProvider {
            latest: Mutex::new(VecDeque::from([10, 11, 12, 13, 14, 15, 15])),
            ..MockProvider::default()
        };
        let (public_spend_key, private_view_key) = view_pair_keys();

        let received = has_received_outputs(
            &provider,
            public_spend_key,
            private_view_key,
            10,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(!received);
        assert_eq!(
            *provider.requested_ranges.lock().unwrap(),
            vec![10..=10, 11..=11, 12..=12, 13..=13, 14..=14, 15..=15]
        );
        assert_eq!(*provider.mempool_hash_calls.lock().unwrap(), 6);
    }

    #[tokio::test]
    async fn cross_batch_reorg_restarts_and_detects_output() {
        let provider = MockProvider {
            chain: Mutex::new(MockChain {
                mine_before_range: Some((10, 9, received_transaction())),
                ..MockChain::default()
            }),
            ..MockProvider::default()
        };
        let (public_spend_key, private_view_key) = view_pair_keys();

        let received = has_received_outputs(
            &provider,
            public_spend_key,
            private_view_key,
            0,
            Some(10),
            None,
        )
        .await
        .unwrap();

        assert!(received);
        assert_eq!(
            *provider.requested_ranges.lock().unwrap(),
            vec![0..=9, 10..=10, 0..=9]
        );
    }

    #[tokio::test]
    async fn same_height_reorg_restarts_and_detects_output() {
        let provider = MockProvider {
            latest: Mutex::new(VecDeque::from([10, 10])),
            chain: Mutex::new(MockChain {
                mine_before_next_mempool_snapshot: Some((10, received_transaction())),
                ..MockChain::default()
            }),
            ..MockProvider::default()
        };
        let (public_spend_key, private_view_key) = view_pair_keys();

        let received = has_received_outputs(
            &provider,
            public_spend_key,
            private_view_key,
            10,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(received);
        assert_eq!(
            *provider.requested_ranges.lock().unwrap(),
            vec![10..=10, 10..=10]
        );
    }

    #[tokio::test]
    async fn same_height_reorg_restarts_and_returns_empty_for_unrelated_wallet() {
        let provider = MockProvider {
            latest: Mutex::new(VecDeque::from([10, 10, 10])),
            chain: Mutex::new(MockChain {
                mine_before_next_mempool_snapshot: Some((10, received_transaction())),
                ..MockChain::default()
            }),
            ..MockProvider::default()
        };
        let (_, private_view_key) = view_pair_keys();

        let received = has_received_outputs(
            &provider,
            public_key(&private_view_key),
            private_view_key,
            10,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(!received);
        assert_eq!(
            *provider.requested_ranges.lock().unwrap(),
            vec![10..=10, 10..=10]
        );
        assert_eq!(*provider.mempool_hash_calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn timelocked_chain_output_counts_as_received() {
        let mut transaction = received_transaction();
        transaction.tx.prefix_mut().additional_timelock = Timelock::Block(usize::MAX);
        let provider = MockProvider {
            chain: Mutex::new(MockChain {
                transactions: BTreeMap::from([(10, vec![transaction])]),
                ..MockChain::default()
            }),
            ..MockProvider::default()
        };
        let (public_spend_key, private_view_key) = view_pair_keys();

        let received = has_received_outputs(
            &provider,
            public_spend_key,
            private_view_key,
            10,
            Some(10),
            None,
        )
        .await
        .unwrap();

        assert!(received);
    }

    #[tokio::test]
    async fn timelocked_mempool_output_counts_as_received() {
        let mut transaction = received_transaction();
        transaction.tx.prefix_mut().additional_timelock = Timelock::Block(usize::MAX);
        let provider = MockProvider {
            mempool_transactions: Mutex::new(vec![transaction]),
            ..MockProvider::default()
        };
        let (public_spend_key, private_view_key) = view_pair_keys();

        let received = has_received_outputs(
            &provider,
            public_spend_key,
            private_view_key,
            10,
            Some(10),
            None,
        )
        .await
        .unwrap();

        assert!(received);
    }

    fn received_transaction() -> MempoolTransaction {
        let tx =
            Transaction::<Pruned>::read(&mut hex::decode(RECEIVED_TX).unwrap().as_slice()).unwrap();
        MempoolTransaction { tx_id: [1; 32], tx }
    }

    fn view_pair_keys() -> (Point, Zeroizing<Scalar>) {
        let spend_key = Scalar::read(&mut hex::decode(SPEND_KEY).unwrap().as_slice()).unwrap();
        let view_key = Scalar::read(&mut hex::decode(VIEW_KEY).unwrap().as_slice()).unwrap();
        (public_key(&spend_key), Zeroizing::new(view_key))
    }
}
