//! redb-backed [`Backend`] — the durable engine chosen (A4). All revision
//! and CAS logic stays in `Store`; this module only persists bytes in a single
//! ACID table, `Versioned` serialized as JSON.

use std::fmt;
use std::path::Path;

use redb::{Database, ReadableTable, TableDefinition};

use crate::{Backend, Revision, StoreError, Versioned};

const TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("estate");

/// A durable, single-file store backend.
pub struct RedbBackend {
    db: Database,
}

impl fmt::Debug for RedbBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedbBackend")
    }
}

impl RedbBackend {
    /// Open (or create) a redb store at `path`, ensuring the table exists.
    ///
    /// # Errors
    /// [`StoreError::Backend`] on any redb open/commit failure.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let db = Database::create(path).map_err(be)?;
        let wtx = db.begin_write().map_err(be)?;
        {
            let _table = wtx.open_table(TABLE).map_err(be)?;
        }
        wtx.commit().map_err(be)?;
        Ok(Self { db })
    }
}

fn be<E: fmt::Display>(err: E) -> StoreError {
    StoreError::Backend(err.to_string())
}

fn decode(bytes: &[u8]) -> Result<Versioned, StoreError> {
    serde_json::from_slice(bytes).map_err(be)
}

fn encode(value: &Versioned) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec(value).map_err(be)
}

impl Backend for RedbBackend {
    fn get(&self, key: &str) -> Result<Option<Versioned>, StoreError> {
        let rtx = self.db.begin_read().map_err(be)?;
        let table = rtx.open_table(TABLE).map_err(be)?;
        match table.get(key).map_err(be)? {
            Some(guard) => Ok(Some(decode(guard.value())?)),
            None => Ok(None),
        }
    }

    fn insert(&mut self, key: &str, value: &Versioned) -> Result<(), StoreError> {
        let bytes = encode(value)?;
        let wtx = self.db.begin_write().map_err(be)?;
        {
            let mut table = wtx.open_table(TABLE).map_err(be)?;
            table.insert(key, bytes.as_slice()).map_err(be)?;
        }
        wtx.commit().map_err(be)?;
        Ok(())
    }

    fn remove(&mut self, key: &str) -> Result<(), StoreError> {
        let wtx = self.db.begin_write().map_err(be)?;
        {
            let mut table = wtx.open_table(TABLE).map_err(be)?;
            table.remove(key).map_err(be)?;
        }
        wtx.commit().map_err(be)?;
        Ok(())
    }

    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Versioned)>, StoreError> {
        let rtx = self.db.begin_read().map_err(be)?;
        let table = rtx.open_table(TABLE).map_err(be)?;
        let mut out = Vec::new();
        for item in table.iter().map_err(be)? {
            let (key, value) = item.map_err(be)?;
            let key = key.value();
            if key.starts_with(prefix) {
                out.push((key.to_owned(), decode(value.value())?));
            }
        }
        Ok(out)
    }

    fn max_revision(&self) -> Result<Revision, StoreError> {
        let rtx = self.db.begin_read().map_err(be)?;
        let table = rtx.open_table(TABLE).map_err(be)?;
        let mut max = 0;
        for item in table.iter().map_err(be)? {
            let (_key, value) = item.map_err(be)?;
            max = max.max(decode(value.value())?.mod_revision);
        }
        Ok(max)
    }
}
