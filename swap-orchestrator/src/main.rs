mod compose;
mod containers;
mod images;
mod keygen;
mod prompt;

use swap_orchestrator as _;

use crate::compose::{
    ASB_DATA_DIR, CloudflaredConfig, DOCKER_COMPOSE_FILE, IntoSpec, MetricsConfig,
    OrchestratorDirectories, OrchestratorImage, OrchestratorImages, OrchestratorInput,
    OrchestratorNetworks, PROMETHEUS_CONFIG_FILE, PROMTAIL_CONFIG_FILE, PromtailConfig,
    build_prometheus_agent_yml, build_promtail_yml,
};
use libp2p::Multiaddr;
use libp2p::multiaddr::Protocol;
use std::path::PathBuf;
use std::str::FromStr;
use swap_env::config::{
    Bitcoin, Config, ConfigNotInitialized, Data, Maker, Monero, Network, TorConf,
    default_price_ticker_rest_poll_interval_exolix_secs, default_price_ticker_source_enabled,
    default_price_ticker_validity_duration_secs,
};
use swap_env::prompt as config_prompt;
use swap_env::{defaults::GetDefaults, env::Mainnet, env::Testnet};
use url::Url;

/// Environment variables that together configure the Cloudflare Tunnel
/// integration. Either all of them must be set, or none — a partial set
/// is a hard error.
const CLOUDFLARE_ENV_VARS: [&str; 4] = [
    "CLOUDFLARE_TUNNEL_TOKEN",
    "CLOUDFLARE_TUNNEL_EXTERNAL_HOST",
    "CLOUDFLARE_TUNNEL_EXTERNAL_PORT",
    "CLOUDFLARE_TUNNEL_INTERNAL_PORT",
];

/// Environment variables that together configure the Promtail log shipper.
/// Either all of them must be set, or none — a partial set is a hard error
/// so half-configured log shipping cannot ship silently.
const PROMTAIL_ENV_VARS: [&str; 3] = [
    "PROMTAIL_LOKI_PUSH_URL",
    "PROMTAIL_LOKI_PUSH_TOKEN",
    "PROMTAIL_INSTANCE",
];

/// Reads the Cloudflare Tunnel configuration from the environment.
///
/// Returns `None` if none of the variables are set. Returns `Some(..)` if
/// all of them are set. Panics if the set is partially populated, because
/// a half-configured tunnel would silently ship a broken deployment.
fn read_cloudflared_config_from_env() -> Option<CloudflaredConfig> {
    let present: Vec<&str> = CLOUDFLARE_ENV_VARS
        .iter()
        .copied()
        .filter(|name| std::env::var(name).is_ok())
        .collect();

    if present.is_empty() {
        return None;
    }

    if present.len() != CLOUDFLARE_ENV_VARS.len() {
        let missing: Vec<&str> = CLOUDFLARE_ENV_VARS
            .iter()
            .copied()
            .filter(|name| std::env::var(name).is_err())
            .collect();
        panic!(
            "Cloudflare Tunnel is partially configured. The following variables are set: {:?}, but these are missing: {:?}. Set all four or none.",
            present, missing
        );
    }

    let token = std::env::var("CLOUDFLARE_TUNNEL_TOKEN").expect("checked above");
    let external_host = std::env::var("CLOUDFLARE_TUNNEL_EXTERNAL_HOST").expect("checked above");
    let external_port: u16 = std::env::var("CLOUDFLARE_TUNNEL_EXTERNAL_PORT")
        .expect("checked above")
        .parse()
        .expect("CLOUDFLARE_TUNNEL_EXTERNAL_PORT must be a valid u16");
    let internal_port: u16 = std::env::var("CLOUDFLARE_TUNNEL_INTERNAL_PORT")
        .expect("checked above")
        .parse()
        .expect("CLOUDFLARE_TUNNEL_INTERNAL_PORT must be a valid u16");

    Some(CloudflaredConfig {
        token,
        external_host,
        external_port,
        internal_port,
    })
}

/// Reads the Promtail log-shipper configuration from the environment.
///
/// Returns `None` if none of the variables are set. Returns `Some(..)` if
/// all of them are set. Panics if the set is partially populated or if the
/// instance label is empty — a half-configured shipper would silently fail
/// to deliver logs.
fn read_promtail_config_from_env() -> Option<PromtailConfig> {
    let present: Vec<&str> = PROMTAIL_ENV_VARS
        .iter()
        .copied()
        .filter(|name| std::env::var(name).is_ok())
        .collect();

    if present.is_empty() {
        return None;
    }

    if present.len() != PROMTAIL_ENV_VARS.len() {
        let missing: Vec<&str> = PROMTAIL_ENV_VARS
            .iter()
            .copied()
            .filter(|name| std::env::var(name).is_err())
            .collect();
        panic!(
            "Promtail log shipper is partially configured. The following variables are set: {:?}, but these are missing: {:?}. Set all three or none.",
            present, missing
        );
    }

    let loki_push_url = std::env::var("PROMTAIL_LOKI_PUSH_URL").expect("checked above");
    let loki_push_token = std::env::var("PROMTAIL_LOKI_PUSH_TOKEN").expect("checked above");
    let instance = std::env::var("PROMTAIL_INSTANCE").expect("checked above");

    if instance.trim().is_empty() {
        panic!(
            "PROMTAIL_INSTANCE must be a non-empty short identifier for this host (e.g. asb-de-1)."
        );
    }
    if loki_push_token.trim().is_empty() {
        panic!("PROMTAIL_LOKI_PUSH_TOKEN must not be empty.");
    }
    if loki_push_url.trim().is_empty() {
        panic!("PROMTAIL_LOKI_PUSH_URL must not be empty.");
    }

    Some(PromtailConfig {
        loki_push_url,
        loki_push_token,
        instance,
    })
}

fn read_metrics_config_from_env(promtail: Option<&PromtailConfig>) -> Option<MetricsConfig> {
    let remote_write_url = std::env::var("METRICS_REMOTE_WRITE_URL").ok()?;

    if remote_write_url.trim().is_empty() {
        panic!("METRICS_REMOTE_WRITE_URL must not be empty.");
    }

    let promtail = promtail.unwrap_or_else(|| {
        panic!(
            "METRICS_REMOTE_WRITE_URL is set but Promtail is not configured. Metrics reuse the Promtail bearer token and instance label, so set the PROMTAIL_* variables as well."
        )
    });

    Some(MetricsConfig {
        remote_write_url,
        token: promtail.loki_push_token.clone(),
        instance: promtail.instance.clone(),
    })
}

/// `GH_TOKEN` for fetching a private build-context repository; `None` if unset
/// or empty. See [`images::source_build_context`].
fn read_gh_token_from_env() -> Option<String> {
    let token = std::env::var("GH_TOKEN").ok()?;
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("gen-rpc-auth") {
        keygen::generate_rpc_auth_keyfile();
        return;
    }

    // Cloudflare Tunnel is opt-in via env vars so existing deployments
    // keep working unchanged.
    let cloudflared_config = read_cloudflared_config_from_env();
    // Promtail log shipping is opt-in via env vars; same rationale as the
    // Cloudflare integration above.
    let promtail_config = read_promtail_config_from_env();
    let metrics_config = read_metrics_config_from_env(promtail_config.as_ref());
    // Opt-in: inlined into the build-context URL so Docker can fetch a private repo.
    let gh_token = read_gh_token_from_env();
    let source_build_context = images::source_build_context(gh_token.as_deref());

    let want_tor = prompt::tor_for_daemons();
    let (bitcoin_network, monero_network) = prompt::network();

    let defaults = match (bitcoin_network, monero_network) {
        (bitcoin::Network::Bitcoin, monero_address::Network::Mainnet) => {
            Mainnet::get_config_file_defaults().expect("defaults to be available")
        }
        (bitcoin::Network::Testnet, monero_address::Network::Stagenet) => {
            Testnet::get_config_file_defaults().expect("defaults to be available")
        }
        _ => panic!("Unsupported Bitcoin / Monero network combination"),
    };

    let recipe = OrchestratorInput {
        ports: OrchestratorNetworks {
            monero: monero_network,
            bitcoin: bitcoin_network,
        }
        .into(),
        networks: OrchestratorNetworks {
            monero: monero_network,
            bitcoin: bitcoin_network,
        },
        images: OrchestratorImages {
            // TODO: These containers should be conditionally removed / disabled,
            // depending on if they are used by the asb
            monerod: OrchestratorImage::Registry(images::MONEROD_IMAGE.to_string()),
            electrs: OrchestratorImage::Registry(images::ELECTRS_IMAGE.to_string()),
            bitcoind: OrchestratorImage::Registry(images::BITCOIND_IMAGE.to_string()),
            tor: OrchestratorImage::Registry(images::TOR_IMAGE.to_string()),
            // TODO: Allow pre-built images here
            asb: OrchestratorImage::Build(images::asb_image_from_source(&source_build_context)),
            // TODO: Allow pre-built images here
            asb_controller: OrchestratorImage::Build(images::asb_controller_image_from_source(
                &source_build_context,
            )),
            asb_tracing_logger: OrchestratorImage::Registry(
                images::ASB_TRACING_LOGGER_IMAGE.to_string(),
            ),
            rendezvous_node: OrchestratorImage::Build(images::rendezvous_node_image_from_source(
                &source_build_context,
            )),
            cloudflared: OrchestratorImage::Registry(images::CLOUDFLARED_IMAGE.to_string()),
            promtail: OrchestratorImage::Registry(images::PROMTAIL_IMAGE.to_string()),
            docker_socket_proxy: OrchestratorImage::Registry(
                images::DOCKER_SOCKET_PROXY_IMAGE.to_string(),
            ),
            cadvisor: OrchestratorImage::Registry(images::CADVISOR_IMAGE.to_string()),
            prometheus_agent: OrchestratorImage::Registry(images::PROMETHEUS_IMAGE.to_string()),
            bitcoin_exporter: OrchestratorImage::Registry(
                images::BITCOIN_PROMETHEUS_EXPORTER_IMAGE.to_string(),
            ),
        },
        directories: OrchestratorDirectories {
            asb_data_dir: PathBuf::from(ASB_DATA_DIR),
        },
        want_tor,
        cloudflared: cloudflared_config.clone(),
        promtail: promtail_config.clone(),
        metrics: metrics_config.clone(),
    };

    // If the config file already exists and be de-serialized,
    // we give the user the ability to skip the setup for the "asb config" (config.toml)
    //
    // The "asb config" is distinctly different from the [`monero_node_type`] and [`electrum_server_type`]
    // since these are also required to decide on the structure of the `docker-compose.yml` file
    enum ConfigExistence {
        PresentAndValid,
        Missing,
        PresentButInvalid(anyhow::Error),
    }

    let asb_config_state = match swap_env::config::read_config(
        recipe.directories.asb_config_path_on_host_as_path_buf(),
    ) {
        Ok(Ok(_)) => ConfigExistence::PresentAndValid,
        Ok(Err(ConfigNotInitialized)) => ConfigExistence::Missing,
        Err(err) => ConfigExistence::PresentButInvalid(err),
    };

    // None, means to do nothing because we already have a valid file at the correct location
    // Some(None) means to generate a config from scratch
    // Some(Some(PathBuf)) means to move the file at the location of the config file to the path
    let should_prompt_config_wizard = match asb_config_state {
        // Config is present and valid => we do not need to prompt a wizard
        ConfigExistence::PresentAndValid => None,
        // Config file is missing => force wizard
        ConfigExistence::Missing => Some(None),
        // Config is present but invalid => Ask user to rename old config file and generate new one
        ConfigExistence::PresentButInvalid(err) => {
            println!("The asb config is present but it is invalid. We were unable to parse it.");
            println!("{:?}", err);
            println!("Do you want to re-generate your asb config from scratch?");

            let unix_epoch = unix_epoch_secs();

            let renamed_file_name = format!(
                "{}.backup_at_{}",
                recipe
                    .directories
                    .asb_config_path_on_host_as_path_buf()
                    .file_name()
                    .expect("asb config file to have filename")
                    .to_str()
                    .expect("asb config file to fit into non OsStr"),
                unix_epoch
            );

            println!(
                "Your previous (invalid) config will be renamed to {}",
                renamed_file_name
            );

            let renamed_path = recipe
                .directories
                .asb_config_path_on_host_as_path_buf()
                .with_file_name(renamed_file_name);

            Some(Some(renamed_path))
        }
    };

    // If the config is invalid or doesn't exist, we prompt the user
    if let Some(should_move_old_file) = should_prompt_config_wizard {
        let min_buy_btc =
            config_prompt::min_buy_amount().expect("Failed to prompt for min buy amount");
        let max_buy_btc =
            config_prompt::max_buy_amount().expect("Failed to prompt for max buy amount");
        let ask_spread = config_prompt::ask_spread().expect("Failed to prompt for ask spread");
        let rendezvous_points =
            config_prompt::rendezvous_points().expect("Failed to prompt for rendezvous points");
        let tor_hidden_service =
            config_prompt::tor_hidden_service().expect("Failed to prompt for tor hidden service");
        let listen_addresses = config_prompt::listen_addresses(&defaults.listen_address_tcp)
            .expect("Failed to prompt for listen addresses");
        let monero_node_type = prompt::monero_node_type();
        let electrum_server_type = prompt::electrum_server_type(&defaults.electrum_rpc_urls);
        let developer_tip =
            config_prompt::developer_tip().expect("Failed to prompt for developer tip");

        let electrs_url = Url::parse(&format!("tcp://electrs:{}", recipe.ports.electrs))
            .expect("electrs url to be convertible to a valid url");
        let monerod_daemon_url =
            Url::parse(&format!("http://monerod:{}", recipe.ports.monerod_rpc))
                .expect("monerod daemon url to be convertible to a valid url");

        let config = Config {
            data: Data {
                dir: recipe.directories.asb_data_dir.clone(),
            },
            network: Network {
                listen: listen_addresses,
                rendezvous_point: rendezvous_points,
                external_addresses: vec![],
                prometheus_port: None,
            },
            bitcoin: Bitcoin {
                electrum_rpc_urls: match electrum_server_type {
                    // If user chose the included option, we will use the electrs url from the container
                    prompt::ElectrumServerType::Included => vec![electrs_url],
                    prompt::ElectrumServerType::Remote(electrum_servers) => electrum_servers,
                },
                network: bitcoin_network,
                target_block: defaults.bitcoin_confirmation_target,
                use_mempool_space_fee_estimation: defaults.use_mempool_space_fee_estimation,
                // This means that we will use the default set in swap-env/src/env.rs
                finality_confirmations: None,
            },
            monero: Monero {
                daemon_url: match monero_node_type.clone() {
                    prompt::MoneroNodeType::Included => Some(monerod_daemon_url),
                    prompt::MoneroNodeType::Pool => None,
                    prompt::MoneroNodeType::Remote(url) => Some(url),
                },
                network: monero_network,
                // This means that we will use the default set in swap-env/src/env.rs
                finality_confirmations: None,
                trusted_daemon: false,
                lock_rebuild_confirmations: 15,
            },
            tor: TorConf {
                register_hidden_service: tor_hidden_service,
                ..Default::default()
            },
            maker: Maker {
                min_buy_btc,
                max_buy_btc,
                ask_spread,
                hermes_funding_amount_piconero:
                    swap_env::config::default_hermes_funding_amount_piconero(),
                hermes_enabled: swap_env::config::default_hermes_enabled(),
                hermes_min_swap_amount: swap_env::config::default_hermes_min_swap_amount(),
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
                btc_redeem_fee_multiplier: swap_env::config::default_btc_redeem_fee_multiplier(),
                refund_policy: defaults.refund_policy,
                developer_tip,
            },
        };

        // If there was an invalid config file previously, we rename it
        if let Some(move_invalid_config_to) = should_move_old_file {
            std::fs::rename(
                recipe.directories.asb_config_path_on_host(),
                move_invalid_config_to,
            )
            .expect("to be able to move old invalid config file");
        }

        // Write the asb config to the host, the config will then be mounted into the `asb` docker container
        let asb_config_path = recipe.directories.asb_config_path_on_host();

        std::fs::write(
            asb_config_path,
            toml::to_string(&config).expect("Failed to write config.toml"),
        )
        .expect("Failed to write config.toml");
    }

    // If Cloudflare Tunnel is enabled, ensure the ASB config advertises the
    // WebSocket listen address and the public wss external address. We do this
    // after the wizard branch so it applies whether the config was just
    // generated or already existed on disk.
    if let Some(cf) = cloudflared_config.as_ref() {
        ensure_cloudflared_addresses_in_config(&recipe, cf);
    }

    if let Some(promtail) = promtail_config.as_ref() {
        std::fs::write(PROMTAIL_CONFIG_FILE, build_promtail_yml(promtail))
            .expect("Failed to write promtail.yml");
    }

    if let Some(metrics) = metrics_config.as_ref() {
        ensure_prometheus_port_in_config(&recipe);
        std::fs::write(
            PROMETHEUS_CONFIG_FILE,
            build_prometheus_agent_yml(
                metrics,
                recipe.ports.asb_metrics_port,
                cloudflared_config.is_some(),
            ),
        )
        .expect("Failed to write prometheus.yml");
    }

    // Write the compose to ./docker-compose.yml
    let compose = recipe.to_spec();
    std::fs::write(DOCKER_COMPOSE_FILE, compose).expect("Failed to write docker-compose.yml");

    println!();
    if !std::path::Path::new(compose::ASB_RPC_AUTH_FILE_ON_HOST).exists() {
        let generate_now = dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
            .with_prompt(format!(
                "No RPC auth keyfile found at {}. Without it the asb will refuse to start its RPC server. Generate one now?",
                compose::ASB_RPC_AUTH_FILE_ON_HOST
            ))
            .default(true)
            .interact()
            .expect("Failed to read confirmation");

        if generate_now {
            keygen::generate_rpc_auth_keyfile();
        } else {
            println!("Run `orchestrator gen-rpc-auth` before starting the services.");
        }
        println!();
    }
    println!("Run `docker compose up -d` to start the services.");

    if let Some(cf) = cloudflared_config.as_ref() {
        print_cloudflared_instructions(cf);
    }

    if let Some(promtail) = promtail_config.as_ref() {
        print_promtail_instructions(promtail);
    }

    if let Some(metrics) = metrics_config.as_ref() {
        print_metrics_instructions(metrics);
    }
}

/// Reads the ASB config from disk, inserts the WebSocket listen address and
/// the public wss external address required by the Cloudflare Tunnel, and
/// writes it back. Idempotent — running this repeatedly does not duplicate
/// entries.
fn ensure_cloudflared_addresses_in_config(recipe: &OrchestratorInput, cf: &CloudflaredConfig) {
    let config_path = recipe.directories.asb_config_path_on_host_as_path_buf();

    let mut config = swap_env::config::read_config(config_path.clone())
        .expect("Failed to read asb config for cloudflared patching")
        .expect("asb config must exist by this point");

    let ws_listen: Multiaddr =
        Multiaddr::from_str(&format!("/ip4/0.0.0.0/tcp/{}/ws", cf.internal_port))
            .expect("ws listen multiaddr to be valid");

    let wss_external: Multiaddr = Multiaddr::from_str(&format!(
        "/dns4/{}/tcp/{}/wss",
        cf.external_host, cf.external_port
    ))
    .expect("wss external multiaddr to be valid");

    // Reject CLOUDFLARE_TUNNEL_INTERNAL_PORT values that would collide with
    // a TCP port the ASB is already bound to. The ASB binds every entry in
    // `config.network.listen` individually, so a clash produces `AddrInUse`
    // at startup and the tunnel silently never comes up. Also check the
    // well-known orchestrator ports (libp2p TCP + RPC + Prometheus metrics)
    // for the same reason.
    let mut reserved_ports: Vec<u16> = vec![
        recipe.ports.asb_libp2p,
        recipe.ports.asb_rpc_port,
        recipe.ports.asb_metrics_port,
    ];
    for existing in &config.network.listen {
        if existing == &ws_listen {
            continue;
        }
        for proto in existing.iter() {
            if let Protocol::Tcp(port) = proto {
                reserved_ports.push(port);
            }
        }
    }
    if reserved_ports.contains(&cf.internal_port) {
        panic!(
            "CLOUDFLARE_TUNNEL_INTERNAL_PORT={} collides with a port the ASB already binds ({:?}). Pick a different internal port.",
            cf.internal_port, reserved_ports
        );
    }

    if !config.network.listen.contains(&ws_listen) {
        config.network.listen.push(ws_listen);
    }

    if !config.network.external_addresses.contains(&wss_external) {
        config.network.external_addresses.push(wss_external);
    }

    std::fs::write(
        &config_path,
        toml::to_string(&config).expect("Failed to serialize patched config.toml"),
    )
    .expect("Failed to write patched config.toml");
}

/// Points the ASB config's libp2p metrics endpoint at the port the Prometheus
/// agent scrapes. Applied whenever metrics shipping is enabled, so it covers
/// both a freshly generated config and one that already existed on disk.
fn ensure_prometheus_port_in_config(recipe: &OrchestratorInput) {
    let config_path = recipe.directories.asb_config_path_on_host_as_path_buf();

    let mut config = swap_env::config::read_config(config_path.clone())
        .expect("Failed to read asb config for prometheus patching")
        .expect("asb config must exist by this point");

    if config.network.prometheus_port == Some(recipe.ports.asb_metrics_port) {
        return;
    }

    config.network.prometheus_port = Some(recipe.ports.asb_metrics_port);

    std::fs::write(
        &config_path,
        toml::to_string(&config).expect("Failed to serialize patched config.toml"),
    )
    .expect("Failed to write patched config.toml");
}

/// Prints the manual steps the operator must take in the Cloudflare Zero
/// Trust dashboard to finish configuring the tunnel.
fn print_cloudflared_instructions(cf: &CloudflaredConfig) {
    println!();
    println!("Cloudflare Tunnel is enabled. Configure it in the dashboard:");
    println!("  1. Open https://one.dash.cloudflare.com/ -> Networks -> Tunnels");
    println!("  2. Select the tunnel matching your CLOUDFLARE_TUNNEL_TOKEN");
    println!("  3. Under 'Public Hostnames', add (or verify) a hostname with:");
    println!("       - Subdomain / domain: {}", cf.external_host);
    println!("       - Service type: HTTP");
    println!(
        "       - URL: asb:{} (the asb container on the docker network)",
        cf.internal_port
    );
    println!(
        "  4. Peers will reach this ASB at /dns4/{}/tcp/{}/wss",
        cf.external_host, cf.external_port
    );
    println!(
        "  5. Do NOT put a Cloudflare Access policy in front of this hostname — libp2p clients cannot authenticate with it."
    );
}

/// Prints the operator-facing summary for the Promtail log shipper so they
/// can verify it landed and know which Grafana query selects this host.
fn print_promtail_instructions(promtail: &PromtailConfig) {
    println!();
    println!("Promtail log shipping is enabled.");
    println!("  - Instance label (host): {}", promtail.instance);
    println!("  - Loki push URL:         {}", promtail.loki_push_url);
    println!("  - Config written to:     {}", PROMTAIL_CONFIG_FILE);
    println!("  - Ships: asb tracing logs + bitcoind/monerod/electrs container logs");
    println!("  - Verify after `docker compose up -d`:");
    println!("      docker compose logs --tail 50 promtail");
    println!(
        "  - Grafana query (whole host):  {{host=\"{}\"}}",
        promtail.instance
    );
    println!(
        "  - Grafana query (node daemons): {{host=\"{}\", job=\"node\"}}",
        promtail.instance
    );
}

fn print_metrics_instructions(metrics: &MetricsConfig) {
    println!();
    println!("Prometheus metrics shipping is enabled.");
    println!("  - Instance label (host): {}", metrics.instance);
    println!("  - Remote write URL:      {}", metrics.remote_write_url);
    println!("  - Config written to:     {}", PROMETHEUS_CONFIG_FILE);
    println!("  - Ships: per-container cpu/memory/pids/network/fs via cadvisor,");
    println!("           bitcoind metrics via bitcoin-exporter, and electrs metrics");
    println!("  - Verify after `docker compose up -d`:");
    println!("      docker compose logs --tail 50 prometheus-agent");
}

fn unix_epoch_secs() -> u64 {
    std::time::UNIX_EPOCH
        .elapsed()
        .expect("unix epoch to be elapsed")
        .as_secs()
}
