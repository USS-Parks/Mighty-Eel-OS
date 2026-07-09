//! redb-backed Raft log storage. Durable log entries keyed by index, plus
//! the persisted `vote` / `committed` / `last_purged` — so a restarted node
//! recovers its committed state. Entries serialize as JSON.

// openraft's `StorageError` is large by design and the storage traits must
// return it unboxed; boxing our internal helpers would just fight the API.
#![allow(clippy::result_large_err)]

use std::fmt;
use std::ops::RangeBounds;
use std::sync::Arc;

use openraft::storage::{LogFlushed, LogState, RaftLogStorage};
use openraft::{
    AnyError, Entry, ErrorSubject, ErrorVerb, LogId, OptionalSend, RaftLogReader, StorageError,
    StorageIOError, Vote,
};
use redb::{Database, ReadableTable, TableDefinition};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::raft::types::{NodeId, TypeConfig};

const LOGS: TableDefinition<u64, &[u8]> = TableDefinition::new("raft_logs");
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("raft_meta");

type StorageResult<T> = Result<T, StorageError<NodeId>>;

/// A durable Raft log over a shared redb database.
#[derive(Clone)]
pub struct RedbLogStore {
    db: Arc<Database>,
}

impl fmt::Debug for RedbLogStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedbLogStore")
    }
}

fn io<E: std::error::Error + 'static>(
    subject: ErrorSubject<NodeId>,
    verb: ErrorVerb,
    err: E,
) -> StorageError<NodeId> {
    StorageError::IO {
        source: StorageIOError::new(subject, verb, AnyError::new(&err)),
    }
}

impl RedbLogStore {
    /// Open a log store over `db`, ensuring the tables exist.
    ///
    /// # Errors
    /// [`StorageError`] on a redb failure.
    pub fn open(db: Arc<Database>) -> StorageResult<Self> {
        let wtx = db
            .begin_write()
            .map_err(|e| io(ErrorSubject::Store, ErrorVerb::Write, e))?;
        {
            wtx.open_table(LOGS)
                .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Write, e))?;
            wtx.open_table(META)
                .map_err(|e| io(ErrorSubject::Store, ErrorVerb::Write, e))?;
        }
        wtx.commit()
            .map_err(|e| io(ErrorSubject::Store, ErrorVerb::Write, e))?;
        Ok(Self { db })
    }

    fn read_meta<T: DeserializeOwned>(&self, key: &str) -> StorageResult<Option<T>> {
        let rtx = self
            .db
            .begin_read()
            .map_err(|e| io(ErrorSubject::Store, ErrorVerb::Read, e))?;
        let table = rtx
            .open_table(META)
            .map_err(|e| io(ErrorSubject::Store, ErrorVerb::Read, e))?;
        match table
            .get(key)
            .map_err(|e| io(ErrorSubject::Store, ErrorVerb::Read, e))?
        {
            Some(guard) => {
                let value = serde_json::from_slice(guard.value())
                    .map_err(|e| io(ErrorSubject::Store, ErrorVerb::Read, e))?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    fn write_meta<T: Serialize>(&self, key: &str, value: &T) -> StorageResult<()> {
        let bytes =
            serde_json::to_vec(value).map_err(|e| io(ErrorSubject::Store, ErrorVerb::Write, e))?;
        let wtx = self
            .db
            .begin_write()
            .map_err(|e| io(ErrorSubject::Store, ErrorVerb::Write, e))?;
        {
            let mut table = wtx
                .open_table(META)
                .map_err(|e| io(ErrorSubject::Store, ErrorVerb::Write, e))?;
            table
                .insert(key, bytes.as_slice())
                .map_err(|e| io(ErrorSubject::Store, ErrorVerb::Write, e))?;
        }
        wtx.commit()
            .map_err(|e| io(ErrorSubject::Store, ErrorVerb::Write, e))?;
        Ok(())
    }

    fn delete_range(&self, from_incl: Option<u64>, to_incl: Option<u64>) -> StorageResult<()> {
        let wtx = self
            .db
            .begin_write()
            .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Delete, e))?;
        {
            let mut table = wtx
                .open_table(LOGS)
                .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Delete, e))?;
            let mut keys = Vec::new();
            {
                let iter = table
                    .iter()
                    .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Delete, e))?;
                for item in iter {
                    let (key, _) =
                        item.map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Delete, e))?;
                    let index = key.value();
                    let above = from_incl.is_none_or(|lo| index >= lo);
                    let below = to_incl.is_none_or(|hi| index <= hi);
                    if above && below {
                        keys.push(index);
                    }
                }
            }
            for key in keys {
                table
                    .remove(key)
                    .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Delete, e))?;
            }
        }
        wtx.commit()
            .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Delete, e))?;
        Ok(())
    }
}

impl RaftLogReader<TypeConfig> for RedbLogStore {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + fmt::Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> StorageResult<Vec<Entry<TypeConfig>>> {
        let rtx = self
            .db
            .begin_read()
            .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Read, e))?;
        let table = rtx
            .open_table(LOGS)
            .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Read, e))?;
        let mut out = Vec::new();
        let iter = table
            .range(range)
            .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Read, e))?;
        for item in iter {
            let (_, value) = item.map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Read, e))?;
            let entry = serde_json::from_slice(value.value())
                .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Read, e))?;
            out.push(entry);
        }
        Ok(out)
    }
}

impl RaftLogStorage<TypeConfig> for RedbLogStore {
    type LogReader = Self;

    async fn get_log_state(&mut self) -> StorageResult<LogState<TypeConfig>> {
        let last_purged: Option<LogId<NodeId>> = self.read_meta("last_purged")?;
        let rtx = self
            .db
            .begin_read()
            .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Read, e))?;
        let table = rtx
            .open_table(LOGS)
            .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Read, e))?;
        let last = table
            .last()
            .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Read, e))?;
        let last_log_id = match last {
            Some((_, value)) => {
                let entry: Entry<TypeConfig> = serde_json::from_slice(value.value())
                    .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Read, e))?;
                Some(entry.log_id)
            }
            None => last_purged,
        };
        Ok(LogState {
            last_purged_log_id: last_purged,
            last_log_id,
        })
    }

    async fn save_committed(&mut self, committed: Option<LogId<NodeId>>) -> StorageResult<()> {
        self.write_meta("committed", &committed)
    }

    async fn read_committed(&mut self) -> StorageResult<Option<LogId<NodeId>>> {
        self.read_meta("committed")
    }

    async fn save_vote(&mut self, vote: &Vote<NodeId>) -> StorageResult<()> {
        self.write_meta("vote", vote)
    }

    async fn read_vote(&mut self) -> StorageResult<Option<Vote<NodeId>>> {
        self.read_meta("vote")
    }

    async fn append<I>(&mut self, entries: I, callback: LogFlushed<TypeConfig>) -> StorageResult<()>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        {
            let wtx = self
                .db
                .begin_write()
                .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Write, e))?;
            {
                let mut table = wtx
                    .open_table(LOGS)
                    .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Write, e))?;
                for entry in entries {
                    let bytes = serde_json::to_vec(&entry)
                        .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Write, e))?;
                    table
                        .insert(entry.log_id.index, bytes.as_slice())
                        .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Write, e))?;
                }
            }
            wtx.commit()
                .map_err(|e| io(ErrorSubject::Logs, ErrorVerb::Write, e))?;
        }
        callback.log_io_completed(Ok(()));
        Ok(())
    }

    async fn truncate(&mut self, log_id: LogId<NodeId>) -> StorageResult<()> {
        self.delete_range(Some(log_id.index), None)
    }

    async fn purge(&mut self, log_id: LogId<NodeId>) -> StorageResult<()> {
        self.write_meta("last_purged", &Some(log_id))?;
        self.delete_range(None, Some(log_id.index))
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        self.clone()
    }
}
