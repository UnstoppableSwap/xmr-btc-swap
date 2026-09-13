mod bitcoind;
mod electrs;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use bitcoin_harness::{BitcoindRpcApi, Client};
use futures::Future;
use libp2p::PeerId;
use libp2p::core::Multiaddr;
use monero_harness::{Monero, image};
use monero_sys::Daemon;
use rust_decimal::Decimal;
use std::cmp::Ordering;
use std::fmt;
use std::path::PathBuf;
use swap_env::config::RefundPolicy;

use std::sync::Arc;
use std::time::Duration;
use swap::asb::FixedRate;
use swap::cli::api;
use swap::database::{AccessMode, SqliteDatabase};
use swap::monero::wallet::no_listener;
use swap::monero::{MoneroAddressPool, Wallets};
use swap::network::rendezvous::XmrBtcNamespace;
use swap::network::swarm;
use swap::protocol::alice::{AliceState, HermesFundingPolicy, Swap, TipConfig};
use swap::protocol::bob::BobState;
use swap::protocol::{Database, State, alice, bob};
use swap::seed::Seed;
use swap::{asb, cli, monero};
use swap_core::bitcoin::{CancelTimelock, PunishTimelock};
use swap_env::env;
use swap_env::env::{Config, GetConfig};
use swap_fs::ensure_directory_exists;
use tempfile::{NamedTempFile, TempDir};
use testcontainers::clients::Cli;
use testcontainers::{Container, RunnableImage};
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio::task::JoinHandle;
use tokio::time::{interval, timeout};
use url::Url;
use uuid::Uuid;

/// developer_tip_ratio is a tuple of (ratio, use_subaddress)
///
/// If use_subaddress is true, we will use a subaddress for the developer tip. We do this
/// because using a subaddress changes things about the tx keys involved
pub async fn setup_test_funded<T, F, C>(
    _config: C,
    developer_tip_ratio: Option<(Decimal, bool)>,
    refund_policy: Option<RefundPolicy>,
    alice_xmr_outputs: u64,
    testfn: T,
) where
    T: Fn(TestContext) -> F,
    F: Future<Output = Result<()>>,
    C: GetConfig,
{
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install default rustls provider");

    let cli = Cli::default();

    tracing_subscriber::fmt()
        .with_env_filter("info,swap=trace,swap_p2p=trace,monero_harness=debug,bitcoin_harness=info,testcontainers=info,monero_cpp=info,monero_sys=debug,monero_wallet_ng=trace") // add `reqwest::connect::verbose=trace` if you want to logs of the RPC clients
        .with_test_writer()
        .init();

    let env_config = C::get_config();

    let (monero, containers) = init_containers(&cli).await;
    monero.init_miner().await.unwrap();

    let btc_amount = bitcoin::Amount::from_sat(1_000_000);
    let xmr_amount =
        monero::Amount::parse_monero(&(btc_amount.to_btc() / FixedRate::RATE).to_string()).unwrap();
    let electrs_rpc_port = containers.electrs.get_host_port_ipv4(electrs::RPC_PORT);

    let developer_seed = Seed::random().unwrap();
    let developer_starting_balances =
        StartingBalances::new(bitcoin::Amount::ZERO, monero::Amount::ZERO, None);
    let developer_tip_monero_dir = TempDir::new()
        .unwrap()
        .path()
        .join("developer_tip-monero-wallets");

    let (_, developer_tip_monero_wallet) = init_test_wallets(
        "developer_tip",
        containers.bitcoind_url.clone(),
        &monero,
        &containers._monerod_container,
        developer_tip_monero_dir,
        developer_starting_balances.clone(),
        electrs_rpc_port,
        &developer_seed,
        env_config,
    )
    .await;

    let developer_tip_monero_wallet_main_address = developer_tip_monero_wallet
        .main_wallet()
        .await
        .main_address()
        .await
        .unwrap()
        .into();

    let developer_tip_monero_wallet_subaddress = developer_tip_monero_wallet
        .main_wallet()
        .await
        // explicitly use a suabddress here to test the additional tx key logic
        .address(0, 2)
        .await
        .unwrap()
        .into();

    let developer_tip = TipConfig {
        ratio: developer_tip_ratio.unwrap_or((Decimal::ZERO, false)).0,
        address: match developer_tip_ratio {
            Some((_, true)) => developer_tip_monero_wallet_subaddress,
            Some((_, false)) => developer_tip_monero_wallet_main_address,
            None => developer_tip_monero_wallet_main_address,
        },
    };

    let alice_starting_balances =
        StartingBalances::new(bitcoin::Amount::ZERO, xmr_amount, Some(alice_xmr_outputs));
    let alice_seed = Seed::random().unwrap();
    let alice_db_path = NamedTempFile::new().unwrap().path().to_path_buf();
    let alice_config_path = alice_db_path.with_extension("config.toml");
    // The `set_external_bitcoin_redeem_address` RPC handler reads, mutates,
    // and rewrites this file. The values are placeholders — the running ASB
    // never re-consumes most of them (the wallet, swarm and DB were
    // constructed via the test harness, not from this file).
    std::fs::write(
        &alice_config_path,
        r#"[data]
dir = "/tmp/alice"

[network]
listen = []
rendezvous_point = []
external_addresses = []

[bitcoin]
electrum_rpc_urls = ["tcp://localhost:50001"]
target_block = 1
finality_confirmations = 1
network = "Regtest"
use_mempool_space_fee_estimation = false

[monero]
finality_confirmations = 1
network = "Mainnet"

[tor]
register_hidden_service = false
hidden_service_num_intro_points = 1

[maker]
min_buy_btc = 0.0
max_buy_btc = 1000.0
ask_spread = "0"
"#,
    )
    .unwrap();
    let alice_monero_dir = TempDir::new().unwrap().path().join("alice-monero-wallets");
    let (alice_bitcoin_wallet, alice_monero_wallet) = init_test_wallets(
        MONERO_WALLET_NAME_ALICE,
        containers.bitcoind_url.clone(),
        &monero,
        &containers._monerod_container,
        alice_monero_dir,
        alice_starting_balances.clone(),
        electrs_rpc_port,
        &alice_seed,
        env_config,
    )
    .await;

    let alice_listen_port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let alice_listen_address: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", alice_listen_port)
        .parse()
        .expect("failed to parse Alice's address");

    let alice_rpc_port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();

    // Far above the mainnet default: regtest fee estimation demands
    // ~0.0084 XMR for the Hermes transaction
    let alice_hermes_funding_amount = monero::Amount::parse_monero("0.02").unwrap();

    let (alice_handle, alice_swap_handle, alice_rpc_server_handle) = start_alice(
        &alice_seed,
        alice_db_path.clone(),
        alice_listen_address.clone(),
        env_config,
        alice_bitcoin_wallet.clone(),
        alice_monero_wallet.clone(),
        developer_tip.clone(),
        alice_hermes_funding_amount,
        refund_policy.clone().unwrap_or_default(),
        alice_rpc_port,
    )
    .await;

    let alice_rpc_client = jsonrpsee::http_client::HttpClientBuilder::default()
        .build(format!("http://127.0.0.1:{}", alice_rpc_port))
        .expect("Failed to create RPC client");

    let bob_seed = Seed::random().unwrap();
    let bob_starting_balances = StartingBalances::new(btc_amount * 10, monero::Amount::ZERO, None);
    let bob_monero_dir = TempDir::new().unwrap().path().join("bob-monero-wallets");
    let (bob_bitcoin_wallet, bob_monero_wallet) = init_test_wallets(
        MONERO_WALLET_NAME_BOB,
        containers.bitcoind_url.clone(),
        &monero,
        &containers._monerod_container,
        bob_monero_dir,
        bob_starting_balances.clone(),
        electrs_rpc_port,
        &bob_seed,
        env_config,
    )
    .await;

    let bob_donation_seed = Seed::random().unwrap();
    let bob_donation_starting_balances =
        StartingBalances::new(bitcoin::Amount::ZERO, monero::Amount::ZERO, None);
    let bob_donation_monero_dir = TempDir::new()
        .unwrap()
        .path()
        .join("bob-donation-monero-wallets");
    let (_, bob_donation_monero_wallet) = init_test_wallets(
        "bob_donation",
        containers.bitcoind_url.clone(),
        &monero,
        &containers._monerod_container,
        bob_donation_monero_dir,
        bob_donation_starting_balances,
        electrs_rpc_port,
        &bob_donation_seed,
        env_config,
    )
    .await;

    let bob_params = BobParams {
        seed: Seed::random().unwrap(),
        db_path: NamedTempFile::new().unwrap().path().to_path_buf(),
        bitcoin_wallet: bob_bitcoin_wallet.clone(),
        monero_wallet: bob_monero_wallet.clone(),
        alice_address: alice_listen_address.clone(),
        alice_peer_id: alice_handle.peer_id,
        env_config,
    };

    monero.start_miner().await.unwrap();

    let test = TestContext {
        env_config,
        btc_amount,
        xmr_amount,
        alice_seed,
        alice_db_path,
        alice_config_path,
        alice_listen_address,
        alice_rpc_port,
        alice_rpc_server_handle,
        alice_rpc_client,
        alice_starting_balances,
        alice_bitcoin_wallet,
        alice_monero_wallet,
        alice_swap_handle,
        alice_handle,
        bob_params,
        bob_starting_balances,
        bob_bitcoin_wallet,
        bob_monero_wallet,
        bob_donation_monero_wallet,
        developer_tip_monero_wallet,
        developer_tip,
        refund_policy: refund_policy.unwrap_or_default(),
        alice_hermes_funding_amount,
        monerod_container_id: containers._monerod_container.id().to_string(),
        monero,
    };

    testfn(test).await.unwrap()
}

/// Thin wrapper over [`setup_test_funded`] with the default maker Monero balance
/// (ten 1-XMR outputs), used by every test that does not need to constrain how
/// much unlocked XMR Alice holds.
pub async fn setup_test<T, F, C>(
    config: C,
    developer_tip_ratio: Option<(Decimal, bool)>,
    refund_policy: Option<RefundPolicy>,
    testfn: T,
) where
    T: Fn(TestContext) -> F,
    F: Future<Output = Result<()>>,
    C: GetConfig,
{
    setup_test_funded(config, developer_tip_ratio, refund_policy, 10, testfn).await
}

async fn init_containers(cli: &Cli) -> (Monero, Containers<'_>) {
    let prefix = random_prefix();
    let bitcoind_name = format!("{}_{}", prefix, "bitcoind");
    let (_bitcoind, bitcoind_url, mapped_port) =
        init_bitcoind_container(cli, prefix.clone(), bitcoind_name.clone(), prefix.clone())
            .await
            .expect("could not init bitcoind");
    let electrs = init_electrs_container(cli, prefix.clone(), bitcoind_name, prefix, mapped_port)
        .await
        .expect("could not init electrs");
    let (monero, _monerod_container, _monero_wallet_rpc_containers) =
        Monero::new(cli, vec![MONERO_WALLET_NAME_ALICE, MONERO_WALLET_NAME_BOB])
            .await
            .unwrap();

    (
        monero,
        Containers {
            bitcoind_url,
            _bitcoind,
            _monerod_container,
            _monero_wallet_rpc_containers,
            electrs,
        },
    )
}

async fn init_bitcoind_container(
    cli: &Cli,
    volume: String,
    name: String,
    network: String,
) -> Result<(Container<'_, bitcoind::Bitcoind>, Url, u16)> {
    let image = bitcoind::Bitcoind::default().with_volume(volume);
    let image = RunnableImage::from(image)
        .with_container_name(name)
        .with_network(network);

    let docker = cli.run(image);
    let port = docker.get_host_port_ipv4(bitcoind::RPC_PORT);

    let bitcoind_url = {
        let input = format!(
            "http://{}:{}@localhost:{}",
            bitcoind::RPC_USER,
            bitcoind::RPC_PASSWORD,
            port
        );
        Url::parse(&input).unwrap()
    };

    init_bitcoind(bitcoind_url.clone(), 5).await?;

    Ok((docker, bitcoind_url.clone(), bitcoind::RPC_PORT))
}

pub async fn init_electrs_container(
    cli: &Cli,
    volume: String,
    bitcoind_container_name: String,
    network: String,
    port: u16,
) -> Result<Container<'_, electrs::Electrs>> {
    let bitcoind_rpc_addr = format!("{}:{}", bitcoind_container_name, port);
    let image = electrs::Electrs::default()
        .with_volume(volume)
        .with_daemon_rpc_addr(bitcoind_rpc_addr)
        .with_tag("latest");
    let image = RunnableImage::from(image.self_and_args())
        .with_network(network.clone())
        .with_container_name(format!("{}_electrs", network));

    let docker = cli.run(image);

    Ok(docker)
}

async fn start_alice(
    seed: &Seed,
    db_path: PathBuf,
    listen_address: Multiaddr,
    env_config: Config,
    bitcoin_wallet: Arc<bitcoin_wallet::Wallet>,
    monero_wallet: Arc<monero::Wallets>,
    developer_tip: TipConfig,
    hermes_funding_amount: monero::Amount,
    refund_policy: RefundPolicy,
    rpc_port: u16,
) -> (
    AliceApplicationHandle,
    Receiver<alice::Swap>,
    tokio_util::task::AbortOnDropHandle<()>,
) {
    if let Some(parent_dir) = db_path.parent() {
        ensure_directory_exists(parent_dir).unwrap();
    }
    if !&db_path.exists() {
        tokio::fs::File::create(&db_path).await.unwrap();
    }
    let db = Arc::new(
        SqliteDatabase::open(db_path.as_path(), AccessMode::ReadWrite)
            .await
            .unwrap(),
    );

    let min_buy = bitcoin::Amount::from_sat(u64::MIN);
    let max_buy = bitcoin::Amount::from_sat(u64::MAX);
    let latest_rate = FixedRate::default();
    let resume_only = false;

    // Tests fund Hermes for any swap size; `disable_alice_hermes` opts out by
    // setting the funding amount to zero.
    let hermes_funding_policy = HermesFundingPolicy {
        enabled: true,
        amount: hermes_funding_amount,
        min_swap_amount: bitcoin::Amount::ZERO,
    };

    let (mut swarm, _, _) = swarm::asb(
        seed,
        min_buy,
        max_buy,
        latest_rate,
        resume_only,
        env_config,
        XmrBtcNamespace::Testnet,
        &[],
        None,
        false,
        1,
        16,
        false,
        3,
        3,
        168,
        db.clone(),
        None,
    )
    .unwrap();
    swarm.listen_on(listen_address).unwrap();

    let (event_loop, swap_handle, service) = asb::EventLoop::new(
        swarm,
        None,
        env_config,
        bitcoin_wallet.clone(),
        monero_wallet.clone(),
        db.clone(),
        FixedRate::default(),
        min_buy,
        max_buy,
        None,
        swap_env::config::default_btc_redeem_fee_multiplier(),
        developer_tip,
        hermes_funding_policy,
        refund_policy,
        None,
        db_path.with_extension("config.toml"),
    )
    .unwrap();

    let rpc_server_handle = asb::rpc::RpcServer::start(
        "127.0.0.1".to_string(),
        rpc_port,
        None,
        bitcoin_wallet,
        monero_wallet,
        service,
        db,
    )
    .await
    .expect("Failed to start RPC server")
    .spawn();

    let peer_id = event_loop.peer_id();
    let handle = tokio::spawn(event_loop.run());

    (
        AliceApplicationHandle { handle, peer_id },
        swap_handle,
        rpc_server_handle,
    )
}

#[allow(clippy::too_many_arguments)]
async fn init_test_wallets(
    name: &str,
    bitcoind_url: Url,
    monero: &Monero,
    monerod_container: &Container<'_, image::Monerod>,
    monero_wallet_dir: PathBuf,
    starting_balances: StartingBalances,
    electrum_rpc_port: u16,
    seed: &Seed,
    env_config: Config,
) -> (Arc<bitcoin_wallet::Wallet>, Arc<monero::Wallets>) {
    let monerod_port = monerod_container
        .ports()
        .map_to_host_port_ipv4(image::RPC_PORT)
        .expect("rpc port should be mapped to some external port");
    let monero_daemon = Daemon {
        hostname: "127.0.0.1".to_string(),
        port: monerod_port,
        ssl: false,
    };

    let wallets = Wallets::new(
        monero_wallet_dir,
        "main".to_string(),
        monero_daemon,
        monero::Network::Mainnet,
        true,
        None,
        None,
    )
    .await
    .unwrap();

    let xmr_wallet = wallets.main_wallet().await;
    tracing::info!(
        address = %xmr_wallet.main_address().await.unwrap(),
        "Initialized monero wallet"
    );

    monero
        .init_external_wallet(
            name,
            &xmr_wallet,
            starting_balances
                .xmr_outputs
                .into_iter()
                .map(|amount| amount.as_pico())
                .collect(),
        )
        .await
        .unwrap();

    // On regtests we need to allow a mismatched daemon version.
    // Regtests use the Mainnet network.
    if env_config.monero_network == monero::Network::Mainnet {
        xmr_wallet.unsafe_prepare_for_regtest().await;
    }

    let electrum_rpc_url = {
        let input = format!("tcp://@localhost:{}", electrum_rpc_port);
        Url::parse(&input).unwrap()
    };

    let btc_wallet = bitcoin_wallet::WalletBuilder::<Seed>::default()
        .seed(seed.clone())
        .network(env_config.bitcoin_network)
        .electrum_rpc_urls(vec![electrum_rpc_url.as_str().to_string()])
        .persister(bitcoin_wallet::PersisterConfig::InMemorySqlite)
        .finality_confirmations(1_u32)
        .target_block(1_u32)
        .sync_interval(Duration::from_secs(3)) // high sync interval to speed up tests
        .use_mempool_space_fee_estimation(false)
        .build()
        .await
        .expect("could not init btc wallet");

    if starting_balances.btc != bitcoin::Amount::ZERO {
        mint(
            bitcoind_url,
            btc_wallet.new_address().await.unwrap(),
            starting_balances.btc,
        )
        .await
        .expect("could not mint btc starting balance");

        let mut interval = interval(Duration::from_secs(1u64));
        let mut retries = 0u8;
        let max_retries = 30u8;
        loop {
            retries += 1;
            btc_wallet.sync().await.unwrap();

            let btc_balance = btc_wallet.balance().await.unwrap();

            if btc_balance == starting_balances.btc {
                break;
            } else if retries == max_retries {
                panic!(
                    "Bitcoin wallet initialization failed, reached max retries upon balance sync"
                )
            }

            interval.tick().await;
        }
    }

    tracing::info!("Waiting for monero wallet to sync");
    xmr_wallet.wait_until_synced(no_listener()).await.unwrap();

    tracing::info!("Monero wallet synced");

    (Arc::new(btc_wallet), Arc::new(wallets))
}

const MONERO_WALLET_NAME_BOB: &str = "bob";
const MONERO_WALLET_NAME_ALICE: &str = "alice";
const BITCOIN_TEST_WALLET_NAME: &str = "testwallet";

#[derive(Debug, Clone)]
pub struct StartingBalances {
    pub xmr: monero::Amount,
    pub xmr_outputs: Vec<monero::Amount>,
    pub btc: bitcoin::Amount,
}

impl StartingBalances {
    /// If monero_outputs is specified the monero balance will be:
    /// monero_outputs * new_xmr = self_xmr
    pub fn new(btc: bitcoin::Amount, xmr: monero::Amount, monero_outputs: Option<u64>) -> Self {
        match monero_outputs {
            None => {
                if xmr == monero::Amount::ZERO {
                    return Self {
                        xmr,
                        xmr_outputs: vec![],
                        btc,
                    };
                }

                Self {
                    xmr,
                    xmr_outputs: vec![xmr],
                    btc,
                }
            }
            Some(outputs) => {
                let mut xmr_outputs = Vec::new();
                let mut sum_xmr = monero::Amount::ZERO;

                for _ in 0..outputs {
                    xmr_outputs.push(xmr);
                    sum_xmr = sum_xmr + xmr;
                }

                Self {
                    xmr: sum_xmr,
                    xmr_outputs,
                    btc,
                }
            }
        }
    }
}

pub struct BobParams {
    seed: Seed,
    db_path: PathBuf,
    bitcoin_wallet: Arc<bitcoin_wallet::Wallet>,
    monero_wallet: Arc<monero::Wallets>,
    alice_address: Multiaddr,
    alice_peer_id: PeerId,
    env_config: Config,
}

impl BobParams {
    pub fn get_concentenated_alice_address(&self) -> String {
        format!(
            "{}/p2p/{}",
            self.alice_address.clone(),
            self.alice_peer_id.to_base58()
        )
    }

    pub async fn get_change_receive_addresses(
        &self,
    ) -> (bitcoin::Address, monero_address::MoneroAddress) {
        (
            self.bitcoin_wallet.new_address().await.unwrap(),
            self.monero_wallet
                .main_wallet()
                .await
                .main_address()
                .await
                .unwrap(),
        )
    }

    pub async fn new_swap_from_db(&self, swap_id: Uuid) -> Result<(bob::Swap, cli::EventLoop)> {
        if let Some(parent_dir) = self.db_path.parent() {
            ensure_directory_exists(parent_dir)?;
        }
        if !self.db_path.exists() {
            tokio::fs::File::create(&self.db_path).await?;
        }
        let db = Arc::new(SqliteDatabase::open(&self.db_path, AccessMode::ReadWrite).await?);

        let (event_loop, mut handle) = self.new_eventloop(db.clone()).await?;

        let swap_handle = handle.swap_handle(self.alice_peer_id, swap_id).await?;

        let swap = bob::Swap::from_db(
            db.clone(),
            swap_id,
            self.bitcoin_wallet.clone(),
            self.monero_wallet.clone(),
            self.env_config,
            swap_handle,
            self.monero_wallet
                .main_wallet()
                .await
                .main_address()
                .await
                .unwrap()
                .into(),
        )
        .await
        .unwrap();

        Ok((swap, event_loop))
    }

    pub async fn new_swap(
        &self,
        btc_amount: bitcoin::Amount,
    ) -> Result<(bob::Swap, cli::EventLoop)> {
        let pool = self
            .monero_wallet
            .main_wallet()
            .await
            .main_address()
            .await
            .unwrap()
            .into();
        self.new_swap_with_pool(btc_amount, pool).await
    }

    pub async fn new_swap_with_pool(
        &self,
        btc_amount: bitcoin::Amount,
        monero_receive_pool: MoneroAddressPool,
    ) -> Result<(bob::Swap, cli::EventLoop)> {
        let swap_id = Uuid::new_v4();

        if let Some(parent_dir) = self.db_path.parent() {
            ensure_directory_exists(parent_dir)?;
        }
        if !self.db_path.exists() {
            tokio::fs::File::create(&self.db_path).await?;
        }
        let db = Arc::new(SqliteDatabase::open(&self.db_path, AccessMode::ReadWrite).await?);

        let (event_loop, mut handle) = self.new_eventloop(db.clone()).await?;

        db.insert_peer_id(swap_id, self.alice_peer_id).await?;

        let swap_handle = handle.swap_handle(self.alice_peer_id, swap_id).await?;

        let swap = bob::Swap::new(
            db,
            swap_id,
            self.bitcoin_wallet.clone(),
            self.monero_wallet.clone(),
            self.env_config,
            swap_handle,
            monero_receive_pool,
            self.bitcoin_wallet.new_address().await?,
            btc_amount,
            bitcoin::Amount::from_sat(1000), // Fixed fee of 1000 satoshis for now
        );

        Ok((swap, event_loop))
    }

    pub async fn new_eventloop(
        &self,
        db: Arc<SqliteDatabase>,
    ) -> Result<(cli::EventLoop, cli::EventLoopHandle)> {
        let identity = self.seed.derive_libp2p_identity();

        let (mut swarm, tor_priority_tracker) = swarm::cli(identity.clone(), None, |relay| {
            cli::Behaviour::new(
                self.env_config,
                self.bitcoin_wallet.clone(),
                identity,
                relay,
                XmrBtcNamespace::Testnet,
                Vec::new(),
                db.clone(),
            )
        })
        .await?;
        swarm.add_peer_address(self.alice_peer_id, self.alice_address.clone());

        cli::EventLoop::new(swarm, db.clone(), None, tor_priority_tracker)
    }
}

pub struct BobApplicationHandle(pub JoinHandle<()>);

impl BobApplicationHandle {
    pub fn abort(&self) {
        self.0.abort()
    }
}

pub struct AliceApplicationHandle {
    handle: JoinHandle<()>,
    peer_id: PeerId,
}

impl AliceApplicationHandle {
    pub fn abort(&self) {
        self.handle.abort()
    }
}

pub struct TestContext {
    env_config: Config,

    btc_amount: bitcoin::Amount,
    xmr_amount: monero::Amount,
    developer_tip: TipConfig,
    refund_policy: RefundPolicy,
    alice_hermes_funding_amount: monero::Amount,

    alice_seed: Seed,
    alice_db_path: PathBuf,
    pub alice_config_path: PathBuf,
    alice_listen_address: Multiaddr,
    alice_rpc_port: u16,
    #[allow(dead_code)]
    alice_rpc_server_handle: tokio_util::task::AbortOnDropHandle<()>,
    pub alice_rpc_client: jsonrpsee::http_client::HttpClient,

    alice_starting_balances: StartingBalances,
    alice_bitcoin_wallet: Arc<bitcoin_wallet::Wallet>,
    alice_monero_wallet: Arc<monero::Wallets>,
    alice_swap_handle: mpsc::Receiver<Swap>,
    alice_handle: AliceApplicationHandle,

    pub bob_params: BobParams,
    bob_starting_balances: StartingBalances,
    bob_bitcoin_wallet: Arc<bitcoin_wallet::Wallet>,
    pub bob_monero_wallet: Arc<monero::Wallets>,
    pub bob_donation_monero_wallet: Arc<monero::Wallets>,

    developer_tip_monero_wallet: Arc<monero::Wallets>,

    // Store the container ID as String instead of reference
    monerod_container_id: String,

    // Handle for the Monero daemon. This allows us to skip waiting times by generating
    // blocks instantly
    pub monero: Monero,
}

impl TestContext {
    pub async fn get_bob_context(self) -> api::Context {
        api::Context::for_harness(
            self.bob_params.seed,
            self.env_config,
            self.bob_params.db_path,
            self.bob_bitcoin_wallet,
            self.bob_monero_wallet,
        )
        .await
    }

    /// Disable the on-chain encrypted signature channel for Alice by setting
    /// her Hermes funding to zero, then restart her so it takes effect.
    pub async fn disable_alice_hermes(&mut self) {
        self.alice_hermes_funding_amount = monero::Amount::ZERO;
        self.restart_alice().await;
    }

    pub async fn restart_alice(&mut self) {
        self.alice_handle.abort();
        // Abort the old RPC server to release the port before starting a new one
        self.alice_rpc_server_handle.abort();
        // Small delay to ensure port is released
        tokio::time::sleep(Duration::from_millis(100)).await;

        let (alice_handle, alice_swap_handle, alice_rpc_server_handle) = start_alice(
            &self.alice_seed,
            self.alice_db_path.clone(),
            self.alice_listen_address.clone(),
            self.env_config,
            self.alice_bitcoin_wallet.clone(),
            self.alice_monero_wallet.clone(),
            self.developer_tip.clone(),
            self.alice_hermes_funding_amount,
            self.refund_policy.clone(),
            self.alice_rpc_port,
        )
        .await;

        self.alice_handle = alice_handle;
        self.alice_swap_handle = alice_swap_handle;
        self.alice_rpc_server_handle = alice_rpc_server_handle;
    }

    /// Restart Alice on a fresh listen address, severing the p2p connection
    /// for good: Bob keeps dialing the old address.
    pub async fn restart_alice_unreachable(&mut self) {
        self.alice_handle.abort();
        self.alice_rpc_server_handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let new_listen_port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        self.alice_listen_address = format!("/ip4/127.0.0.1/tcp/{}", new_listen_port)
            .parse()
            .expect("failed to parse Alice's address");

        let (alice_handle, alice_swap_handle, alice_rpc_server_handle) = start_alice(
            &self.alice_seed,
            self.alice_db_path.clone(),
            self.alice_listen_address.clone(),
            self.env_config,
            self.alice_bitcoin_wallet.clone(),
            self.alice_monero_wallet.clone(),
            self.developer_tip.clone(),
            self.alice_hermes_funding_amount,
            self.refund_policy.clone(),
            self.alice_rpc_port,
        )
        .await;

        self.alice_handle = alice_handle;
        self.alice_swap_handle = alice_swap_handle;
        self.alice_rpc_server_handle = alice_rpc_server_handle;
    }

    /// Stops Alice, mutates her persisted state for `swap_id`, writes it back, and
    /// restarts her. Used to simulate a corrupted/malicious swap from Alice's point of view.
    pub async fn corrupt_alice_state(
        &mut self,
        swap_id: Uuid,
        modify: impl FnOnce(&mut AliceState),
    ) {
        // Stop Alice so we have exclusive access to her database.
        self.alice_handle.abort();
        self.alice_rpc_server_handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Load the latest state, apply the mutation, and persist it again.
        {
            let db = SqliteDatabase::open(self.alice_db_path.as_path(), AccessMode::ReadWrite)
                .await
                .unwrap();
            let State::Alice(mut alice_state) = db.get_state(swap_id).await.unwrap() else {
                panic!("expected an Alice state for swap {swap_id}");
            };
            modify(&mut alice_state);
            db.insert_latest_state(swap_id, State::Alice(alice_state))
                .await
                .unwrap();
        }

        // Bring Alice back up so she can answer Bob's cooperative-redeem request.
        self.restart_alice().await;
    }

    pub async fn alice_next_swap(&mut self) -> alice::Swap {
        timeout(Duration::from_secs(20), self.alice_swap_handle.recv())
            .await
            .expect("No Alice swap within 20 seconds, aborting because this test is likely waiting for a swap forever...")
            .unwrap()
    }

    pub fn xmr_amount(&self) -> monero::Amount {
        self.xmr_amount
    }

    pub async fn bob_swap(&mut self) -> (bob::Swap, BobApplicationHandle) {
        let (swap, event_loop) = self.bob_params.new_swap(self.btc_amount).await.unwrap();

        // ensure the wallet is up to date for concurrent swap tests
        swap.bitcoin_wallet.sync().await.unwrap();

        let join_handle = tokio::spawn(event_loop.run());

        (swap, BobApplicationHandle(join_handle))
    }

    pub async fn bob_swap_with_pool(
        &mut self,
        monero_receive_pool: MoneroAddressPool,
    ) -> (bob::Swap, BobApplicationHandle) {
        let (swap, event_loop) = self
            .bob_params
            .new_swap_with_pool(self.btc_amount, monero_receive_pool)
            .await
            .unwrap();

        swap.bitcoin_wallet.sync().await.unwrap();

        let join_handle = tokio::spawn(event_loop.run());

        (swap, BobApplicationHandle(join_handle))
    }

    pub async fn stop_and_resume_bob_from_db(
        &mut self,
        join_handle: BobApplicationHandle,
        swap_id: Uuid,
    ) -> (bob::Swap, BobApplicationHandle) {
        join_handle.abort();

        let (swap, event_loop) = self.bob_params.new_swap_from_db(swap_id).await.unwrap();

        let join_handle = tokio::spawn(event_loop.run());

        (swap, BobApplicationHandle(join_handle))
    }

    pub async fn assert_alice_redeemed(&mut self, state: AliceState) {
        assert!(matches!(state, AliceState::BtcRedeemed));

        assert_eventual_balance(
            self.alice_bitcoin_wallet.as_ref(),
            Ordering::Equal,
            self.alice_redeemed_btc_balance().await,
        )
        .await
        .unwrap();

        assert_eventual_balance(
            &*self.alice_monero_wallet.main_wallet().await,
            Ordering::Less,
            self.alice_redeemed_xmr_balance(),
        )
        .await
        .unwrap();
    }

    pub async fn assert_alice_refunded(&mut self, state: AliceState) {
        assert!(matches!(state, AliceState::XmrRefunded { .. }));

        assert_eventual_balance(
            self.alice_bitcoin_wallet.as_ref(),
            Ordering::Equal,
            self.alice_refunded_btc_balance(),
        )
        .await
        .unwrap();

        // Alice pays fees - comparison does not take exact lock fee into account
        assert_eventual_balance(
            &*self.alice_monero_wallet.main_wallet().await,
            Ordering::Greater,
            self.alice_refunded_xmr_balance(),
        )
        .await
        .unwrap();
    }

    pub async fn assert_alice_withhold_confirmed(&mut self, state: AliceState) {
        assert!(matches!(state, AliceState::BtcWithholdConfirmed { .. }));

        // Same as refunded - Alice still has her XMR back
        assert_eventual_balance(
            self.alice_bitcoin_wallet.as_ref(),
            Ordering::Equal,
            self.alice_refunded_btc_balance(),
        )
        .await
        .unwrap();

        assert_eventual_balance(
            &*self.alice_monero_wallet.main_wallet().await,
            Ordering::Greater,
            self.alice_refunded_xmr_balance(),
        )
        .await
        .unwrap();
    }

    pub async fn assert_alice_mercy_confirmed(&mut self, state: AliceState) {
        assert!(matches!(state, AliceState::BtcMercyConfirmed { .. }));

        // Same as refunded - Alice still has her XMR back
        assert_eventual_balance(
            self.alice_bitcoin_wallet.as_ref(),
            Ordering::Equal,
            self.alice_refunded_btc_balance(),
        )
        .await
        .unwrap();

        assert_eventual_balance(
            &*self.alice_monero_wallet.main_wallet().await,
            Ordering::Greater,
            self.alice_refunded_xmr_balance(),
        )
        .await
        .unwrap();
    }

    pub async fn assert_alice_developer_tip_received(&self) {
        assert_eventual_balance(
            &*self.developer_tip_monero_wallet.main_wallet().await,
            Ordering::Equal,
            self.developer_tip_wallet_received_xmr_balance(),
        )
        .await
        .unwrap();
    }

    /// Wait until `wallet`'s balance reaches at least `expected - tolerance`.
    /// Used by Bob's donation tests where the exact balance is fee-dependent
    /// and we need a refresh-with-timeout loop to absorb sync timing.
    pub async fn assert_eventual_balance_within(
        &self,
        wallet: &monero::Wallets,
        expected_pico: u64,
        tolerance_pico: u64,
    ) {
        let lower_bound = monero::Amount::from_pico(expected_pico.saturating_sub(tolerance_pico));
        assert_eventual_balance(&*wallet.main_wallet().await, Ordering::Greater, lower_bound)
            .await
            .unwrap();
    }

    pub async fn assert_alice_punished(&self, state: AliceState) {
        let (cancel_fee, punish_fee) = match state {
            AliceState::BtcPunished { state3, .. } => (state3.tx_cancel_fee, state3.tx_punish_fee),
            _ => panic!("Alice is not in btc punished state: {:?}", state),
        };

        assert_eventual_balance(
            self.alice_bitcoin_wallet.as_ref(),
            Ordering::Equal,
            self.alice_punished_btc_balance(cancel_fee, punish_fee)
                .await,
        )
        .await
        .unwrap();

        assert_eventual_balance(
            &*self.alice_monero_wallet.main_wallet().await,
            Ordering::Less,
            self.alice_punished_xmr_balance(),
        )
        .await
        .unwrap();
    }

    pub async fn assert_bob_redeemed(&self, state: BobState) {
        assert_eventual_balance(
            self.bob_bitcoin_wallet.as_ref(),
            Ordering::Equal,
            self.bob_redeemed_btc_balance(state).await.unwrap(),
        )
        .await
        .unwrap();

        assert_eventual_balance(
            &*self.bob_monero_wallet.main_wallet().await,
            Ordering::Greater,
            self.bob_redeemed_xmr_balance(),
        )
        .await
        .unwrap();
    }

    pub async fn assert_bob_refunded(&self, state: BobState) {
        self.bob_bitcoin_wallet.sync().await.unwrap();

        let (lock_tx_id, cancel_fee, refund_fee) = match state {
            BobState::BtcRefunded(state6) => (
                state6.tx_lock_id(),
                state6.tx_cancel_fee,
                state6.tx_refund_fee,
            ),
            _ => panic!("Bob is not in btc refunded state: {:?}", state),
        };
        let lock_tx_bitcoin_fee = self
            .bob_bitcoin_wallet
            .transaction_fee(lock_tx_id)
            .await
            .unwrap();

        let btc_balance_after_swap = self.bob_bitcoin_wallet.balance().await.unwrap();

        let bob_cancelled_and_refunded = btc_balance_after_swap
            == self.bob_starting_balances.btc - lock_tx_bitcoin_fee - cancel_fee - refund_fee;

        assert!(bob_cancelled_and_refunded);

        assert_eventual_balance(
            &*self.bob_monero_wallet.main_wallet().await,
            Ordering::Equal,
            self.bob_refunded_xmr_balance(),
        )
        .await
        .unwrap();
    }

    pub async fn assert_bob_punished(&self, state: BobState) {
        assert_eventual_balance(
            self.bob_bitcoin_wallet.as_ref(),
            Ordering::Equal,
            self.bob_punished_btc_balance(state).await.unwrap(),
        )
        .await
        .unwrap();

        assert_eventual_balance(
            &*self.bob_monero_wallet.main_wallet().await,
            Ordering::Equal,
            self.bob_punished_xmr_balance(),
        )
        .await
        .unwrap();
    }

    pub async fn assert_bob_partially_refunded(&self, state: BobState) {
        self.bob_bitcoin_wallet.sync().await.unwrap();

        let (lock_tx_id, cancel_fee, partial_refund_fee, amnesty_amount) = match state {
            BobState::BtcPartiallyRefunded(state6) => (
                state6.tx_lock_id(),
                state6.tx_cancel_fee,
                state6.tx_partial_refund_fee.expect("partial refund fee"),
                state6.btc_amnesty_amount.expect("amnesty amount"),
            ),
            _ => panic!("Bob is not in btc partially refunded state: {:?}", state),
        };
        let lock_tx_bitcoin_fee = self
            .bob_bitcoin_wallet
            .transaction_fee(lock_tx_id)
            .await
            .unwrap();

        let btc_balance_after_swap = self.bob_bitcoin_wallet.balance().await.unwrap();
        let expected_balance = self.bob_starting_balances.btc
            - lock_tx_bitcoin_fee
            - cancel_fee
            - partial_refund_fee
            - amnesty_amount;

        assert_eq!(btc_balance_after_swap, expected_balance);
    }

    pub async fn assert_bob_amnesty_received(&self, state: BobState) {
        self.bob_bitcoin_wallet.sync().await.unwrap();

        let (lock_tx_id, cancel_fee, partial_refund_fee, amnesty_fee) = match state {
            BobState::BtcReclaimConfirmed(state6) => (
                state6.tx_lock_id(),
                state6.tx_cancel_fee,
                state6.tx_partial_refund_fee.expect("partial refund fee"),
                state6.tx_reclaim_fee.expect("amnesty fee"),
            ),
            _ => panic!("Bob is not in btc amnesty confirmed state: {:?}", state),
        };
        let lock_tx_bitcoin_fee = self
            .bob_bitcoin_wallet
            .transaction_fee(lock_tx_id)
            .await
            .unwrap();

        let btc_balance_after_swap = self.bob_bitcoin_wallet.balance().await.unwrap();
        // Bob gets full amount back minus all the fees
        let expected_balance = self.bob_starting_balances.btc
            - lock_tx_bitcoin_fee
            - cancel_fee
            - partial_refund_fee
            - amnesty_fee;

        assert_eq!(btc_balance_after_swap, expected_balance);
    }

    pub async fn assert_bob_withheld(&self, state: BobState) {
        self.bob_bitcoin_wallet.sync().await.unwrap();

        let (lock_tx_id, cancel_fee, partial_refund_fee, amnesty_amount) = match state {
            BobState::BtcWithheld(state6) => (
                state6.tx_lock_id(),
                state6.tx_cancel_fee,
                state6.tx_partial_refund_fee.expect("partial refund fee"),
                state6.btc_amnesty_amount.expect("amnesty amount"),
            ),
            _ => panic!("Bob is not in btc withheld state: {:?}", state),
        };
        let lock_tx_bitcoin_fee = self
            .bob_bitcoin_wallet
            .transaction_fee(lock_tx_id)
            .await
            .unwrap();

        let btc_balance_after_swap = self.bob_bitcoin_wallet.balance().await.unwrap();
        // Bob lost the amnesty amount (it was withheld)
        let expected_balance = self.bob_starting_balances.btc
            - lock_tx_bitcoin_fee
            - cancel_fee
            - partial_refund_fee
            - amnesty_amount;

        assert_eq!(btc_balance_after_swap, expected_balance);
    }

    pub async fn assert_bob_mercy_received(&self, state: BobState) {
        self.bob_bitcoin_wallet.sync().await.unwrap();

        let (lock_tx_id, cancel_fee, partial_refund_fee, mercy_fee) = match state {
            BobState::BtcMercyConfirmed(state6) => (
                state6.tx_lock_id(),
                state6.tx_cancel_fee,
                state6.tx_partial_refund_fee.expect("partial refund fee"),
                state6.tx_mercy_fee.expect("mercy fee"),
            ),
            _ => panic!("Bob is not in btc mercy confirmed state: {:?}", state),
        };
        let lock_tx_bitcoin_fee = self
            .bob_bitcoin_wallet
            .transaction_fee(lock_tx_id)
            .await
            .unwrap();

        let btc_balance_after_swap = self.bob_bitcoin_wallet.balance().await.unwrap();
        // Bob gets full amount back via mercy
        let expected_balance = self.bob_starting_balances.btc
            - lock_tx_bitcoin_fee
            - cancel_fee
            - partial_refund_fee
            - mercy_fee;

        assert_eq!(btc_balance_after_swap, expected_balance);
    }

    fn alice_redeemed_xmr_balance(&self) -> monero::Amount {
        self.alice_starting_balances.xmr - self.xmr_amount
    }

    async fn alice_redeemed_btc_balance(&self) -> bitcoin::Amount {
        // Get the last transaction Alice published
        // This should be btc_redeem
        let txid = self
            .alice_bitcoin_wallet
            .last_published_txid()
            .await
            .unwrap();

        // Get the fee for the last transaction
        let fee = self
            .alice_bitcoin_wallet
            .transaction_fee(txid)
            .await
            .expect("To estimate fee correctly");

        self.alice_starting_balances.btc + self.btc_amount - fee
    }

    fn bob_redeemed_xmr_balance(&self) -> monero::Amount {
        self.bob_starting_balances.xmr
    }

    async fn bob_redeemed_btc_balance(&self, state: BobState) -> Result<bitcoin::Amount> {
        self.bob_bitcoin_wallet.sync().await?;

        let lock_tx_id = if let BobState::XmrRedeemed { tx_lock_id } = state {
            tx_lock_id
        } else {
            bail!("Bob in not in xmr redeemed state: {:?}", state);
        };

        let lock_tx_bitcoin_fee = self.bob_bitcoin_wallet.transaction_fee(lock_tx_id).await?;

        Ok(self.bob_starting_balances.btc - self.btc_amount - lock_tx_bitcoin_fee)
    }

    fn alice_refunded_xmr_balance(&self) -> monero::Amount {
        self.alice_starting_balances.xmr - self.xmr_amount
    }

    fn developer_tip_wallet_received_xmr_balance(&self) -> monero::Amount {
        use rust_decimal::prelude::ToPrimitive;

        let effective_tip_amount = monero::Amount::from_pico(
            self.developer_tip
                .ratio
                .saturating_mul(Decimal::from(self.xmr_amount.as_pico()))
                .to_u64()
                .unwrap(),
        );

        // This is defined in `swap/src/protocol/alice/swap.rs` in `build_transfer_destinations`
        if effective_tip_amount.as_pico() < 30_000_000 {
            return monero::Amount::ZERO;
        }

        effective_tip_amount
    }

    fn alice_refunded_btc_balance(&self) -> bitcoin::Amount {
        self.alice_starting_balances.btc
    }

    fn bob_refunded_xmr_balance(&self) -> monero::Amount {
        self.bob_starting_balances.xmr
    }

    fn alice_punished_xmr_balance(&self) -> monero::Amount {
        self.alice_starting_balances.xmr - self.xmr_amount
    }

    async fn alice_punished_btc_balance(
        &self,
        cancel_fee: bitcoin::Amount,
        punish_fee: bitcoin::Amount,
    ) -> bitcoin::Amount {
        self.alice_starting_balances.btc + self.btc_amount - cancel_fee - punish_fee
    }

    fn bob_punished_xmr_balance(&self) -> monero::Amount {
        self.bob_starting_balances.xmr
    }

    async fn bob_punished_btc_balance(&self, state: BobState) -> Result<bitcoin::Amount> {
        self.bob_bitcoin_wallet.sync().await?;

        let lock_tx_id = if let BobState::BtcPunished { tx_lock_id, .. } = state {
            tx_lock_id
        } else {
            bail!("Bob in not in btc punished state: {:?}", state);
        };

        let lock_tx_bitcoin_fee = self.bob_bitcoin_wallet.transaction_fee(lock_tx_id).await?;

        Ok(self.bob_starting_balances.btc - self.btc_amount - lock_tx_bitcoin_fee)
    }

    pub async fn stop_alice_monero_wallet_rpc(&self) {
        tracing::info!("Killing monerod container");

        // Use Docker CLI to forcefully kill the container
        let output = tokio::process::Command::new("docker")
            .args(["kill", &self.monerod_container_id])
            .output()
            .await
            .expect("Failed to execute docker kill command");

        if output.status.success() {
            tracing::info!(
                "Successfully killed monerod container: {}",
                &self.monerod_container_id
            );
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::error!(
                "Failed to kill monerod container {}: {}",
                &self.monerod_container_id,
                stderr
            );
        }
    }

    pub async fn empty_alice_monero_wallet(&self) {
        let burn_address = monero_address::MoneroAddress::from_str_with_unchecked_network("49LEH26DJGuCyr8xzRAzWPUryzp7bpccC7Hie1DiwyfJEyUKvMFAethRLybDYrFdU1eHaMkKQpUPebY4WT3cSjEvThmpjPa").unwrap();
        let wallet = self.alice_monero_wallet.main_wallet().await;

        wallet
            .sweep(&burn_address)
            .await
            .expect("Failed to empty alice monero wallet to burn address");
    }

    pub async fn assert_alice_monero_wallet_empty(&self) {
        let wallet = self.alice_monero_wallet.main_wallet().await;
        assert_eventual_balance(&*wallet, Ordering::Equal, monero::Amount::ZERO)
            .await
            .unwrap();
    }
}

async fn assert_eventual_balance<A: fmt::Display + PartialOrd>(
    wallet: &impl Wallet<Amount = A>,
    ordering: Ordering,
    expected: A,
) -> Result<()> {
    let ordering_str = match ordering {
        Ordering::Less => "less than",
        Ordering::Equal => "equal to",
        Ordering::Greater => "greater than",
    };

    let mut current_balance = wallet.get_balance().await?;

    let assertion = async {
        while current_balance.partial_cmp(&expected).unwrap() != ordering {
            tokio::time::sleep(Duration::from_millis(500)).await;

            wallet.refresh().await?;
            current_balance = wallet.get_balance().await?;
        }

        tracing::debug!(
            "Assertion successful! Balance {} is {} {}",
            current_balance,
            ordering_str,
            expected
        );

        Result::<_, anyhow::Error>::Ok(())
    };

    let timeout = Duration::from_secs(10);

    tokio::time::timeout(timeout, assertion)
        .await
        .with_context(|| {
            format!(
                "Expected balance to be {} {} after at most {}s but was {}",
                ordering_str,
                expected,
                timeout.as_secs(),
                current_balance
            )
        })??;

    Ok(())
}

#[async_trait]
trait Wallet {
    type Amount;

    fn refresh(&self) -> impl Future<Output = Result<()>>;
    fn get_balance(&self) -> impl Future<Output = Result<Self::Amount>>;
}

impl Wallet for monero::Wallet {
    type Amount = monero::Amount;

    async fn refresh(&self) -> Result<()> {
        self.wait_until_synced(no_listener()).await
    }

    async fn get_balance(&self) -> Result<Self::Amount> {
        Ok(self.total_balance().await?.into())
    }
}

impl Wallet for bitcoin_wallet::Wallet {
    type Amount = bitcoin::Amount;

    async fn refresh(&self) -> Result<()> {
        self.sync().await
    }

    async fn get_balance(&self) -> Result<Self::Amount> {
        self.balance().await
    }
}

fn random_prefix() -> String {
    use rand::distributions::Alphanumeric;
    use rand::{Rng, thread_rng};
    use std::iter;
    const LEN: usize = 8;
    let mut rng = thread_rng();
    let chars: String = iter::repeat(())
        .map(|()| rng.sample(Alphanumeric))
        .map(char::from)
        .take(LEN)
        .collect();
    chars
}

async fn mine(bitcoind_client: Client, reward_address: bitcoin::Address) -> Result<()> {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        bitcoind_client
            .generatetoaddress(1, reward_address.clone())
            .await?;
    }
}

async fn init_bitcoind(node_url: Url, spendable_quantity: u32) -> Result<Client> {
    let bitcoind_client = Client::new(node_url.clone());

    bitcoind_client
        .createwallet(BITCOIN_TEST_WALLET_NAME, None, None, None, None)
        .await?;

    let reward_address = bitcoind_client
        .with_wallet(BITCOIN_TEST_WALLET_NAME)?
        .getnewaddress(None, None)
        .await?;

    let reward_address = reward_address.require_network(bitcoind_client.network().await?)?;

    bitcoind_client
        .generatetoaddress(101 + spendable_quantity, reward_address.clone())
        .await?;
    tokio::spawn(mine(bitcoind_client.clone(), reward_address));
    Ok(bitcoind_client)
}

/// Send Bitcoin to the specified address, limited to the spendable bitcoin
/// quantity.
pub async fn mint(node_url: Url, address: bitcoin::Address, amount: bitcoin::Amount) -> Result<()> {
    let bitcoind_client = Client::new(node_url.clone());

    bitcoind_client
        .send_to_address(BITCOIN_TEST_WALLET_NAME, address.clone(), amount)
        .await?;

    // Confirm the transaction
    let reward_address = bitcoind_client
        .with_wallet(BITCOIN_TEST_WALLET_NAME)?
        .getnewaddress(None, None)
        .await?;

    let reward_address = reward_address.require_network(bitcoind_client.network().await?)?;

    bitcoind_client.generatetoaddress(1, reward_address).await?;

    Ok(())
}

// This is just to keep the containers alive
struct Containers<'a> {
    bitcoind_url: Url,
    _bitcoind: Container<'a, bitcoind::Bitcoind>,
    _monerod_container: Container<'a, image::Monerod>,
    _monero_wallet_rpc_containers: Vec<Container<'a, image::MoneroWalletRpc>>,
    electrs: Container<'a, electrs::Electrs>,
}

pub mod alice_run_until {
    use swap::protocol::alice::AliceState;

    pub fn is_btc_lock_transaction_seen(state: &AliceState) -> bool {
        matches!(state, AliceState::BtcLockTransactionSeen { .. })
    }

    pub fn is_xmr_lock_transaction_sent(state: &AliceState) -> bool {
        matches!(state, AliceState::XmrLockTransactionSent { .. })
    }

    pub fn is_encsig_learned(state: &AliceState) -> bool {
        matches!(state, AliceState::EncSigLearned { .. })
    }

    pub fn is_btc_redeemed(state: &AliceState) -> bool {
        matches!(state, AliceState::BtcRedeemed { .. })
    }

    pub fn is_btc_partially_refunded(state: &AliceState) -> bool {
        matches!(state, AliceState::BtcPartiallyRefunded { .. })
    }

    pub fn is_xmr_refunded(state: &AliceState) -> bool {
        matches!(state, AliceState::XmrRefunded { .. })
    }

    pub fn is_btc_withhold_confirmed(state: &AliceState) -> bool {
        matches!(state, AliceState::BtcWithholdConfirmed { .. })
    }

    pub fn is_btc_mercy_confirmed(state: &AliceState) -> bool {
        matches!(state, AliceState::BtcMercyConfirmed { .. })
    }
}

pub mod bob_run_until {
    use swap::protocol::bob::BobState;

    pub fn is_btc_locked(state: &BobState) -> bool {
        matches!(state, BobState::BtcLocked { .. })
    }

    pub fn is_lock_proof_received(state: &BobState) -> bool {
        matches!(state, BobState::XmrLockTransactionCandidate { .. })
    }

    pub fn is_xmr_locked(state: &BobState) -> bool {
        matches!(state, BobState::XmrLocked(..))
    }

    pub fn is_encsig_sent(state: &BobState) -> bool {
        matches!(state, BobState::EncSigSent { .. })
    }

    pub fn is_btc_partially_refunded(state: &BobState) -> bool {
        matches!(state, BobState::BtcPartiallyRefunded(..))
    }

    pub fn is_waiting_for_remaining_refund_timelock(state: &BobState) -> bool {
        matches!(state, BobState::WaitingForReclaimTimelockExpiration(..))
    }

    pub fn is_remaining_refund_timelock_expired(state: &BobState) -> bool {
        matches!(state, BobState::ReclaimTimelockExpired(..))
    }

    pub fn is_btc_amnesty_confirmed(state: &BobState) -> bool {
        matches!(state, BobState::BtcReclaimConfirmed(..))
    }

    pub fn is_btc_withheld(state: &BobState) -> bool {
        matches!(state, BobState::BtcWithheld(..))
    }

    pub fn is_btc_mercy_confirmed(state: &BobState) -> bool {
        matches!(state, BobState::BtcMercyConfirmed(..))
    }
}

pub struct SlowCancelConfig;

impl GetConfig for SlowCancelConfig {
    fn get_config() -> Config {
        Config {
            bitcoin_cancel_timelock: CancelTimelock::new(180).into(),
            ..env::Regtest::get_config()
        }
    }
}

pub struct FastCancelConfig;

impl GetConfig for FastCancelConfig {
    fn get_config() -> Config {
        Config {
            bitcoin_cancel_timelock: CancelTimelock::new(10).into(),
            ..env::Regtest::get_config()
        }
    }
}

pub struct FastPunishConfig;

impl GetConfig for FastPunishConfig {
    fn get_config() -> Config {
        Config {
            bitcoin_cancel_timelock: CancelTimelock::new(10).into(),
            bitcoin_punish_timelock: PunishTimelock::new(10).into(),
            ..env::Regtest::get_config()
        }
    }
}

pub struct FastAmnestyConfig;

impl GetConfig for FastAmnestyConfig {
    fn get_config() -> Config {
        Config {
            bitcoin_cancel_timelock: CancelTimelock::new(10).into(),
            bitcoin_remaining_refund_timelock: 3,
            ..env::Regtest::get_config()
        }
    }
}

/// Config with a longer remaining refund timelock for burn tests.
/// Alice needs time to refund XMR (which waits for 10 Monero confirmations)
/// before publishing the burn transaction.
pub struct SlowAmnestyConfig;

impl GetConfig for SlowAmnestyConfig {
    fn get_config() -> Config {
        Config {
            bitcoin_cancel_timelock: CancelTimelock::new(10).into(),
            // Much longer timelock to give Alice time to:
            // 1. Wait for 10 Monero confirmations on the lock tx
            // 2. Sweep the XMR to her wallet
            // 3. Then publish the burn transaction
            // In regtest, each BTC block is ~5s, so 100 blocks is ~8 minutes
            bitcoin_remaining_refund_timelock: 100,
            ..env::Regtest::get_config()
        }
    }
}
