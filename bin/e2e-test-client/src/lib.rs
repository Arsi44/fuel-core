use crate::{
    config::SuiteConfig,
    test_context::TestContext,
};
use libtest_mimic::{
    Arguments,
    Failed,
    Trial,
};
use std::{
    env,
    fs,
    future::Future,
    time::Duration,
};

pub const CONFIG_FILE_KEY: &str = "FUEL_CORE_E2E_CONFIG";
pub const SYNC_TIMEOUT: Duration = Duration::from_secs(10);

pub mod config;
pub mod test_context;
pub mod tests;

pub fn main_body(config: SuiteConfig, mut args: Arguments) {
    fn with_cloned(
        config: &SuiteConfig,
        f: impl FnOnce(SuiteConfig) -> anyhow::Result<(), Failed>,
    ) -> impl FnOnce() -> anyhow::Result<(), Failed> {
        let config = config.clone();
        move || f(config)
    }

    // If we run tests in parallel they may fail because try to use the same state like UTXOs.
    args.test_threads = Some(1);

    let tests = vec![
        Trial::test(
            "can transfer from alice to bob",
            with_cloned(&config, |config| {
                let ctx = TestContext::new(config);
                async_execute(tests::transfers::basic_transfer(&ctx))
            }),
        ),
        Trial::test(
            "can transfer from alice to bob and back",
            with_cloned(&config, |config| {
                let ctx = TestContext::new(config);
                async_execute(tests::transfers::transfer_back(&ctx))
            }),
        ),
        Trial::test(
            "can execute script and get receipts",
            with_cloned(&config, |config| {
                let ctx = TestContext::new(config);
                async_execute(tests::script::receipts(&ctx))
            }),
        ),
        Trial::test(
            "can dry run transfer script and get receipts",
            with_cloned(&config, |config| {
                let ctx = TestContext::new(config);
                async_execute(tests::script::dry_run(&ctx))?;
                Ok(())
            }),
        ),
        Trial::test(
            "can deploy a large contract",
            with_cloned(&config, |config| {
                let ctx = TestContext::new(config);
                async_execute(tests::contracts::deploy_large_contract(&ctx))
            }),
        ),
    ];

    libtest_mimic::run(&args, tests).exit();
}

pub fn load_config_env() -> SuiteConfig {
    // load from env var
    env::var_os(CONFIG_FILE_KEY)
        .map(|path| load_config(path.to_string_lossy().to_string()))
        .unwrap_or_default()
}

pub fn load_config(path: String) -> SuiteConfig {
    let file = fs::read(path).unwrap();
    toml::from_slice(&file).unwrap()
}

fn async_execute<F: Future<Output = anyhow::Result<(), Failed>>>(
    func: F,
) -> Result<(), Failed> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(func)
}
