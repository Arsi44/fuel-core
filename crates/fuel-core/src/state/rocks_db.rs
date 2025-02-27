use crate::{
    database::{
        convert_to_rocksdb_direction,
        Column,
        Error as DatabaseError,
        Result as DatabaseResult,
    },
    state::{
        BatchOperations,
        IterDirection,
        KVItem,
        KeyValueStore,
        TransactableStorage,
        WriteOperation,
    },
};
#[cfg(feature = "metrics")]
use fuel_core_metrics::core_metrics::DATABASE_METRICS;
use fuel_core_storage::iter::{
    BoxedIter,
    IntoBoxedIter,
};
use rocksdb::{
    BoundColumnFamily,
    ColumnFamilyDescriptor,
    DBCompressionType,
    DBWithThreadMode,
    IteratorMode,
    MultiThreaded,
    Options,
    ReadOptions,
    SliceTransform,
    WriteBatch,
};
use std::{
    iter,
    path::Path,
    sync::Arc,
};

type DB = DBWithThreadMode<MultiThreaded>;
#[derive(Debug)]
pub struct RocksDb {
    db: DBWithThreadMode<MultiThreaded>,
}

impl RocksDb {
    pub fn default_open<P: AsRef<Path>>(path: P) -> DatabaseResult<RocksDb> {
        Self::open(path, enum_iterator::all::<Column>().collect::<Vec<_>>())
    }

    pub fn open<P: AsRef<Path>>(
        path: P,
        columns: Vec<Column>,
    ) -> DatabaseResult<RocksDb> {
        let cf_descriptors: Vec<_> = columns
            .clone()
            .into_iter()
            .map(|i| ColumnFamilyDescriptor::new(RocksDb::col_name(i), Self::cf_opts(i)))
            .collect();

        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_compression_type(DBCompressionType::Lz4);
        let db = match DB::open_cf_descriptors(&opts, &path, cf_descriptors) {
            Err(_) => {
                // setup cfs
                match DB::open_cf(&opts, &path, &[] as &[&str]) {
                    Ok(db) => {
                        for i in columns {
                            let opts = Self::cf_opts(i);
                            db.create_cf(RocksDb::col_name(i), &opts)
                                .map_err(|e| DatabaseError::Other(e.into()))?;
                        }
                        Ok(db)
                    }
                    err => err,
                }
            }
            ok => ok,
        }
        .map_err(|e| DatabaseError::Other(e.into()))?;
        let rocks_db = RocksDb { db };
        Ok(rocks_db)
    }

    fn cf(&self, column: Column) -> Arc<BoundColumnFamily> {
        self.db
            .cf_handle(&RocksDb::col_name(column))
            .expect("invalid column state")
    }

    fn col_name(column: Column) -> String {
        format!("column-{}", column as u32)
    }

    fn cf_opts(column: Column) -> Options {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_compression_type(DBCompressionType::Lz4);

        // All double-keys should be configured here
        match column {
            Column::OwnedCoins
            | Column::TransactionsByOwnerBlockIdx
            | Column::OwnedMessageIds
            | Column::ContractsAssets
            | Column::ContractsState => {
                // prefix is address length
                opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(32))
            }
            _ => {}
        };

        opts
    }

    fn _iter_all(
        &self,
        column: Column,
        opts: ReadOptions,
        iter_mode: IteratorMode,
    ) -> impl Iterator<Item = KVItem> + '_ {
        self.db
            .iterator_cf_opt(&self.cf(column), opts, iter_mode)
            .map(|item| {
                item.map(|(key, value)| {
                    let value_as_vec = Vec::from(value);
                    let key_as_vec = Vec::from(key);
                    #[cfg(feature = "metrics")]
                    {
                        DATABASE_METRICS.read_meter.inc();
                        DATABASE_METRICS
                            .bytes_read
                            .observe((key_as_vec.len() + value_as_vec.len()) as f64);
                    }
                    (key_as_vec, value_as_vec)
                })
                .map_err(|e| DatabaseError::Other(e.into()))
            })
    }
}

impl KeyValueStore for RocksDb {
    fn get(&self, key: &[u8], column: Column) -> DatabaseResult<Option<Vec<u8>>> {
        #[cfg(feature = "metrics")]
        DATABASE_METRICS.read_meter.inc();
        let value = self
            .db
            .get_cf(&self.cf(column), key)
            .map_err(|e| DatabaseError::Other(e.into()));
        #[cfg(feature = "metrics")]
        {
            if let Ok(Some(value)) = &value {
                DATABASE_METRICS.bytes_read.observe(value.len() as f64);
            }
        }
        value
    }

    fn put(
        &self,
        key: &[u8],
        column: Column,
        value: Vec<u8>,
    ) -> DatabaseResult<Option<Vec<u8>>> {
        #[cfg(feature = "metrics")]
        {
            DATABASE_METRICS.write_meter.inc();
            DATABASE_METRICS.bytes_written.observe(value.len() as f64);
        }
        // FIXME: This is a race condition. We should use a transaction.
        let prev = self.get(key, column)?;
        // FIXME: This is a race condition. We should use a transaction.
        self.db
            .put_cf(&self.cf(column), key, value)
            .map_err(|e| DatabaseError::Other(e.into()))
            .map(|_| prev)
    }

    fn delete(&self, key: &[u8], column: Column) -> DatabaseResult<Option<Vec<u8>>> {
        // FIXME: This is a race condition. We should use a transaction.
        let prev = self.get(key, column)?;
        // FIXME: This is a race condition. We should use a transaction.
        self.db
            .delete_cf(&self.cf(column), key)
            .map_err(|e| DatabaseError::Other(e.into()))
            .map(|_| prev)
    }

    fn exists(&self, key: &[u8], column: Column) -> DatabaseResult<bool> {
        // use pinnable mem ref to avoid memcpy of values associated with the key
        // since we're just checking for the existence of the key
        self.db
            .get_pinned_cf(&self.cf(column), key)
            .map_err(|e| DatabaseError::Other(e.into()))
            .map(|v| v.is_some())
    }

    fn iter_all(
        &self,
        column: Column,
        prefix: Option<&[u8]>,
        start: Option<&[u8]>,
        direction: IterDirection,
    ) -> BoxedIter<KVItem> {
        match (prefix, start) {
            (None, None) => {
                let iter_mode =
                    // if no start or prefix just start iterating over entire keyspace
                    match direction {
                        IterDirection::Forward => IteratorMode::Start,
                        // end always iterates in reverse
                        IterDirection::Reverse => IteratorMode::End,
                    };
                self._iter_all(column, ReadOptions::default(), iter_mode)
                    .into_boxed()
            }
            (Some(prefix), None) => {
                // start iterating in a certain direction within the keyspace
                let iter_mode =
                    IteratorMode::From(prefix, convert_to_rocksdb_direction(direction));
                let mut opts = ReadOptions::default();
                opts.set_prefix_same_as_start(true);

                self._iter_all(column, opts, iter_mode).into_boxed()
            }
            (None, Some(start)) => {
                // start iterating in a certain direction from the start key
                let iter_mode =
                    IteratorMode::From(start, convert_to_rocksdb_direction(direction));
                self._iter_all(column, ReadOptions::default(), iter_mode)
                    .into_boxed()
            }
            (Some(prefix), Some(start)) => {
                // TODO: Maybe we want to allow the `start` to be without a `prefix` in the future.
                // If the `start` doesn't have the same `prefix`, return nothing.
                if !start.starts_with(prefix) {
                    return iter::empty().into_boxed()
                }

                // start iterating in a certain direction from the start key
                // and end iterating when we've gone outside the prefix
                let prefix = prefix.to_vec();
                let iter_mode =
                    IteratorMode::From(start, convert_to_rocksdb_direction(direction));
                self._iter_all(column, ReadOptions::default(), iter_mode)
                    .take_while(move |item| {
                        if let Ok((key, _)) = item {
                            key.starts_with(prefix.as_slice())
                        } else {
                            true
                        }
                    })
                    .into_boxed()
            }
        }
    }

    fn size_of_value(&self, key: &[u8], column: Column) -> DatabaseResult<Option<usize>> {
        #[cfg(feature = "metrics")]
        DATABASE_METRICS.read_meter.inc();

        Ok(self
            .db
            .get_pinned_cf(&self.cf(column), key)
            .map_err(|e| DatabaseError::Other(e.into()))?
            .map(|value| value.len()))
    }

    fn read(
        &self,
        key: &[u8],
        column: Column,
        mut buf: &mut [u8],
    ) -> DatabaseResult<Option<usize>> {
        #[cfg(feature = "metrics")]
        DATABASE_METRICS.read_meter.inc();

        let r = self
            .db
            .get_pinned_cf(&self.cf(column), key)
            .map_err(|e| DatabaseError::Other(e.into()))?
            .map(|value| {
                let read = value.len();
                std::io::Write::write_all(&mut buf, value.as_ref())
                    .map_err(|e| DatabaseError::Other(anyhow::anyhow!(e)))?;
                DatabaseResult::Ok(read)
            })
            .transpose()?;

        #[cfg(feature = "metrics")]
        {
            if let Some(r) = &r {
                DATABASE_METRICS.bytes_read.observe(*r as f64);
            }
        }
        Ok(r)
    }

    fn write(&self, key: &[u8], column: Column, buf: Vec<u8>) -> DatabaseResult<usize> {
        #[cfg(feature = "metrics")]
        {
            DATABASE_METRICS.write_meter.inc();
            DATABASE_METRICS.bytes_written.observe(buf.len() as f64);
        }

        let r = buf.len();
        self.db
            .put_cf(&self.cf(column), key, buf)
            .map_err(|e| DatabaseError::Other(e.into()))?;

        Ok(r)
    }

    fn read_alloc(&self, key: &[u8], column: Column) -> DatabaseResult<Option<Vec<u8>>> {
        #[cfg(feature = "metrics")]
        DATABASE_METRICS.read_meter.inc();

        let r = self
            .db
            .get_pinned_cf(&self.cf(column), key)
            .map_err(|e| DatabaseError::Other(e.into()))?
            .map(|value| value.to_vec());

        #[cfg(feature = "metrics")]
        {
            if let Some(r) = &r {
                DATABASE_METRICS.bytes_read.observe(r.len() as f64);
            }
        }
        Ok(r)
    }

    fn replace(
        &self,
        key: &[u8],
        column: Column,
        buf: Vec<u8>,
    ) -> DatabaseResult<(usize, Option<Vec<u8>>)> {
        // FIXME: This is a race condition. We should use a transaction.
        let existing = self.read_alloc(key, column)?;
        // FIXME: This is a race condition. We should use a transaction.
        let r = self.write(key, column, buf)?;

        Ok((r, existing))
    }

    fn take(&self, key: &[u8], column: Column) -> DatabaseResult<Option<Vec<u8>>> {
        // FIXME: This is a race condition. We should use a transaction.
        let prev = self.read_alloc(key, column)?;
        // FIXME: This is a race condition. We should use a transaction.
        self.db
            .delete_cf(&self.cf(column), key)
            .map_err(|e| DatabaseError::Other(e.into()))
            .map(|_| prev)
    }
}

impl BatchOperations for RocksDb {
    fn batch_write(
        &self,
        entries: &mut dyn Iterator<Item = WriteOperation>,
    ) -> DatabaseResult<()> {
        let mut batch = WriteBatch::default();

        for entry in entries {
            match entry {
                WriteOperation::Insert(key, column, value) => {
                    batch.put_cf(&self.cf(column), key, value);
                }
                WriteOperation::Remove(key, column) => {
                    batch.delete_cf(&self.cf(column), key);
                }
            }
        }
        #[cfg(feature = "metrics")]
        {
            DATABASE_METRICS.write_meter.inc();
            DATABASE_METRICS
                .bytes_written
                .observe(batch.size_in_bytes() as f64);
        }
        self.db
            .write(batch)
            .map_err(|e| DatabaseError::Other(e.into()))
    }
}

impl TransactableStorage for RocksDb {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_db() -> (RocksDb, TempDir) {
        let tmp_dir = TempDir::new().unwrap();
        (RocksDb::default_open(tmp_dir.path()).unwrap(), tmp_dir)
    }

    #[test]
    fn can_put_and_read() {
        let key = vec![0xA, 0xB, 0xC];

        let (db, _tmp) = create_db();
        db.put(&key, Column::Metadata, vec![1, 2, 3]).unwrap();

        assert_eq!(
            db.get(&key, Column::Metadata).unwrap().unwrap(),
            vec![1, 2, 3]
        )
    }

    #[test]
    fn put_returns_previous_value() {
        let key = vec![0xA, 0xB, 0xC];

        let (db, _tmp) = create_db();
        db.put(&key, Column::Metadata, vec![1, 2, 3]).unwrap();
        let prev = db.put(&key, Column::Metadata, vec![2, 4, 6]).unwrap();

        assert_eq!(prev, Some(vec![1, 2, 3]));
    }

    #[test]
    fn delete_and_get() {
        let key = vec![0xA, 0xB, 0xC];

        let (db, _tmp) = create_db();
        db.put(&key, Column::Metadata, vec![1, 2, 3]).unwrap();
        assert_eq!(
            db.get(&key, Column::Metadata).unwrap().unwrap(),
            vec![1, 2, 3]
        );

        db.delete(&key, Column::Metadata).unwrap();
        assert_eq!(db.get(&key, Column::Metadata).unwrap(), None);
    }

    #[test]
    fn key_exists() {
        let key = vec![0xA, 0xB, 0xC];

        let (db, _tmp) = create_db();
        db.put(&key, Column::Metadata, vec![1, 2, 3]).unwrap();
        assert!(db.exists(&key, Column::Metadata).unwrap());
    }

    #[test]
    fn batch_write_inserts() {
        let key = vec![0xA, 0xB, 0xC];
        let value = vec![1, 2, 3];

        let (db, _tmp) = create_db();
        let ops = vec![WriteOperation::Insert(
            key.clone(),
            Column::Metadata,
            value.clone(),
        )];

        db.batch_write(&mut ops.into_iter()).unwrap();
        assert_eq!(db.get(&key, Column::Metadata).unwrap().unwrap(), value)
    }

    #[test]
    fn batch_write_removes() {
        let key = vec![0xA, 0xB, 0xC];
        let value = vec![1, 2, 3];

        let (db, _tmp) = create_db();
        db.put(&key, Column::Metadata, value).unwrap();

        let ops = vec![WriteOperation::Remove(key.clone(), Column::Metadata)];
        db.batch_write(&mut ops.into_iter()).unwrap();

        assert_eq!(db.get(&key, Column::Metadata).unwrap(), None);
    }

    #[test]
    fn can_use_unit_value() {
        let key = vec![0x00];

        let (db, _tmp) = create_db();
        db.put(&key, Column::Metadata, vec![]).unwrap();

        assert_eq!(
            db.get(&key, Column::Metadata).unwrap().unwrap(),
            Vec::<u8>::with_capacity(0)
        );

        assert!(db.exists(&key, Column::Metadata).unwrap());

        assert_eq!(
            db.iter_all(Column::Metadata, None, None, IterDirection::Forward)
                .collect::<Result<Vec<_>, _>>()
                .unwrap()[0],
            (key.clone(), Vec::<u8>::with_capacity(0))
        );

        assert_eq!(
            db.delete(&key, Column::Metadata).unwrap().unwrap(),
            Vec::<u8>::with_capacity(0)
        );

        assert!(!db.exists(&key, Column::Metadata).unwrap());
    }

    #[test]
    fn can_use_unit_key() {
        let key: Vec<u8> = Vec::with_capacity(0);

        let (db, _tmp) = create_db();
        db.put(&key, Column::Metadata, vec![1, 2, 3]).unwrap();

        assert_eq!(
            db.get(&key, Column::Metadata).unwrap().unwrap(),
            vec![1, 2, 3]
        );

        assert!(db.exists(&key, Column::Metadata).unwrap());

        assert_eq!(
            db.iter_all(Column::Metadata, None, None, IterDirection::Forward)
                .collect::<Result<Vec<_>, _>>()
                .unwrap()[0],
            (key.clone(), vec![1, 2, 3])
        );

        assert_eq!(
            db.delete(&key, Column::Metadata).unwrap().unwrap(),
            vec![1, 2, 3]
        );

        assert!(!db.exists(&key, Column::Metadata).unwrap());
    }

    #[test]
    fn can_use_unit_key_and_value() {
        let key: Vec<u8> = Vec::with_capacity(0);

        let (db, _tmp) = create_db();
        db.put(&key, Column::Metadata, vec![]).unwrap();

        assert_eq!(
            db.get(&key, Column::Metadata).unwrap().unwrap(),
            Vec::<u8>::with_capacity(0)
        );

        assert!(db.exists(&key, Column::Metadata).unwrap());

        assert_eq!(
            db.iter_all(Column::Metadata, None, None, IterDirection::Forward)
                .collect::<Result<Vec<_>, _>>()
                .unwrap()[0],
            (key.clone(), Vec::<u8>::with_capacity(0))
        );

        assert_eq!(
            db.delete(&key, Column::Metadata).unwrap().unwrap(),
            Vec::<u8>::with_capacity(0)
        );

        assert!(!db.exists(&key, Column::Metadata).unwrap());
    }
}
