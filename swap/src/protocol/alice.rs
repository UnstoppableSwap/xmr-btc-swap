//! Run an XMR/BTC swap in the role of Alice.
//! Alice holds XMR and wishes receive BTC.
use crate::protocol::Database;
pub use crate::protocol::alice::swap::*;
use crate::{asb, monero};
use bitcoin_wallet::BitcoinWallet;
use rust_decimal::Decimal;
use std::sync::Arc;
use swap_env::env::Config;
pub use swap_machine::alice::*;
use uuid::Uuid;

pub mod lock_phase;
pub mod swap;

pub struct Swap {
    pub state: AliceState,
    pub event_loop_handle: asb::EventLoopHandle,
    pub bitcoin_wallet: Arc<dyn BitcoinWallet>,
    pub monero_wallet: Arc<monero::Wallets>,
    pub env_config: Config,
    pub developer_tip: TipConfig,
    pub hermes_funding_policy: HermesFundingPolicy,
    pub swap_id: Uuid,
    pub db: Arc<dyn Database + Send + Sync>,
}

/// How the maker funds Bob's on-chain Hermes encrypted-signature channel: the
/// extra Monero output attached to the lock transaction. Funding is skipped
/// when the channel is disabled or when the swap is below `min_swap_amount`.
#[derive(Clone, Copy, Debug)]
pub struct HermesFundingPolicy {
    pub enabled: bool,
    pub amount: monero::Amount,
    pub min_swap_amount: bitcoin::Amount,
}

impl HermesFundingPolicy {
    /// The Hermes funding amount for a swap of `swap_btc_amount`, or zero when
    /// the channel is disabled or the swap is below the minimum size.
    pub fn funding_amount(&self, swap_btc_amount: bitcoin::Amount) -> monero::Amount {
        if self.enabled && swap_btc_amount >= self.min_swap_amount {
            self.amount
        } else {
            monero::Amount::ZERO
        }
    }
}

/// Configures how much the and where the user wants to send tips to
///
/// The ratio is a number between 0 and 1
///
/// ratio = 0 means that no tip will be sent
/// ratio = 0.5 means that for a swap of 1 XMR, 0.5 XMR will be tipped
#[derive(Clone)]
pub struct TipConfig {
    pub ratio: Decimal,
    pub address: monero_address::MoneroAddress,
}
