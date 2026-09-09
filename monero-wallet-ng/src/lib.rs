pub mod confirmations;
pub mod double_spend;
pub mod empty;
pub mod hermes;
pub mod retry;
pub mod rpc;
pub mod scanner;
pub mod sweep;
pub mod util;
pub mod verify;

pub const HARDFORK_VERSION: u8 = 16;

/// Ring size for CLSAG Bulletproofs+ transactions.
///
/// Matches the value hardcoded inside `monero-oxide` for
/// `RctType::ClsagBulletproofPlus` (see `send/mod.rs` and `send/tx.rs`), which
/// is the protocol-required ring length for the current hard fork. There is
/// no exported constant upstream, so we mirror the magic number here.
pub const RING_LEN: u8 = 16;

/// Upper bound (in piconero per transaction weight unit) we accept from the
/// daemon's dynamic fee estimate.
///
/// Mainnet's fee estimate sits well under 100k pico/weight; regtest has been
/// observed returning ~1.2M pico/weight. This cap is loose enough to
/// accommodate regtest while still catching a daemon that reports an
/// obviously-wrong estimate. `wallet2` has no equivalent constant — it trusts
/// whatever the daemon returns.
pub const MAX_FEE_PER_WEIGHT: u64 = 10_000_000;
