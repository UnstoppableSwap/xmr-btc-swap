use swap_env::{
    config::{Config, validate_config},
    env,
};

fn config(monero_options: &str) -> Config {
    toml::from_str(&format!(
        r#"
[data]
dir = "/tmp/config-test"
[network]
listen = []
[bitcoin]
electrum_rpc_urls = []
target_block = 1
network = "Mainnet"
[monero]
network = "Mainnet"
{monero_options}
[tor]
register_hidden_service = false
hidden_service_num_intro_points = 5
[maker]
min_buy_btc = 0.001
max_buy_btc = 1.0
ask_spread = 0.01
"#
    ))
    .unwrap()
}

#[test]
fn trust_requires_an_explicit_daemon() {
    let config = config("trusted_daemon = true");
    let runtime = env::new(false, &config);
    let error = validate_config(&config, runtime).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("requires an explicit monero.daemon_url")
    );
}

#[test]
fn loaded_trust_setting_is_applied_and_validated() {
    for trust in ["", "trusted_daemon = false", "trusted_daemon = true"] {
        let config = config(&format!("daemon_url = \"http://127.0.0.1:18081\"\n{trust}"));
        let runtime = env::new(false, &config);
        assert_eq!(runtime.monero_trusted_daemon, trust.ends_with("true"));
        validate_config(&config, runtime).unwrap();
    }
}

#[test]
fn rebuild_confirmations_default_override_and_validation() {
    for (options, expected) in [("", 10), ("lock_rebuild_confirmations = 5", 5)] {
        let config = config(options);
        let runtime = env::new(false, &config);
        assert_eq!(runtime.monero_lock_rebuild_confirmations, expected);
        validate_config(&config, runtime).unwrap();
    }
    let config = config("lock_rebuild_confirmations = 0");
    assert!(validate_config(&config, env::new(false, &config)).is_err());
}

#[test]
fn public_pool_remains_available_without_trust() {
    let config = config("");
    validate_config(&config, env::new(false, &config)).unwrap();
}

#[test]
fn cli_network_selection_is_not_overwritten_by_file() {
    let mut config = config("");
    config.bitcoin.finality_confirmations = Some(7);
    config.monero.finality_confirmations = Some(12);
    let runtime = env::new(true, &config);
    assert_eq!(runtime.bitcoin_finality_confirmations, 7);
    assert_eq!(runtime.monero_finality_confirmations, 12);
    // The CLI selected testnet; a mainnet file is rejected instead of overriding it.
    assert!(validate_config(&config, runtime).is_err());
}
