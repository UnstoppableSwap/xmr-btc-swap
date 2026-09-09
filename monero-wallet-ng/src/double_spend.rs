//! Locate sufficiently confirmed conflicting inputs without wallet-output scanning.

use anyhow::{Context, Result, ensure};
use monero_interface::{ProvidesBlockchainMeta, ProvidesScannableBlocks, ScannableBlock};
use monero_oxide_wallet::transaction::Input;

const BLOCKS_PER_BATCH: usize = 10;

#[cfg(test)]
#[path = "double_spend_tests.rs"]
mod tests;

/// Find any other transaction spending one of `key_images` in sufficiently deep blocks.
/// The caller must first establish that an input is blockchain-spent using the trusted daemon.
/// Absence from the searched range does not establish finality and returns false.
pub async fn has_confirmed_conflict<P>(
    provider: &P,
    original_tx: [u8; 32],
    key_images: &[[u8; 32]],
    restore_height: usize,
    required_confirmations: usize,
) -> Result<bool>
where
    P: ProvidesBlockchainMeta + ProvidesScannableBlocks,
{
    ensure!(
        required_confirmations > 0,
        "Rebuild confirmations must be positive"
    );
    let tip = provider.latest_block_number().await?;
    let Some(last_eligible_height) = tip.checked_sub(required_confirmations - 1) else {
        return Ok(false);
    };

    let mut start = restore_height;
    while start <= last_eligible_height {
        let end = start
            .saturating_add(BLOCKS_PER_BATCH - 1)
            .min(last_eligible_height);
        let blocks = provider.contiguous_scannable_blocks(start..=end).await?;
        ensure!(
            blocks.len() == end - start + 1,
            "Incomplete conflict-search block batch"
        );
        for (offset, block) in blocks.into_iter().enumerate() {
            let height = start + offset;
            ensure!(
                block.block.number() == height,
                "Unexpected conflict-search block height"
            );
            if !contains_conflict(&block, original_tx, key_images)? {
                continue;
            }

            // The search may take time: verify both depth and canonical identity again.
            let current_tip = provider.latest_block_number().await?;
            if current_tip
                .checked_sub(height)
                .is_none_or(|depth| depth < required_confirmations - 1)
            {
                return Ok(false);
            }
            let canonical = provider
                .scannable_block_by_number(height)
                .await
                .context("Failed to recheck conflicting spend's canonical block")?;
            return Ok(canonical.block.hash() == block.block.hash());
        }
        if end == last_eligible_height {
            break;
        }
        start = end + 1;
    }
    Ok(false)
}

fn contains_conflict(
    block: &ScannableBlock,
    original_tx: [u8; 32],
    key_images: &[[u8; 32]],
) -> Result<bool> {
    ensure!(
        block.block.transactions.len() == block.transactions.len(),
        "Incomplete conflict-search transaction list"
    );
    Ok(block.block.transactions.iter().zip(&block.transactions).any(|(hash, tx)| {
        *hash != original_tx && tx.prefix().inputs.iter().any(|input| {
            matches!(input, Input::ToKey { key_image, .. } if key_images.contains(&key_image.to_bytes()))
        })
    }))
}
