use crate::defaults::{
    BITFINEX_PRICE_TICKER_WS_URL, EXOLIX_PRICE_TICKER_REST_URL, GetDefaults,
    KRAKEN_PRICE_TICKER_WS_URL, KUCOIN_PRICE_TICKER_REST_URL,
};
use crate::env::{Mainnet, Testnet};
use crate::prompt;
use anyhow::{Context, Result, bail};
use config::ConfigError;
use libp2p::core::Multiaddr;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use swap_fs::ensure_directory_exists;
use url::Url;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub data: Data,
    pub network: Network,
    pub bitcoin: Bitcoin,
    pub monero: Monero,
    pub tor: TorConf,
    pub maker: Maker,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    pub dir: PathBuf,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    #[serde(deserialize_with = "swap_serde::libp2p::multiaddresses::deserialize")]
    pub listen: Vec<Multiaddr>,
    #[serde(
        default,
        deserialize_with = "swap_serde::libp2p::multiaddresses::deserialize"
    )]
    pub rendezvous_point: Vec<Multiaddr>,
    #[serde(
        default,
        deserialize_with = "swap_serde::libp2p::multiaddresses::deserialize"
    )]
    pub external_addresses: Vec<Multiaddr>,
    /// Port on which to expose libp2p Prometheus metrics over HTTP at
    /// `/metrics`. When unset, the metrics endpoint is disabled.
    #[serde(default)]
    pub prometheus_port: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bitcoin {
    #[serde(deserialize_with = "swap_serde::electrum::urls::deserialize")]
    pub electrum_rpc_urls: Vec<Url>,
    pub target_block: u16,
    pub finality_confirmations: Option<u32>,
    #[serde(with = "swap_serde::bitcoin::network")]
    pub network: bitcoin::Network,
    #[serde(default = "default_use_mempool_space_fee_estimation")]
    pub use_mempool_space_fee_estimation: bool,
}

fn default_use_mempool_space_fee_estimation() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Monero {
    /// If None, we will use the Monero Remote Node Pool with built in defaults
    #[serde(default)]
    pub daemon_url: Option<Url>,
    pub finality_confirmations: Option<u64>,
    #[serde(with = "swap_serde::monero::network")]
    pub network: monero_address::Network,
    /// Whether the configured Monero daemon is trusted. You should generally only
    /// consider self-hosted Monero nodes on your own hardware as trusted. If an
    /// attacker controls your node and you set this to true, they might be able to
    /// steal your funds.
    #[serde(default)]
    // Validated by validate_config: trust requires an explicitly configured daemon_url.
    pub trusted_daemon: bool,
    /// Required confirmations of a conflicting input spend before rebuilding a lock.
    #[serde(default = "default_lock_rebuild_confirmations")]
    pub lock_rebuild_confirmations: u64,
}

fn default_lock_rebuild_confirmations() -> u64 {
    10
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TorConf {
    pub register_hidden_service: bool,
    pub hidden_service_num_intro_points: u8,
    /// Maximum number of concurrent rendezvous circuit constructions.
    /// Limits how fast the PoW priority queue is drained, creating backpressure
    /// that allows the suggested PoW effort to ramp up under load.
    #[serde(default = "default_max_concurrent_rend_requests")]
    pub max_concurrent_rend_requests: usize,
    /// Enable wormholes.
    #[serde(default = "default_wormhole_enabled")]
    pub wormhole_enabled: bool,
    /// Maximum concurrent rendezvous requests per wormhole.
    #[serde(default = "default_wormhole_max_concurrent_rend_requests")]
    pub wormhole_max_concurrent_rend_requests: usize,
    /// Number of introduction points per wormhole onion service.
    #[serde(default = "default_wormhole_num_intro_points")]
    pub wormhole_num_intro_points: u8,
    /// Only swaps whose latest state update occurred within this many hours
    /// are considered when deciding which peers receive a wormhole. Stale
    /// swaps beyond this window are ignored.
    #[serde(default = "default_wormhole_swap_freshness_hours")]
    pub wormhole_swap_freshness_hours: u64,
}

fn default_max_concurrent_rend_requests() -> usize {
    16
}

fn default_wormhole_enabled() -> bool {
    true
}

fn default_wormhole_max_concurrent_rend_requests() -> usize {
    3
}

fn default_wormhole_num_intro_points() -> u8 {
    3
}

fn default_wormhole_swap_freshness_hours() -> u64 {
    7 * 24
}

impl Default for TorConf {
    fn default() -> Self {
        Self {
            register_hidden_service: true,
            hidden_service_num_intro_points: 5,
            max_concurrent_rend_requests: default_max_concurrent_rend_requests(),
            wormhole_enabled: default_wormhole_enabled(),
            wormhole_max_concurrent_rend_requests: default_wormhole_max_concurrent_rend_requests(),
            wormhole_num_intro_points: default_wormhole_num_intro_points(),
            wormhole_swap_freshness_hours: default_wormhole_swap_freshness_hours(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Maker {
    #[serde(with = "::bitcoin::amount::serde::as_btc")]
    pub min_buy_btc: bitcoin::Amount,
    #[serde(with = "::bitcoin::amount::serde::as_btc")]
    pub max_buy_btc: bitcoin::Amount,
    pub ask_spread: Decimal,
    /// What refund conditions to give to takers.
    #[serde(default)]
    pub refund_policy: RefundPolicy,
    #[serde(default = "default_price_ticker_ws_url_kraken")]
    pub price_ticker_ws_url_kraken: Url,
    #[serde(default = "default_price_ticker_ws_url_bitfinex")]
    pub price_ticker_ws_url_bitfinex: Url,
    #[serde(default = "default_price_ticker_rest_url_kucoin")]
    pub price_ticker_rest_url_kucoin: Url,
    #[serde(default = "default_price_ticker_rest_url_exolix")]
    pub price_ticker_rest_url_exolix: Url,
    /// Whether the Kraken price feed contributes to the XMR/BTC rate.
    #[serde(default = "default_price_ticker_source_enabled")]
    pub price_ticker_source_kraken_enabled: bool,
    /// Whether the Bitfinex price feed contributes to the XMR/BTC rate.
    #[serde(default = "default_price_ticker_source_enabled")]
    pub price_ticker_source_bitfinex_enabled: bool,
    /// Whether the KuCoin price feed contributes to the XMR/BTC rate.
    #[serde(default = "default_price_ticker_source_enabled")]
    pub price_ticker_source_kucoin_enabled: bool,
    /// Optional Exolix API key. When set, the Exolix rate endpoint is
    /// polled and included in the price average alongside Kraken,
    /// Bitfinex, and KuCoin.
    #[serde(default)]
    pub price_ticker_source_exolix_api_key: Option<String>,
    /// How often the Exolix REST rate endpoint is polled, in seconds.
    #[serde(default = "default_price_ticker_rest_poll_interval_exolix_secs")]
    pub price_ticker_rest_poll_interval_exolix_secs: u64,
    /// How long a polled/streamed exchange-rate sample remains usable
    /// before it is discarded as stale (in seconds). Applies uniformly
    /// to all feeds (Kraken, Bitfinex, KuCoin, Exolix).
    #[serde(default = "default_price_ticker_validity_duration_secs")]
    pub price_ticker_validity_duration_secs: u64,
    /// If specified, Bitcoin received from successful swaps will be sent to this address.
    #[serde(default, with = "swap_serde::bitcoin::address_serde::option")]
    pub external_bitcoin_redeem_address: Option<bitcoin::Address>,
    /// Multiplier applied to the estimated BTC redeem fee. Defaults to 1.0;
    /// set higher (e.g. 2.0) to overpay for faster confirmation as a safety
    /// margin against fee estimation undershooting actual mempool conditions.
    #[serde(default = "default_btc_redeem_fee_multiplier")]
    pub btc_redeem_fee_multiplier: Decimal,
    /// Percentage (between 0.0 and 1.0) of the swap amount
    /// that will be donated to the devepment fund as part of the Monero lock transaction.
    #[serde(default = "default_developer_tip")]
    pub developer_tip: Decimal,
    /// Amount of Monero (in piconero) attached to the Monero lock transaction to
    /// fund the Hermes transaction.
    #[serde(default = "default_hermes_funding_amount_piconero")]
    pub hermes_funding_amount_piconero: u64,
    /// Whether to fund the on-chain Hermes encrypted-signature channel at all.
    /// Enabled by default.
    #[serde(default = "default_hermes_enabled")]
    pub hermes_enabled: bool,
    /// Minimum swap size (in BTC) below which the Hermes amount is not funded.
    #[serde(
        with = "::bitcoin::amount::serde::as_btc",
        default = "default_hermes_min_swap_amount"
    )]
    pub hermes_min_swap_amount: bitcoin::Amount,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RefundPolicy {
    /// Takers will only receive this percentage of their Bitcoin back by default.
    /// Maker can still issue "amnesty" to refund the rest.
    /// This protects the maker against griefing attacks.
    #[serde(default = "default_anti_spam_deposit_ratio")]
    pub anti_spam_deposit_ratio: Decimal,
    /// If true, Alice will publish TxWithhold after refunding her XMR,
    /// denying Bob access to the amnesty output. Alice can later grant
    /// final amnesty to return the funds to Bob.
    #[serde(skip)]
    pub always_withhold_deposit: bool,
}

impl Default for RefundPolicy {
    fn default() -> Self {
        Self {
            anti_spam_deposit_ratio: default_anti_spam_deposit_ratio(),
            always_withhold_deposit: false,
        }
    }
}

fn default_price_ticker_ws_url_kraken() -> Url {
    Url::parse(KRAKEN_PRICE_TICKER_WS_URL).expect("default kraken ws url to be valid")
}

fn default_price_ticker_ws_url_bitfinex() -> Url {
    Url::parse(BITFINEX_PRICE_TICKER_WS_URL).expect("default bitfinex ws url to be valid")
}

fn default_price_ticker_rest_url_kucoin() -> Url {
    Url::parse(KUCOIN_PRICE_TICKER_REST_URL).expect("default kucoin rest url to be valid")
}

fn default_price_ticker_rest_url_exolix() -> Url {
    Url::parse(EXOLIX_PRICE_TICKER_REST_URL).expect("default exolix rest url to be valid")
}

pub fn default_price_ticker_rest_poll_interval_exolix_secs() -> u64 {
    10
}

pub fn default_price_ticker_source_enabled() -> bool {
    true
}

pub fn default_price_ticker_validity_duration_secs() -> u64 {
    10 * 60
}

fn default_developer_tip() -> Decimal {
    Decimal::ZERO
}

pub fn default_hermes_funding_amount_piconero() -> u64 {
    // 0.0001 XMR, roughly twice the typical network fee of a Hermes transaction
    100_000_000
}

pub fn default_hermes_enabled() -> bool {
    true
}

pub fn default_hermes_min_swap_amount() -> bitcoin::Amount {
    // ~50 USD worth of BTC at a reference price of 50,000 USD/BTC
    bitcoin::Amount::from_btc(0.001).expect("0.001 BTC to be a valid amount")
}

pub fn default_btc_redeem_fee_multiplier() -> Decimal {
    Decimal::ONE
}

fn default_anti_spam_deposit_ratio() -> Decimal {
    Decimal::ZERO
}

impl Config {
    pub fn read<D>(config_file: D) -> Result<Self, ConfigError>
    where
        D: AsRef<OsStr>,
    {
        let config_file = Path::new(&config_file);

        let config = config::Config::builder()
            .add_source(config::File::from(config_file))
            .add_source(
                config::Environment::with_prefix("ASB")
                    .separator("__")
                    .list_separator(","),
            )
            .build()?;

        config.try_into()
    }
}

impl TryFrom<config::Config> for Config {
    type Error = config::ConfigError;

    fn try_from(value: config::Config) -> Result<Self, Self::Error> {
        value.try_deserialize()
    }
}

#[derive(thiserror::Error, Debug, Clone, Copy)]
#[error("config not initialized")]
pub struct ConfigNotInitialized;

pub fn read_config(config_path: PathBuf) -> Result<Result<Config, ConfigNotInitialized>> {
    if config_path.exists() {
        tracing::info!(
            path = %config_path.display(),
            "Reading config file",
        );
    } else {
        return Ok(Err(ConfigNotInitialized {}));
    }

    let file = Config::read(&config_path)
        .with_context(|| format!("Failed to read config file at {}", config_path.display()))?;

    Ok(Ok(file))
}

/// Maximum allowed anti-spam deposit ratio. Values above this are implausible
/// and likely indicate a misconfiguration (e.g. deposit exceeding fees).
pub const MAX_ANTI_SPAM_DEPOSIT_RATIO: Decimal = Decimal::from_parts(2, 0, 0, false, 1); // 0.2

/// Minimum allowed BTC redeem fee multiplier. Values below this would scale
/// the estimated fee down so far that the redeem transaction is unlikely to
/// confirm, and likely indicate a misconfiguration.
pub const MIN_BTC_REDEEM_FEE_MULTIPLIER: Decimal = Decimal::from_parts(1, 0, 0, false, 1); // 0.1

/// Maximum allowed BTC redeem fee multiplier. Values above this are implausible
/// (10x the estimated fee) and likely indicate a misconfiguration.
pub const MAX_BTC_REDEEM_FEE_MULTIPLIER: Decimal = Decimal::from_parts(10, 0, 0, false, 0); // 10

pub fn validate_config(config: &Config, env_config: crate::env::Config) -> Result<()> {
    if config.monero.lock_rebuild_confirmations == 0 {
        bail!("monero.lock_rebuild_confirmations must be positive");
    }
    if config.monero.trusted_daemon && config.monero.daemon_url.is_none() {
        bail!("monero.trusted_daemon requires an explicit monero.daemon_url; automatically selected public nodes cannot be trusted");
    }

    if config.monero.network != env_config.monero_network {
        bail!(
            "Expected monero network in config file to be {:?} but was {:?}",
            env_config.monero_network,
            config.monero.network
        );
    }
    if config.bitcoin.network != env_config.bitcoin_network {
        bail!(
            "Expected bitcoin network in config file to be {:?} but was {:?}",
            env_config.bitcoin_network,
            config.bitcoin.network
        );
    }

    let ratio = config.maker.refund_policy.anti_spam_deposit_ratio;
    if ratio < Decimal::ZERO || ratio > Decimal::ONE {
        bail!("anti_spam_deposit_ratio must be between 0 and 1, got {ratio}");
    }
    if ratio > MAX_ANTI_SPAM_DEPOSIT_RATIO {
        bail!(
            "anti_spam_deposit_ratio of {ratio} exceeds maximum of {MAX_ANTI_SPAM_DEPOSIT_RATIO}. \
             Such a high deposit ratio is implausible and likely a misconfiguration."
        );
    }

    let multiplier = config.maker.btc_redeem_fee_multiplier;
    if multiplier < MIN_BTC_REDEEM_FEE_MULTIPLIER {
        bail!(
            "btc_redeem_fee_multiplier of {multiplier} is below minimum of \
             {MIN_BTC_REDEEM_FEE_MULTIPLIER}. A multiplier this low scales the \
             redeem fee down so far that the transaction is unlikely to confirm \
             (a non-positive multiplier would also fail at runtime in scale_fee)."
        );
    }
    if multiplier > MAX_BTC_REDEEM_FEE_MULTIPLIER {
        bail!(
            "btc_redeem_fee_multiplier of {multiplier} exceeds maximum of \
             {MAX_BTC_REDEEM_FEE_MULTIPLIER}. Such a high multiplier is implausible \
             and likely a misconfiguration."
        );
    }

    Ok(())
}

pub fn initial_setup(config_path: PathBuf, config: Config) -> Result<()> {
    let toml = toml::to_string(&config)?;

    ensure_directory_exists(config_path.as_path())?;
    fs::write(&config_path, toml)?;

    tracing::info!(
        path = %config_path.as_path().display(),
        "Initial setup complete, config file created",
    );
    Ok(())
}

pub fn query_user_for_initial_config_with_network(
    bitcoin_network: bitcoin::Network,
    monero_network: monero_address::Network,
) -> Result<Config> {
    let defaults = match bitcoin_network {
        bitcoin::Network::Bitcoin => Mainnet::get_config_file_defaults()?,
        bitcoin::Network::Testnet => Testnet::get_config_file_defaults()?,
        _ => bail!("Unsupported bitcoin network"),
    };

    let data_dir = prompt::data_directory(&defaults.data_dir)?;
    let target_block = prompt::bitcoin_confirmation_target(defaults.bitcoin_confirmation_target)?;
    let listen_addresses = prompt::listen_addresses(&defaults.listen_address_tcp)?;
    let electrum_rpc_urls = prompt::electrum_rpc_urls(&defaults.electrum_rpc_urls)?;
    let monero_daemon_url = prompt::monero_daemon_url()?;
    let register_hidden_service = prompt::tor_hidden_service()?;
    let min_buy = prompt::min_buy_amount()?;
    let max_buy = prompt::max_buy_amount()?;
    let ask_spread = prompt::ask_spread()?;
    let rendezvous_points = prompt::rendezvous_points()?;
    let developer_tip = prompt::developer_tip()?;

    println!();

    Ok(Config {
        data: Data { dir: data_dir },
        network: Network {
            listen: listen_addresses,
            rendezvous_point: rendezvous_points,
            external_addresses: vec![],
            prometheus_port: None,
        },
        bitcoin: Bitcoin {
            electrum_rpc_urls,
            target_block,
            finality_confirmations: None,
            network: bitcoin_network,
            use_mempool_space_fee_estimation: true,
        },
        monero: Monero {
            daemon_url: monero_daemon_url,
            finality_confirmations: None,
            network: monero_network,
            trusted_daemon: false,
            lock_rebuild_confirmations: default_lock_rebuild_confirmations(),
        },
        tor: TorConf {
            register_hidden_service,
            ..Default::default()
        },
        maker: Maker {
            min_buy_btc: min_buy,
            max_buy_btc: max_buy,
            ask_spread,
            price_ticker_ws_url_kraken: defaults.price_ticker_ws_url_kraken,
            price_ticker_ws_url_bitfinex: defaults.price_ticker_ws_url_bitfinex,
            price_ticker_rest_url_kucoin: defaults.price_ticker_rest_url_kucoin,
            price_ticker_rest_url_exolix: defaults.price_ticker_rest_url_exolix,
            price_ticker_source_exolix_api_key: None,
            price_ticker_rest_poll_interval_exolix_secs:
                default_price_ticker_rest_poll_interval_exolix_secs(),
            price_ticker_validity_duration_secs: default_price_ticker_validity_duration_secs(),
            price_ticker_source_kraken_enabled: default_price_ticker_source_enabled(),
            price_ticker_source_bitfinex_enabled: default_price_ticker_source_enabled(),
            price_ticker_source_kucoin_enabled: default_price_ticker_source_enabled(),
            external_bitcoin_redeem_address: None,
            btc_redeem_fee_multiplier: default_btc_redeem_fee_multiplier(),
            developer_tip,
            hermes_funding_amount_piconero: default_hermes_funding_amount_piconero(),
            hermes_enabled: default_hermes_enabled(),
            hermes_min_swap_amount: default_hermes_min_swap_amount(),
            refund_policy: defaults.refund_policy,
        },
    })
}

pub fn query_user_for_initial_config(testnet: bool) -> Result<Config> {
    let (bitcoin_network, monero_network) = if testnet {
        let bitcoin_network = bitcoin::Network::Testnet;
        let monero_network = monero_address::Network::Stagenet;
        (bitcoin_network, monero_network)
    } else {
        let bitcoin_network = bitcoin::Network::Bitcoin;
        let monero_network = monero_address::Network::Mainnet;
        (bitcoin_network, monero_network)
    };

    query_user_for_initial_config_with_network(bitcoin_network, monero_network)
}
