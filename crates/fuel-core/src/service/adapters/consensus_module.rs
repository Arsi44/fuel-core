use crate::{
    database::Database,
    service::{
        adapters::{
            MaybeRelayerAdapter,
            VerifierAdapter,
        },
        Config,
    },
};
use fuel_core_consensus_module::block_verifier::{
    config::Config as VerifierConfig,
    Verifier,
};
use fuel_core_producer::ports::BlockProducerDatabase;
use fuel_core_storage::{
    tables::FuelBlocks,
    Result as StorageResult,
    StorageAsRef,
};
use fuel_core_types::{
    blockchain::{
        header::BlockHeader,
        primitives::BlockHeight,
    },
    fuel_tx::Bytes32,
};
use std::sync::Arc;

pub mod poa;

impl VerifierAdapter {
    pub fn new(
        config: &Config,
        database: Database,
        relayer: MaybeRelayerAdapter,
    ) -> Self {
        let config = VerifierConfig::new(
            config.chain_conf.clone(),
            config.manual_blocks_enabled,
            config.verifier.clone(),
        );
        Self {
            block_verifier: Arc::new(Verifier::new(config, database, relayer)),
        }
    }
}

impl fuel_core_poa::ports::Database for Database {
    fn block_header(&self, height: &BlockHeight) -> StorageResult<BlockHeader> {
        Ok(self.get_block(height)?.header().clone())
    }

    fn block_header_merkle_root(&self, height: &BlockHeight) -> StorageResult<Bytes32> {
        self.storage::<FuelBlocks>().root(height).map(Into::into)
    }
}
