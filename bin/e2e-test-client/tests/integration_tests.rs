use fuel_core::service::{
    Config,
    FuelService,
};
// Add methods on commands
use fuel_core_e2e_client::config::SuiteConfig;
use std::fs;
use tempfile::TempDir; // Used for writing assertions // Run programs

#[tokio::test(flavor = "multi_thread")]
async fn works_in_local_env() {
    // setup a local node
    let srv = setup_local_node().await;
    // generate a config file
    let config = generate_config_file(srv.bound_address.to_string());
    // execute suite
    execute_suite(config.path).await
}

// Spins up a node for each wallet and verifies that the suite works across multiple nodes
#[cfg(feature = "p2p")]
#[tokio::test(flavor = "multi_thread")]
async fn works_in_multinode_local_env() {
    use fuel_core::p2p_test_helpers::*;
    use fuel_core_types::{
        fuel_crypto::{
            rand::{
                prelude::StdRng,
                SeedableRng,
            },
            SecretKey,
        },
        fuel_tx::Input,
    };

    let mut rng = StdRng::seed_from_u64(line!() as u64);
    let secret = SecretKey::random(&mut rng);
    let pub_key = Input::owner(&secret.public_key());
    let Nodes {
        mut producers,
        mut validators,
        bootstrap_nodes: _dont_drop,
    } = make_nodes(
        [Some(BootstrapSetup::new(pub_key))],
        [Some(
            ProducerSetup::new(secret).with_txs(1).with_name("Alice"),
        )],
        [Some(ValidatorSetup::new(pub_key).with_name("Bob"))],
    )
    .await;

    let producer = producers.pop().unwrap();
    let validator = validators.pop().unwrap();

    // generate a config file without a default endpoint to
    // verify wallets connect to different nodes
    let mut config = SuiteConfig {
        endpoint: "".to_string(),
        ..Default::default()
    };

    config.wallet_a.endpoint = Some(producer.node.bound_address.to_string());
    config.wallet_b.endpoint = Some(validator.node.bound_address.to_string());

    // save config file
    let config = save_config_file(config);

    // execute suite
    execute_suite(config.path).await
}

async fn execute_suite(config_path: String) {
    let _ = tokio::task::spawn_blocking(|| {
        fuel_core_e2e_client::main_body(
            fuel_core_e2e_client::load_config(config_path),
            Default::default(),
        )
    })
    .await;
}

async fn setup_local_node() -> FuelService {
    FuelService::new_node(Config::local_node()).await.unwrap()
}

fn generate_config_file(endpoint: String) -> TestConfig {
    // setup config for test env
    let config = SuiteConfig {
        endpoint,
        ..Default::default()
    };
    save_config_file(config)
}

fn save_config_file(config: SuiteConfig) -> TestConfig {
    // generate a tmp dir
    let tmp_dir = TempDir::new().unwrap();
    // write config to file
    let config_path = tmp_dir.path().join("config.toml");
    fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();

    TestConfig {
        path: config_path.to_str().unwrap().to_string(),
        _dir: tmp_dir,
    }
}

struct TestConfig {
    path: String,
    // keep the temp dir alive to defer the deletion of the temp dir until the end of the test
    _dir: TempDir,
}

fuel_core_trace::enable_tracing!();
