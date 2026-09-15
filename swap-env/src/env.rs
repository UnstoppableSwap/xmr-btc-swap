use crate::config::Config as AsbConfig;
use serde::Serialize;
use std::cmp::max;
use std::time::Duration;
use time::ext::NumericalStdDuration;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
pub struct Config {
    pub bitcoin_lock_mempool_timeout: Duration,
    pub bitcoin_lock_confirmed_timeout: Duration,
    pub bitcoin_finality_confirmations: u32,
    /// The upper bound for the number of blocks that will be mined before our
    /// Bitcoin transaction is included in a block
    pub bitcoin_blocks_till_confirmed_upper_bound_assumption: u32,
    pub bitcoin_avg_block_time: Duration,
    pub bitcoin_cancel_timelock: u32,
    pub bitcoin_punish_timelock: u32,
    pub bitcoin_remaining_refund_timelock: u32,
    pub bitcoin_network: bitcoin::Network,
    pub monero_avg_block_time: Duration,
    pub monero_finality_confirmations: u64,
    // If Alice does manage to lock her Monero within this timeout, she will initiate an early refund of the Bitcoin.
    pub monero_lock_retry_timeout: Duration,
    // After this many confirmations we assume that the Monero transaction is safe from double spending
    pub monero_double_spend_safe_confirmations: u64,
    // Whether the configured Monero daemon is trusted. You should generally only consider
    // self-hosted Monero nodes on your own hardware as trusted. If an attacker controls your
    // node and you set this to true, they might be able to steal your funds.
    pub monero_trusted_daemon: bool,
    /// Required depth of a conflicting input spend before rebuilding a lock transaction.
    pub monero_lock_rebuild_confirmations: u64,
    #[serde(with = "swap_serde::monero::network")]
    pub monero_network: monero_address::Network,
}

impl Config {
    pub fn bitcoin_sync_interval(&self) -> Duration {
        sync_interval(self.bitcoin_avg_block_time)
    }

    pub fn monero_sync_interval(&self) -> Duration {
        sync_interval(self.monero_avg_block_time)
    }
}

pub trait GetConfig {
    fn get_config() -> Config;
}

#[derive(Clone, Copy)]
pub struct Mainnet;

#[derive(Clone, Copy)]
pub struct Testnet;

#[derive(Clone, Copy)]
pub struct Regtest;

impl GetConfig for Mainnet {
    fn get_config() -> Config {
        Config {
            bitcoin_lock_mempool_timeout: 10.std_minutes(),
            bitcoin_lock_confirmed_timeout: 2.std_hours(),
            bitcoin_finality_confirmations: 1,
            // We assume that a transaction that was constructed to be confirmed within one block
            // will be confirmed within at most 6 blocks
            bitcoin_blocks_till_confirmed_upper_bound_assumption: 6,
            bitcoin_avg_block_time: 10.std_minutes(),
            bitcoin_cancel_timelock: 24,
            bitcoin_punish_timelock: 144,
            bitcoin_remaining_refund_timelock: 2,
            bitcoin_network: bitcoin::Network::Bitcoin,
            monero_avg_block_time: 2.std_minutes(),
            // If Alice cannot lock her Monero within this timeout,
            // she will initiate an early refund of Bobs Bitcoin
            monero_lock_retry_timeout: 10.std_minutes(),
            monero_finality_confirmations: 10,
            monero_double_spend_safe_confirmations: 10,
            monero_trusted_daemon: false,
            monero_lock_rebuild_confirmations: 15,
            monero_network: monero_address::Network::Mainnet,
        }
    }
}

impl GetConfig for Testnet {
    fn get_config() -> Config {
        Config {
            bitcoin_lock_mempool_timeout: 10.std_minutes(),
            bitcoin_lock_confirmed_timeout: 1.std_hours(),
            bitcoin_finality_confirmations: 1,
            bitcoin_blocks_till_confirmed_upper_bound_assumption: 6,
            bitcoin_avg_block_time: 10.std_minutes(),
            bitcoin_cancel_timelock: 12 * 3,
            bitcoin_punish_timelock: 24 * 3,
            bitcoin_remaining_refund_timelock: 2,
            bitcoin_network: bitcoin::Network::Testnet,
            monero_avg_block_time: 2.std_minutes(),
            monero_lock_retry_timeout: 10.std_minutes(),
            monero_finality_confirmations: 10,
            monero_double_spend_safe_confirmations: 10,
            monero_trusted_daemon: false,
            monero_lock_rebuild_confirmations: 15,
            monero_network: monero_address::Network::Stagenet,
        }
    }
}

impl GetConfig for Regtest {
    fn get_config() -> Config {
        Config {
            bitcoin_lock_mempool_timeout: 30.std_seconds(),
            bitcoin_lock_confirmed_timeout: 5.std_minutes(),
            bitcoin_finality_confirmations: 1,
            bitcoin_blocks_till_confirmed_upper_bound_assumption: 6,
            bitcoin_avg_block_time: 5.std_seconds(),
            bitcoin_cancel_timelock: 100,
            bitcoin_punish_timelock: 50,
            bitcoin_remaining_refund_timelock: 5,
            bitcoin_network: bitcoin::Network::Regtest,
            monero_avg_block_time: 1.std_seconds(),
            monero_lock_retry_timeout: 1.std_minutes(),
            monero_finality_confirmations: 10,
            monero_double_spend_safe_confirmations: 10,
            monero_trusted_daemon: false,
            monero_lock_rebuild_confirmations: 15,
            monero_network: monero_address::Network::Mainnet, // yes this is strange
        }
    }
}

fn sync_interval(avg_block_time: Duration) -> Duration {
    max(avg_block_time / 10, Duration::from_secs(1))
}

pub fn new(is_testnet: bool, asb_config: &AsbConfig) -> Config {
    let env_config = if is_testnet {
        Testnet::get_config()
    } else {
        Mainnet::get_config()
    };

    let env_config =
        if let Some(bitcoin_finality_confirmations) = asb_config.bitcoin.finality_confirmations {
            Config {
                bitcoin_finality_confirmations,
                ..env_config
            }
        } else {
            env_config
        };

    let env_config =
        if let Some(monero_finality_confirmations) = asb_config.monero.finality_confirmations {
            Config {
                monero_finality_confirmations,
                ..env_config
            }
        } else {
            env_config
        };

    Config {
        monero_trusted_daemon: asb_config.monero.trusted_daemon,
        monero_lock_rebuild_confirmations: asb_config.monero.lock_rebuild_confirmations,
        ..env_config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_interval_is_one_second_if_avg_blocktime_is_one_second() {
        let interval = sync_interval(Duration::from_secs(1));

        assert_eq!(interval, Duration::from_secs(1))
    }

    #[test]
    fn check_interval_is_tenth_of_avg_blocktime() {
        let interval = sync_interval(Duration::from_secs(100));

        assert_eq!(interval, Duration::from_secs(10))
    }
}
