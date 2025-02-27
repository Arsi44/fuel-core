use crate::{
    ports,
    ports::BlockProducerDatabase,
    Config,
};
use anyhow::{
    anyhow,
    Context,
};
use fuel_core_storage::transactional::StorageTransaction;
use fuel_core_types::{
    blockchain::{
        block::PartialFuelBlock,
        header::{
            ApplicationHeader,
            ConsensusHeader,
            PartialBlockHeader,
        },
        primitives::{
            BlockHeight,
            DaBlockHeight,
        },
    },
    fuel_asm::Word,
    fuel_tx::{
        Receipt,
        Transaction,
    },
    fuel_types::Bytes32,
    services::executor::{
        ExecutionBlock,
        UncommittedResult,
    },
    tai64::Tai64,
};
use std::sync::Arc;
use thiserror::Error;
use tokio::{
    sync::{
        Mutex,
        Semaphore,
    },
    task::spawn_blocking,
};
use tracing::debug;

#[cfg(test)]
mod tests;

#[derive(Error, Debug)]
pub enum Error {
    #[error(
        "0 is an invalid block height for production. It is reserved for genesis data."
    )]
    GenesisBlock,
    #[error("Previous block height {0} doesn't exist")]
    MissingBlock(BlockHeight),
    #[error("Best finalized da_height {best} is behind previous block da_height {previous_block}")]
    InvalidDaFinalizationState {
        best: DaBlockHeight,
        previous_block: DaBlockHeight,
    },
}

pub struct Producer<Database> {
    pub config: Config,
    pub db: Database,
    pub txpool: Box<dyn ports::TxPool>,
    pub executor: Arc<dyn ports::Executor<Database>>,
    pub relayer: Box<dyn ports::Relayer>,
    // use a tokio lock since we want callers to yield until the previous block
    // execution has completed (which may take a while).
    pub lock: Mutex<()>,
    pub dry_run_semaphore: Semaphore,
}

impl<Database> Producer<Database>
where
    Database: BlockProducerDatabase + 'static,
{
    /// Produces and execute block for the specified height
    pub async fn produce_and_execute_block(
        &self,
        height: BlockHeight,
        block_time: Tai64,
        max_gas: Word,
    ) -> anyhow::Result<UncommittedResult<StorageTransaction<Database>>> {
        //  - get previous block info (hash, root, etc)
        //  - select best da_height from relayer
        //  - get available txs from txpool
        //  - select best txs based on factors like:
        //      1. fees
        //      2. parallel throughput
        //  - Execute block with production mode to correctly malleate txs outputs and block headers

        // prevent simultaneous block production calls, the guard will drop at the end of this fn.
        let _production_guard = self.lock.lock().await;

        let best_transactions = self.txpool.get_includable_txs(height, max_gas);

        let header = self.new_header(height, block_time).await?;
        let block = PartialFuelBlock::new(
            header,
            best_transactions
                .into_iter()
                .map(|tx| tx.as_ref().into())
                .collect(),
        );

        // Store the context string incase we error.
        let context_string =
            format!("Failed to produce block {block:?} due to execution failure");
        let result = self
            .executor
            .execute_without_commit(ExecutionBlock::Production(block))
            .context(context_string)?;

        debug!("Produced block with result: {:?}", result.result());
        Ok(result)
    }

    // TODO: Support custom `block_time` for `dry_run`.
    /// Simulate a transaction without altering any state. Does not aquire the production lock
    /// since it is basically a "read only" operation and shouldn't get in the way of normal
    /// production.
    pub async fn dry_run(
        &self,
        transaction: Transaction,
        height: Option<BlockHeight>,
        utxo_validation: Option<bool>,
    ) -> anyhow::Result<Vec<Receipt>> {
        // setup the block with the provided tx and optional height
        // dry_run execute tx on the executor
        // return the receipts
        let _permit = self.dry_run_semaphore.acquire().await;

        let height = match height {
            None => self.db.current_block_height()?,
            Some(height) => height,
        } + 1u64.into();

        let is_script = transaction.is_script();
        let header = self.new_header(height, Tai64::now()).await?;
        let block =
            PartialFuelBlock::new(header, vec![transaction].into_iter().collect());

        let executor = self.executor.clone();
        // use the blocking threadpool for dry_run to avoid clogging up the main async runtime
        let res: Vec<_> = spawn_blocking(move || -> anyhow::Result<Vec<Receipt>> {
            Ok(executor
                .dry_run(ExecutionBlock::Production(block), utxo_validation)?
                .into_iter()
                .flatten()
                .collect())
        })
        .await??;
        if is_script && res.is_empty() {
            return Err(anyhow!("Expected at least one set of receipts"))
        }
        Ok(res)
    }
}

impl<Database> Producer<Database>
where
    Database: BlockProducerDatabase,
{
    /// Create the header for a new block at the provided height
    async fn new_header(
        &self,
        height: BlockHeight,
        block_time: Tai64,
    ) -> anyhow::Result<PartialBlockHeader> {
        let previous_block_info = self.previous_block_info(height)?;
        let new_da_height = self
            .select_new_da_height(previous_block_info.da_height)
            .await?;

        Ok(PartialBlockHeader {
            application: ApplicationHeader {
                da_height: new_da_height,
                generated: Default::default(),
            },
            consensus: ConsensusHeader {
                prev_root: previous_block_info.prev_root,
                height,
                time: block_time,
                generated: Default::default(),
            },
        })
    }

    async fn select_new_da_height(
        &self,
        previous_da_height: DaBlockHeight,
    ) -> anyhow::Result<DaBlockHeight> {
        let best_height = self.relayer.wait_for_at_least(&previous_da_height).await?;
        if best_height < previous_da_height {
            // If this happens, it could mean a block was erroneously imported
            // without waiting for our relayer's da_height to catch up to imported da_height.
            return Err(Error::InvalidDaFinalizationState {
                best: best_height,
                previous_block: previous_da_height,
            }
            .into())
        }
        Ok(best_height)
    }

    fn previous_block_info(
        &self,
        height: BlockHeight,
    ) -> anyhow::Result<PreviousBlockInfo> {
        // TODO: It is not guaranteed that the genesis height is `0` height. Update the code to
        //  use a genesis height from the database. If the `height` less than genesis height ->
        //  return a new error.
        // block 0 is reserved for genesis
        if height == 0u32.into() {
            Err(Error::GenesisBlock.into())
        } else {
            // get info from previous block height
            let prev_height = height - 1u32.into();
            let previous_block = self.db.get_block(&prev_height)?;
            let prev_root = self.db.block_header_merkle_root(&prev_height)?;

            Ok(PreviousBlockInfo {
                prev_root,
                da_height: previous_block.header().da_height,
            })
        }
    }
}

struct PreviousBlockInfo {
    prev_root: Bytes32,
    da_height: DaBlockHeight,
}
