#![deny(unused_crate_dependencies)]

use fuel_core_types::services::txpool::ArcPoolTx;
use std::{
    ops::Deref,
    time::Duration,
};

pub mod config;
mod containers;
pub mod ports;
pub mod service;
mod transaction_selector;
pub mod txpool;
pub mod types;

#[cfg(any(test, feature = "test-helpers"))]
pub mod mock_db;
#[cfg(any(test, feature = "test-helpers"))]
pub use mock_db::MockDb;

pub use config::Config;
pub use fuel_core_types::services::txpool::Error;
pub use service::{
    new_service,
    Service,
};
pub use txpool::TxPool;

#[cfg(any(test, feature = "test-helpers"))]
pub(crate) mod test_helpers;

#[cfg(test)]
fuel_core_trace::enable_tracing!();

/// Information of a transaction fetched from the txpool.
#[derive(Debug, Clone)]
pub struct TxInfo {
    tx: ArcPoolTx,
    submitted_time: Duration,
    creation_instant: tokio::time::Instant,
}

#[allow(missing_docs)]
impl TxInfo {
    pub fn new(tx: ArcPoolTx) -> Self {
        let since_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Now is bellow of the `UNIX_EPOCH`");

        Self {
            tx,
            submitted_time: since_epoch,
            creation_instant: tokio::time::Instant::now(),
        }
    }

    pub fn tx(&self) -> &ArcPoolTx {
        &self.tx
    }

    pub fn submitted_time(&self) -> Duration {
        self.submitted_time
    }

    pub fn created(&self) -> tokio::time::Instant {
        self.creation_instant
    }
}

impl Deref for TxInfo {
    type Target = ArcPoolTx;
    fn deref(&self) -> &Self::Target {
        &self.tx
    }
}
