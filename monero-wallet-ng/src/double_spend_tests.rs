use super::*;
use monero_interface::InterfaceError;
use monero_oxide::{
    block::{Block, BlockHeader},
    ed25519::CompressedPoint,
};
use monero_oxide_wallet::transaction::{Pruned, Timelock, Transaction, TransactionPrefix};
use std::ops::RangeInclusive;

const CONFLICT: [u8; 32] = [1; 32];
const ORIGINAL: [u8; 32] = [2; 32];

fn block(height: usize, conflict: bool) -> ScannableBlock {
    let prefix = TransactionPrefix {
        additional_timelock: Timelock::None,
        inputs: vec![Input::Gen(height)],
        outputs: vec![],
        extra: vec![],
    };
    let transaction = Transaction::<Pruned>::V1 {
        prefix: TransactionPrefix {
            inputs: vec![Input::ToKey {
                amount: None,
                key_offsets: vec![1],
                key_image: CompressedPoint::G,
            }],
            ..prefix.clone()
        },
        signatures: (),
    };
    ScannableBlock {
        block: Block::new(
            BlockHeader {
                hardfork_version: crate::HARDFORK_VERSION,
                hardfork_signal: 0,
                timestamp: 0,
                previous: [0; 32],
                nonce: 0,
            },
            Transaction::V1 {
                prefix,
                signatures: vec![],
            },
            if conflict { vec![CONFLICT] } else { vec![] },
        )
        .unwrap(),
        transactions: if conflict { vec![transaction] } else { vec![] },
        output_index_for_first_ringct_output: Some(0),
    }
}

// Only the RPC methods used by the search are supplied; no wallet or blockchain process.
struct Blocks {
    tip: usize,
    conflict_height: usize,
    replaced: bool,
    fail_fetch: bool,
}

impl ProvidesBlockchainMeta for Blocks {
    async fn latest_block_number(&self) -> Result<usize, InterfaceError> {
        Ok(self.tip)
    }
}
impl ProvidesScannableBlocks for Blocks {
    async fn contiguous_scannable_blocks(
        &self,
        range: RangeInclusive<usize>,
    ) -> Result<Vec<ScannableBlock>, InterfaceError> {
        if self.fail_fetch {
            return Err(InterfaceError::InvalidInterface("fetch failed".into()));
        }
        assert!(*range.end() <= self.tip);
        Ok(range
            .map(|height| block(height, height == self.conflict_height))
            .collect())
    }
    async fn scannable_block_by_number(
        &self,
        height: usize,
    ) -> Result<ScannableBlock, InterfaceError> {
        Ok(block(
            height,
            height == self.conflict_height && !self.replaced,
        ))
    }
    async fn scannable_block(&self, _: [u8; 32]) -> Result<ScannableBlock, InterfaceError> {
        panic!("not used")
    }
}

#[tokio::test]
async fn requires_exact_confirmation_depth() {
    for (tip, expected) in [(18, false), (19, true), (20, true)] {
        let provider = Blocks {
            tip,
            conflict_height: 10,
            replaced: false,
            fail_fetch: false,
        };
        assert_eq!(
            has_confirmed_conflict(&provider, ORIGINAL, &[CompressedPoint::G.to_bytes()], 0, 10)
                .await
                .unwrap(),
            expected
        );
    }
}

#[tokio::test]
async fn excludes_original_transaction_and_unrelated_inputs() {
    let provider = Blocks {
        tip: 19,
        conflict_height: 10,
        replaced: false,
        fail_fetch: false,
    };
    assert!(
        !has_confirmed_conflict(&provider, CONFLICT, &[CompressedPoint::G.to_bytes()], 0, 10)
            .await
            .unwrap()
    );
    assert!(
        !has_confirmed_conflict(&provider, ORIGINAL, &[[9; 32]], 0, 10)
            .await
            .unwrap()
    );
    assert!(
        has_confirmed_conflict(
            &provider,
            ORIGINAL,
            &[[9; 32], CompressedPoint::G.to_bytes()],
            0,
            10
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn rejects_reorganized_conflict_and_propagates_rpc_failure() {
    let mut provider = Blocks {
        tip: 19,
        conflict_height: 10,
        replaced: true,
        fail_fetch: false,
    };
    assert!(
        !has_confirmed_conflict(&provider, ORIGINAL, &[CompressedPoint::G.to_bytes()], 0, 10)
            .await
            .unwrap()
    );
    provider.fail_fetch = true;
    assert!(
        has_confirmed_conflict(&provider, ORIGINAL, &[CompressedPoint::G.to_bytes()], 0, 10)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn respects_restore_height_and_rejects_zero_requirement() {
    let provider = Blocks {
        tip: 19,
        conflict_height: 10,
        replaced: false,
        fail_fetch: false,
    };
    assert!(
        !has_confirmed_conflict(
            &provider,
            ORIGINAL,
            &[CompressedPoint::G.to_bytes()],
            11,
            10
        )
        .await
        .unwrap()
    );
    assert!(
        has_confirmed_conflict(&provider, ORIGINAL, &[CompressedPoint::G.to_bytes()], 0, 0)
            .await
            .is_err()
    );
}
