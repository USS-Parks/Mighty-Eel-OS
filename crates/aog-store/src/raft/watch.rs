//! Watch + informer. The state machine fans out a change-event stream as
//! it applies mutations; an [`Informer`] keeps a prefix-scoped local cache
//! current from that stream, and **re-lists authoritative state on lag or a
//! fresh start** so it can never miss a final state. This is the
//! read path the Phase-R controllers build on.

use std::collections::BTreeMap;

use tokio::sync::broadcast;

use crate::raft::state_machine::RedbStateMachine;
use crate::{Revision, StoreError, Versioned};

/// Whether a watched key was written or deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Put,
    Delete,
}

/// One applied change to the desired state, published as it commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchEvent {
    pub revision: Revision,
    pub key: String,
    pub kind: EventKind,
}

/// A prefix-scoped local cache kept current from the change stream, with
/// re-list recovery (K8s-informer style). Correctness comes from [`resync`]:
/// buffer overflow (`Lagged`) or a fresh start both re-list authoritative
/// state, so no final state is ever missed.
///
/// [`resync`]: Informer::resync
pub struct Informer {
    sm: RedbStateMachine,
    prefix: String,
    cache: BTreeMap<String, Versioned>,
    rx: broadcast::Receiver<WatchEvent>,
    revision: Revision,
}

impl Informer {
    /// Create an informer for `prefix`, subscribed to the change stream. Call
    /// [`Informer::resync`] before reading to populate the cache.
    #[must_use]
    pub fn new(sm: RedbStateMachine, prefix: impl Into<String>) -> Self {
        let rx = sm.subscribe();
        Self {
            sm,
            prefix: prefix.into(),
            cache: BTreeMap::new(),
            rx,
            revision: 0,
        }
    }

    /// Re-list authoritative state from the store and reset the cache — the
    /// recovery path for a dropped or lagged watch. Re-subscribes first, so no
    /// event applied after the list is missed.
    ///
    /// # Errors
    /// Store backend failure.
    pub async fn resync(&mut self) -> Result<(), StoreError> {
        self.rx = self.sm.subscribe();
        let entries = self.sm.range(&self.prefix).await?;
        self.revision = self.sm.revision().await;
        self.cache = entries.into_iter().collect();
        Ok(())
    }

    /// Drain pending change events into the cache. On `Lagged` — the buffer
    /// overflowed, i.e. a dropped connection — re-list so no final state is
    /// missed.
    ///
    /// # Errors
    /// Store backend failure during a re-list.
    pub async fn poll(&mut self) -> Result<(), StoreError> {
        loop {
            match self.rx.try_recv() {
                Ok(event) => self.apply_event(event).await?,
                Err(
                    broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed,
                ) => {
                    break;
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => self.resync().await?,
            }
        }
        Ok(())
    }

    async fn apply_event(&mut self, event: WatchEvent) -> Result<(), StoreError> {
        if !event.key.starts_with(&self.prefix) {
            return Ok(());
        }
        self.revision = self.revision.max(event.revision);
        match event.kind {
            EventKind::Delete => {
                self.cache.remove(&event.key);
            }
            EventKind::Put => match self.sm.get(&event.key).await? {
                Some(value) => {
                    self.cache.insert(event.key, value);
                }
                None => {
                    self.cache.remove(&event.key);
                }
            },
        }
        Ok(())
    }

    /// The current cached view of the prefix.
    #[must_use]
    pub fn snapshot(&self) -> &BTreeMap<String, Versioned> {
        &self.cache
    }

    /// The highest revision observed.
    #[must_use]
    pub fn revision(&self) -> Revision {
        self.revision
    }
}
