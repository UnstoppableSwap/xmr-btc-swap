#![warn(
    unused_extern_crates,
    missing_copy_implementations,
    rust_2018_idioms,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::fallible_impl_from,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::dbg_macro
)]
#![forbid(unsafe_code)]
#![allow(non_snake_case)]

use anyhow::{Context, Result, bail};
use comfy_table::Table;
use libp2p::Swarm;
use monero_sys::Daemon;
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use std::convert::TryInto;
use std::env;
use std::sync::Arc;
use structopt::clap;
use structopt::clap::ErrorKind;
mod command;
use command::{Arguments, Command, parse_args};
use swap::asb::metrics;
use swap::asb::rpc::RpcServer;
use swap::asb::{
    EventLoop, ExchangeRate, Finality, cancel, grant_mercy, punish, redeem, refund, safely_abort,
};
use swap::common::tor::{bootstrap_tor_client, create_tor_client};
use swap::common::tracing_util::Format;
use swap::common::{self, get_logs, warn_if_outdated};
use swap::database::{AccessMode, open_db};
use swap::monero;
use swap::network::rendezvous::XmrBtcNamespace;
use swap::network::swarm;
use swap::protocol::alice::{AliceState, HermesFundingPolicy, TipConfig, run};
use swap::protocol::{Database, State};
use swap::seed::Seed;
use swap_env::config::{
    Config, ConfigNotInitialized, initial_setup, query_user_for_initial_config, read_config,
    validate_config,
};
use swap_feed;
use swap_machine::alice::is_complete;
use uuid::Uuid;

const DEFAULT_WALLET_NAME: &str = "asb-wallet";

/// Initialize tracing with the specified configuration
fn initialize_tracing(json: bool, config: &Config, trace: bool) -> Result<()> {
    let format = if json { Format::Json } else { Format::Raw };
    let log_dir = config.data.dir.join("logs");

    common::tracing_util::init(format, log_dir, None, trace).expect("initialize tracing");

    tracing::info!(
        binary = "asb",
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        "Setting up context"
    );

    Ok(())
}

trait IntoDaemon {
    fn into_daemon(self) -> Result<Daemon>;
}

impl IntoDaemon for url::Url {
    fn into_daemon(self) -> Result<Daemon> {
        let hostname = self
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("No hostname found in URL"))?
            .to_string();

        let port = self
            .port()
            .ok_or_else(|| anyhow::anyhow!("No port found in URL"))?;

        let ssl = self.scheme() == "https";

        Ok(Daemon {
            hostname,
            port,
            ssl,
        })
    }
}

impl IntoDaemon for monero_rpc_pool::ServerInfo {
    fn into_daemon(self) -> Result<Daemon> {
        Ok(Daemon {
            hostname: self.host,
            port: self.port,
            ssl: false,
        })
    }
}

#[tokio::main]
pub async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install default rustls provider");

    let Arguments {
        testnet,
        json,
        trace,
        config_path,
        env_config: _,
        cmd,
    } = match parse_args(env::args_os()) {
        Ok(args) => args,
        Err(e) => {
            if let Some(clap_err) = e.downcast_ref::<clap::Error>() {
                if let ErrorKind::HelpDisplayed | ErrorKind::VersionDisplayed = clap_err.kind {
                    println!("{}", clap_err.message);
                    std::process::exit(0);
                }
            }
            bail!(e);
        }
    };

    // Check in the background if there's a new version available
    tokio::spawn(async move { warn_if_outdated(env!("CARGO_PKG_VERSION")).await });

    // Read our config
    let config = match read_config(config_path.clone())? {
        Ok(config) => config,
        Err(ConfigNotInitialized {}) => {
            initial_setup(config_path.clone(), query_user_for_initial_config(testnet)?)?;
            read_config(config_path.clone())?.expect("after initial setup config can be read")
        }
    };

    // Apply file settings to runtime configuration while keeping CLI network selection authoritative.
    // Parse arguments first because --config and --testnet determine which file to load.
    let env_config = swap_env::env::new(testnet, &config);

    // Initialize tracing
    initialize_tracing(json, &config, trace)?;

    validate_config(&config, env_config)?;

    let seed = Seed::from_file_or_generate(&config.data.dir)
        .await
        .expect("Could not retrieve/initialize seed");

    let db_file = config.data.dir.join("sqlite");

    match cmd {
        Command::Start {
            resume_only,
            rpc_bind_host,
            rpc_bind_port,
            rpc_auth_file,
        } => {
            let rpc_auth_verifier = match (&rpc_bind_host, &rpc_bind_port) {
                (Some(_), Some(_)) => {
                    let auth_file = rpc_auth_file.context(
                        "The JSON-RPC server requires authentication: pass --rpc-auth-file pointing at the RPC auth verifier file",
                    )?;
                    Some(swap_env::rpc_auth::load_verifier(&auth_file)?)
                }
                _ => None,
            };

            let db = open_db(db_file, AccessMode::ReadWrite, None).await?;

            let developer_tip = config.maker.developer_tip;
            if developer_tip.is_zero() {
                tracing::info!(
                    "Not tipping the developers (maker.developer_tip = 0 or not set in config)"
                );
            } else {
                tracing::info!(%developer_tip, "Tipping to the developers is enabled. Thank you for your support!");
            }

            // Initialize Monero wallet
            let monero_wallet = init_monero_wallet(&config, env_config).await?;
            let monero_address = monero_wallet.main_wallet().await.main_address().await?;
            tracing::info!(%monero_address, "Monero wallet address");

            // Check Monero balance
            let wallet = monero_wallet.main_wallet().await;

            let total = wallet.total_balance().await?.as_pico();
            let unlocked = wallet.unlocked_balance().await?.as_pico();

            match (total, unlocked) {
                (0, _) => {
                    tracing::warn!(
                        %monero_address,
                        "The Monero balance is 0, make sure to deposit funds at",
                    )
                }
                (total, 0) => {
                    let total = monero::Amount::from_pico(total);
                    tracing::warn!(
                        %total,
                        "Unlocked Monero balance is 0, total balance is",
                    )
                }
                (total, unlocked) => {
                    let total = monero::Amount::from_pico(total);
                    let unlocked = monero::Amount::from_pico(unlocked);
                    tracing::info!(%total, %unlocked, "Monero wallet balance");
                }
            }

            // Initialize Bitcoin wallet
            let bitcoin_wallet = init_bitcoin_wallet(&config, &seed, env_config, false).await?;
            let bitcoin_balance = bitcoin_wallet.balance().await?;
            tracing::info!(%bitcoin_balance, "Bitcoin wallet balance");

            // Connect to each enabled price feed. Each source is
            // independently toggleable via config; Exolix additionally
            // requires an API key.
            let kraken_price_updates = if config.maker.price_ticker_source_kraken_enabled {
                Some(swap_feed::connect_kraken(
                    config.maker.price_ticker_ws_url_kraken.clone(),
                )?)
            } else {
                None
            };
            let bitfinex_price_updates = if config.maker.price_ticker_source_bitfinex_enabled {
                Some(swap_feed::connect_bitfinex(
                    config.maker.price_ticker_ws_url_bitfinex.clone(),
                )?)
            } else {
                None
            };
            let kucoin_price_updates = if config.maker.price_ticker_source_kucoin_enabled {
                Some(swap_feed::connect_kucoin(
                    config.maker.price_ticker_rest_url_kucoin.clone(),
                    reqwest::Client::new(),
                )?)
            } else {
                None
            };
            let exolix_poll_interval = std::time::Duration::from_secs(
                config.maker.price_ticker_rest_poll_interval_exolix_secs,
            );
            let exolix_price_updates = config
                .maker
                .price_ticker_source_exolix_api_key
                .as_ref()
                .map(|api_key| {
                    swap_feed::connect_exolix(
                        config.maker.price_ticker_rest_url_exolix.clone(),
                        api_key.clone(),
                        exolix_poll_interval,
                        reqwest::Client::new(),
                    )
                })
                .transpose()?;
            tracing::info!(
                kraken = kraken_price_updates.is_some(),
                bitfinex = bitfinex_price_updates.is_some(),
                kucoin = kucoin_price_updates.is_some(),
                exolix = exolix_price_updates.is_some(),
                "Price feed sources",
            );

            let price_validity_duration =
                std::time::Duration::from_secs(config.maker.price_ticker_validity_duration_secs);
            let kraken_rate = ExchangeRate::new(
                config.maker.ask_spread,
                kraken_price_updates,
                bitfinex_price_updates,
                kucoin_price_updates,
                exolix_price_updates,
                price_validity_duration,
            )
            .context("Invalid price feed configuration")?;
            let namespace = XmrBtcNamespace::from_is_testnet(testnet);

            // Initialize and bootstrap Tor client
            let tor_client = create_tor_client(&config.data.dir).await?;
            bootstrap_tor_client(tor_client.clone(), None).await?;
            let tor_client = tor_client.into();

            let mut metrics_registry = config
                .network
                .prometheus_port
                .map(|_| metrics::Registry::default());

            let (mut swarm, onion_addresses, onion_service_handle) = swarm::asb(
                &seed,
                config.maker.min_buy_btc,
                config.maker.max_buy_btc,
                kraken_rate.clone(),
                resume_only,
                env_config,
                namespace,
                &config.network.rendezvous_point,
                tor_client,
                config.tor.register_hidden_service,
                config.tor.hidden_service_num_intro_points,
                config.tor.max_concurrent_rend_requests,
                config.tor.wormhole_enabled,
                config.tor.wormhole_max_concurrent_rend_requests,
                config.tor.wormhole_num_intro_points,
                config.tor.wormhole_swap_freshness_hours,
                db.clone(),
                metrics_registry.as_mut(),
            )?;

            for listen in config.network.listen.clone() {
                if let Err(e) = Swarm::listen_on(&mut swarm, listen.clone()) {
                    tracing::warn!(
                        "Failed to listen on network interface {}: {}. Consider removing it from the config.",
                        listen,
                        e
                    );
                }
            }

            for onion_address in onion_addresses {
                match swarm.listen_on(onion_address.clone()) {
                    Err(e) => {
                        tracing::warn!(
                            "Failed to listen on onion address {}: {}",
                            onion_address,
                            e
                        );
                    }
                    _ => {
                        swarm.add_external_address(onion_address);
                    }
                }
            }

            tracing::info!(peer_id = %swarm.local_peer_id(), "Network layer initialized");

            for external_address in &config.network.external_addresses {
                swarm.add_external_address(external_address.clone());
            }

            let tip_config = {
                let tip_address = monero_address::MoneroAddress::from_str_with_unchecked_network(
                    match env_config.monero_network {
                        monero::Network::Mainnet => {
                            swap_env::defaults::DEFAULT_DEVELOPER_TIP_ADDRESS_MAINNET
                        }
                        monero::Network::Stagenet => {
                            swap_env::defaults::DEFAULT_DEVELOPER_TIP_ADDRESS_STAGENET
                        }
                        monero::Network::Testnet => panic!("Testnet is not supported"),
                    },
                )
                .expect("Hardcoded developer tip address to be valid");

                assert_eq!(
                    tip_address.network(),
                    env_config.monero_network,
                    "Developer tip address must be on the correct Monero network"
                );

                TipConfig {
                    ratio: config.maker.developer_tip,
                    address: tip_address,
                }
            };

            let hermes_funding_policy = HermesFundingPolicy {
                enabled: config.maker.hermes_enabled,
                amount: monero::Amount::from_pico(config.maker.hermes_funding_amount_piconero),
                min_swap_amount: config.maker.hermes_min_swap_amount,
            };

            let (metrics, _metrics_server) =
                match (config.network.prometheus_port, metrics_registry) {
                    (Some(port), Some(mut registry)) => {
                        let metrics = metrics::Metrics::new(&mut registry);
                        let server = metrics::MetricsServer::start(port, registry).await?;
                        (Some(metrics), Some(server))
                    }
                    _ => (None, None),
                };

            let bitcoin_wallet = Arc::new(bitcoin_wallet);
            let (event_loop, mut swap_receiver, event_loop_service) = EventLoop::new(
                swarm,
                metrics,
                env_config,
                bitcoin_wallet.clone(),
                monero_wallet.clone(),
                db.clone(),
                kraken_rate.clone(),
                config.maker.min_buy_btc,
                config.maker.max_buy_btc,
                config.maker.external_bitcoin_redeem_address,
                config.maker.btc_redeem_fee_multiplier,
                tip_config,
                hermes_funding_policy,
                config.maker.refund_policy,
                onion_service_handle,
                config_path.clone(),
            )
            .unwrap();

            // Start RPC server conditionally
            let _rpc_server = if let (Some(host), Some(port)) = (rpc_bind_host, rpc_bind_port) {
                let rpc_server = RpcServer::start(
                    host,
                    port,
                    rpc_auth_verifier,
                    bitcoin_wallet.clone(),
                    monero_wallet.clone(),
                    event_loop_service,
                    db,
                )
                .await?;

                Some(rpc_server.spawn())
            } else {
                None
            };

            tokio::spawn(async move {
                while let Some(swap) = swap_receiver.recv().await {
                    let rate = kraken_rate.clone();
                    tokio::spawn(async move {
                        let swap_id = swap.swap_id;
                        match run(swap, rate).await {
                            Ok(state) => {
                                tracing::debug!(%swap_id, final_state=%state, "Swap completed")
                            }
                            Err(error) => {
                                tracing::error!(%swap_id, "Swap failed: {:#}", error)
                            }
                        }
                    });
                }
            });

            event_loop.run().await;
        }
        Command::History { only_unfinished } => {
            let db: Arc<dyn Database + Send + Sync> =
                open_db(db_file, AccessMode::ReadOnly, None).await?;
            let mut table = Table::new();

            table.set_header(vec![
                "Swap ID",
                "Start Date",
                "State",
                "Bitcoin Lock TxId",
                "BTC Amount",
                "XMR Amount",
                "Exchange Rate",
                "Taker Peer ID",
                "Completed",
            ]);

            let all_swaps = db.all().await?;
            for (_, swap_id, state) in all_swaps {
                let state: AliceState = state
                    .try_into()
                    .expect("Alice database only has Alice states");

                if only_unfinished && is_complete(&state) {
                    continue;
                }

                match SwapDetails::from_db_state(swap_id, state, &db).await {
                    Ok(details) => {
                        if json {
                            details.log_info();
                        } else {
                            table.add_row(details.to_table_row());
                        }
                    }
                    Err(e) => {
                        tracing::error!(swap_id = %swap_id, error = %e, "Failed to get swap details");
                    }
                }
            }

            if !json {
                println!("{}", table);
            }
        }
        Command::Config => {
            let config_json = serde_json::to_string_pretty(&config)?;
            println!("{}", config_json);
        }
        Command::Logs {
            logs_dir,
            swap_id,
            redact,
        } => {
            let dir = logs_dir.unwrap_or(config.data.dir.join("logs"));

            let log_messages = get_logs(dir, swap_id, redact).await?;

            for msg in log_messages {
                println!("{msg}");
            }
        }
        Command::WithdrawBtc { amount, address } => {
            let bitcoin_wallet = init_bitcoin_wallet(&config, &seed, env_config, true).await?;

            let withdraw_tx_unsigned = match amount {
                Some(amount) => {
                    bitcoin_wallet
                        .send_to_address_dynamic_fee(address, amount, None)
                        .await?
                }
                None => {
                    bitcoin_wallet
                        .sweep_balance_to_address_dynamic_fee(address)
                        .await?
                }
            };

            let signed_tx = bitcoin_wallet
                .sign_and_finalize(withdraw_tx_unsigned)
                .await?;

            bitcoin_wallet.broadcast(signed_tx, "withdraw").await?;
        }
        Command::Balance => {
            let monero_wallet = init_monero_wallet(&config, env_config).await?;
            let monero_balance = monero_wallet.main_wallet().await.total_balance().await?;
            tracing::info!(%monero_balance);

            let bitcoin_wallet = init_bitcoin_wallet(&config, &seed, env_config, true).await?;
            let bitcoin_balance = bitcoin_wallet.balance().await?;
            tracing::info!(%bitcoin_balance);
            tracing::info!(%bitcoin_balance, %monero_balance, "Current balance");
        }
        Command::Cancel { swap_id } => {
            let db = open_db(db_file, AccessMode::ReadWrite, None).await?;

            let bitcoin_wallet = init_bitcoin_wallet(&config, &seed, env_config, true).await?;

            let (txid, _) = cancel(swap_id, Arc::new(bitcoin_wallet), db).await?;

            tracing::info!("Cancel transaction successfully published with id {}", txid);
        }
        Command::Refund { swap_id } => {
            let db = open_db(db_file, AccessMode::ReadWrite, None).await?;

            let bitcoin_wallet = init_bitcoin_wallet(&config, &seed, env_config, true).await?;
            let monero_wallet = init_monero_wallet(&config, env_config).await?;

            refund(swap_id, Arc::new(bitcoin_wallet), monero_wallet.clone(), db).await?;

            tracing::info!("Monero successfully refunded");
        }
        Command::Punish { swap_id } => {
            let db = open_db(db_file, AccessMode::ReadWrite, None).await?;

            let bitcoin_wallet = init_bitcoin_wallet(&config, &seed, env_config, true).await?;

            let (txid, _) = punish(swap_id, Arc::new(bitcoin_wallet), db).await?;

            tracing::info!("Punish transaction successfully published with id {}", txid);
        }
        Command::SafelyAbort { swap_id } => {
            let db = open_db(db_file, AccessMode::ReadWrite, None).await?;

            safely_abort(swap_id, db).await?;

            tracing::info!("Swap safely aborted");
        }
        Command::GrantMercy { swap_id } => {
            let db = open_db(db_file, AccessMode::ReadWrite, None).await?;

            grant_mercy(swap_id, db).await?;

            tracing::info!("Mercy granted for swap {}", swap_id);
        }
        Command::Redeem {
            swap_id,
            do_not_await_finality,
        } => {
            let db = open_db(db_file, AccessMode::ReadWrite, None).await?;

            let bitcoin_wallet = init_bitcoin_wallet(&config, &seed, env_config, true).await?;

            let (txid, _) = redeem(
                swap_id,
                Arc::new(bitcoin_wallet),
                db,
                Finality::from_bool(do_not_await_finality),
            )
            .await?;

            tracing::info!("Redeem transaction successfully published with id {}", txid);
        }
        Command::ExportBitcoinWallet => {
            let bitcoin_wallet = init_bitcoin_wallet(&config, &seed, env_config, false).await?;
            let wallet_export = bitcoin_wallet.wallet_export("asb").await?;
            println!("{}", wallet_export)
        }
        Command::ExportMoneroWallet => {
            let monero_wallet = init_monero_wallet(&config, env_config).await?;
            let main_wallet = monero_wallet.main_wallet().await;

            let seed = main_wallet.seed().await?;
            let creation_height = main_wallet.creation_height().await?;

            println!("Seed          : {seed}");
            println!("Restore height: {creation_height}");
        }
        Command::ExportMoneroLockWallet { swap_id } => {
            let db = open_db(db_file, AccessMode::ReadWrite, None).await?;
            let bitcoin_wallet = init_bitcoin_wallet(&config, &seed, env_config, false).await?;

            let swap_states = db
                .get_states(swap_id)
                .await
                .context(format!("Error querying database for swap {swap_id}"))?;

            if swap_states.is_empty() {
                tracing::error!("No state save for this swap in the database");
            }

            tracing::info!(?swap_states, "Found swap states");

            let state3 = swap_states
                .iter()
                .filter_map(|state| match state {
                    State::Alice(AliceState::Started { state3 })
                    | State::Alice(AliceState::BtcLocked { state3 })
                    | State::Alice(AliceState::XmrReadyToLock { state3, .. })
                    | State::Alice(AliceState::BtcLockTransactionSeen { state3 }) => {
                        Some(state3.clone())
                    }
                    _ => None,
                })
                .next()
                .context("Couldn't find state Started for this swap")?;

            let secret_spend_key = match state3.watch_for_btc_tx_full_refund(&bitcoin_wallet).await
            {
                Ok(secret) => secret,
                Err(error) => {
                    tracing::error!(
                        ?error,
                        "Could not extract refund secret from taker's refund transaction"
                    );
                    return Ok(());
                }
            };
            let secret_view_key = state3.v;
            let primary_address = {
                let public_spend_key = monero::PublicKey::from_private_key(&secret_spend_key);
                let public_view_key = monero::PublicKey::from_private_key(&secret_view_key.into());

                monero_address::MoneroAddress::new(
                    config.monero.network,
                    monero_address::AddressType::Subaddress,
                    public_spend_key.decompress(),
                    public_view_key.decompress(),
                )
            };

            println!("Retrieved the refund secret from taker's refund transaction. Below are the keys to the Monero lock wallet:
private spend key: {secret_spend_key}
private view key: {secret_view_key}
primary address: {primary_address}");
        }
    }

    Ok(())
}

async fn init_bitcoin_wallet(
    config: &Config,
    seed: &Seed,
    env_config: swap_env::env::Config,
    sync: bool,
) -> Result<bitcoin_wallet::Wallet> {
    tracing::debug!("Opening Bitcoin wallet");

    let wallet = bitcoin_wallet::WalletBuilder::<Seed>::default()
        .seed(seed.clone())
        .network(env_config.bitcoin_network)
        .electrum_rpc_urls(
            config
                .bitcoin
                .electrum_rpc_urls
                .iter()
                .map(|url| url.as_str().to_string())
                .collect::<Vec<String>>(),
        )
        .persister(bitcoin_wallet::PersisterConfig::SqliteFile {
            data_dir: config.data.dir.clone(),
        })
        .finality_confirmations(env_config.bitcoin_finality_confirmations)
        .target_block(config.bitcoin.target_block)
        .use_mempool_space_fee_estimation(config.bitcoin.use_mempool_space_fee_estimation)
        .sync_interval(env_config.bitcoin_sync_interval())
        .build()
        .await
        .context("Failed to initialize Bitcoin wallet")?;

    if sync {
        wallet.sync().await?;
    } else {
        tracing::info!(
            "Skipping Bitcoin wallet sync because we are only using it for receiving funds"
        );
    }

    Ok(wallet)
}

async fn init_monero_wallet(
    config: &Config,
    env_config: swap_env::env::Config,
) -> Result<Arc<monero::Wallets>> {
    tracing::debug!("Initializing Monero wallets");

    let daemon = match &config.monero.daemon_url {
        // If a daemon URL is provided, use it
        Some(url) => {
            tracing::info!("Using direct Monero daemon connection: {url}");

            url.clone()
                .into_daemon()
                .context("Failed to convert daemon URL to Daemon")?
        }
        // If no daemon URL is provided, start the monero-rpc-pool and use it
        None => {
            let (server_info, _status_receiver, _pool_handle) =
                monero_rpc_pool::start_server_with_random_port(
                    monero_rpc_pool::config::Config::new_random_port(
                        config.data.dir.join("monero-rpc-pool"),
                        env_config.monero_network,
                    ),
                )
                .await
                .context("Failed to start Monero RPC Pool for ASB")?;

            let pool_url = format!("http://{}:{}", server_info.host, server_info.port);
            tracing::info!("Monero RPC Pool started for ASB on {}", pool_url);

            server_info
                .into_daemon()
                .context("Failed to convert ServerInfo to Daemon")?
        }
    };

    let manager = monero::Wallets::new(
        config.data.dir.join("monero/wallets"),
        DEFAULT_WALLET_NAME.to_string(),
        daemon,
        env_config.monero_network,
        false,
        None,
        None,
    )
    .await
    .context("Failed to initialize Monero wallets")?;

    Ok(Arc::new(manager))
}

/// This struct is used to extract swap details from the database and print them in a table format
#[derive(Debug)]
struct SwapDetails {
    swap_id: String,
    start_date: String,
    state: String,
    btc_lock_txid: String,
    btc_amount: String,
    xmr_amount: String,
    exchange_rate: String,
    peer_id: String,
    completed: bool,
}

impl SwapDetails {
    async fn from_db_state(
        swap_id: Uuid,
        latest_state: AliceState,
        db: &Arc<dyn Database + Send + Sync>,
    ) -> Result<Self> {
        let completed = is_complete(&latest_state);

        let all_states = db.get_states(swap_id).await?;
        let state3 = all_states
            .iter()
            .find_map(|s| match s {
                State::Alice(AliceState::BtcLockTransactionSeen { state3 }) => Some(state3),
                _ => None,
            })
            .context("Failed to get \"BtcLockTransactionSeen\" state")?;

        let exchange_rate = Self::calculate_exchange_rate(state3.btc, state3.xmr)?;
        let start_date = db.get_swap_start_date(swap_id).await?;
        let btc_lock_txid = state3.tx_lock.txid();
        let peer_id = db.get_peer_id(swap_id).await?;

        Ok(Self {
            swap_id: swap_id.to_string(),
            start_date: start_date.to_string(),
            state: latest_state.to_string(),
            btc_lock_txid: btc_lock_txid.to_string(),
            btc_amount: state3.btc.to_string(),
            xmr_amount: state3.xmr.to_string(),
            exchange_rate,
            peer_id: peer_id.to_string(),
            completed,
        })
    }

    fn calculate_exchange_rate(btc: bitcoin::Amount, xmr: monero::Amount) -> Result<String> {
        let btc_decimal = Decimal::from_f64(btc.to_btc())
            .ok_or_else(|| anyhow::anyhow!("Failed to convert BTC amount to Decimal"))?;
        let xmr_decimal = Decimal::new(xmr.as_pico().try_into()?, monero::Amount::XMR_SCALE);

        let rate = btc_decimal
            .checked_div(xmr_decimal)
            .ok_or_else(|| anyhow::anyhow!("Division by zero or overflow"))?;

        Ok(format!("{} XMR/BTC", rate.round_dp(8)))
    }

    fn to_table_row(&self) -> Vec<String> {
        vec![
            self.swap_id.clone(),
            self.start_date.clone(),
            self.state.clone(),
            self.btc_lock_txid.clone(),
            self.btc_amount.clone(),
            self.xmr_amount.clone(),
            self.exchange_rate.clone(),
            self.peer_id.clone(),
            self.completed.to_string(),
        ]
    }

    fn log_info(&self) {
        tracing::info!(
            swap_id = %self.swap_id,
            swap_start_date = %self.start_date,
            latest_state = %self.state,
            btc_lock_txid = %self.btc_lock_txid,
            btc_amount = %self.btc_amount,
            xmr_amount = %self.xmr_amount,
            exchange_rate = %self.exchange_rate,
            taker_peer_id = %self.peer_id,
            completed = self.completed,
            "Found swap in database"
        );
    }
}
