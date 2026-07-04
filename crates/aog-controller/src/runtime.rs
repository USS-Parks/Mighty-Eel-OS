//! R1 — the level-triggered controller runtime. A [`Controller`] wires one
//! [`Reconciler`] to a store informer (K4): each sync it (1) refreshes the
//! informer cache, (2) enqueues every key whose revision changed since last
//! observed — duplicates coalesce in the [`WorkQueue`] — and (3), only when
//! its [`LeaderGate`] says this replica leads, drains due retries and runs
//! the reconciler over queued keys, with per-key exponential backoff on
//! failure.
//!
//! Level-triggered means a reconciler is handed only a *key*: it must read
//! current authoritative state and converge toward it, never interpret the
//! event that woke it. Dropping or duplicating events therefore cannot
//! change the end state (the R1 gate, proven in `tests/replay.rs`): a drop
//! is recovered by the informer's lag re-list, a duplicate is one extra
//! idempotent reconcile of the same current state.
//!
//! Leader gating ("singleton controllers"): a non-leader still observes —
//! cache warm, queue accumulating — but never acts. On takeover, the queue
//! it built is exactly the reconcile-everything a new leader owes the
//! estate. The single-node kernel runs [`AlwaysLeader`]; H1 wires
//! [`SharedGate`] to Raft leadership.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use aog_store::raft::watch::Informer;
use aog_store::{Revision, StoreError};

use crate::queue::{Backoff, WorkQueue};

/// What a reconciler asks the runtime to do next for a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Converged — nothing more to do until the key changes again.
    Done,
    /// Run again immediately (more work remains toward convergence).
    Requeue,
    /// Run again after a delay (e.g. waiting on an external system).
    RequeueAfter(Duration),
}

/// A reconcile attempt failed; the runtime schedules a backed-off retry.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ReconcileError(pub String);

/// A level-triggered reconciler. `reconcile` observes **current**
/// authoritative state for `key` and converges it toward desired; it must be
/// idempotent — the runtime may call it any number of times for one change.
pub trait Reconciler: Send + Sync {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send;
}

/// Whether this replica's controllers may act. Only the leading replica
/// reconciles (singleton controllers); every replica observes.
pub trait LeaderGate: Send + Sync {
    fn is_leader(&self) -> bool;
}

/// The single-node kernel gate: always leading.
#[derive(Debug, Clone, Copy, Default)]
pub struct AlwaysLeader;

impl LeaderGate for AlwaysLeader {
    fn is_leader(&self) -> bool {
        true
    }
}

/// A settable gate — flipped by whatever owns leadership (Raft at H1; tests
/// directly). Loss of leadership takes effect on the next sync: fail-closed
/// for action, not for observation.
#[derive(Debug, Default)]
pub struct SharedGate(AtomicBool);

impl SharedGate {
    #[must_use]
    pub fn new(leader: bool) -> Arc<Self> {
        Arc::new(Self(AtomicBool::new(leader)))
    }

    pub fn set(&self, leader: bool) {
        self.0.store(leader, Ordering::SeqCst);
    }
}

impl LeaderGate for SharedGate {
    fn is_leader(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// What one [`Controller::sync`] pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Whether this replica led during the pass (processing only happens then).
    pub leader: bool,
    /// Keys enqueued because their observed revision changed.
    pub enqueued: usize,
    /// Delayed (retry / requeue-after) keys that came due.
    pub drained: usize,
    /// Reconciles attempted.
    pub processed: usize,
    /// Reconciles that failed (each scheduled a backed-off retry).
    pub failed: usize,
}

/// One controller: an informer-fed, leader-gated reconcile loop over a key
/// prefix. Step-driven — [`sync`](Controller::sync) is one deterministic
/// pass; [`run`](Controller::run) loops it on an interval.
pub struct Controller<R: Reconciler> {
    name: String,
    informer: Informer,
    reconciler: R,
    gate: Arc<dyn LeaderGate>,
    queue: WorkQueue,
    known: BTreeMap<String, Revision>,
    budget: usize,
    started: bool,
}

impl<R: Reconciler> Controller<R> {
    /// A controller named `name` reconciling the keys `informer` watches.
    pub fn new(
        name: impl Into<String>,
        informer: Informer,
        reconciler: R,
        gate: Arc<dyn LeaderGate>,
    ) -> Self {
        Self {
            name: name.into(),
            informer,
            reconciler,
            gate,
            queue: WorkQueue::new(Backoff::default()),
            known: BTreeMap::new(),
            budget: 64,
            started: false,
        }
    }

    /// Replace the retry backoff policy.
    #[must_use]
    pub fn with_backoff(mut self, backoff: Backoff) -> Self {
        self.queue = WorkQueue::new(backoff);
        self
    }

    /// Cap reconciles per sync pass (min 1) — the global rate limit.
    #[must_use]
    pub fn with_budget(mut self, budget: usize) -> Self {
        self.budget = budget.max(1);
        self
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Keys currently queued for reconciliation.
    #[must_use]
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    /// Keys scheduled for a delayed re-add (retries / requeue-after).
    #[must_use]
    pub fn delayed_len(&self) -> usize {
        self.queue.delayed_len()
    }

    /// Force a key onto the queue (a manual kick; dedup applies).
    pub fn enqueue(&mut self, key: &str) {
        self.queue.add(key);
    }

    /// One reconcile pass. Observe (poll the informer — first pass re-lists —
    /// and enqueue changed keys); then, only if leading, drain due retries and
    /// reconcile up to `budget` keys.
    ///
    /// # Errors
    /// [`StoreError`] if the informer cannot read the store.
    pub async fn sync(&mut self, now: Instant) -> Result<SyncStats, StoreError> {
        let mut stats = SyncStats {
            leader: self.gate.is_leader(),
            ..SyncStats::default()
        };

        // Observe. poll() internally re-lists on a lagged (dropped) watch, and
        // the first pass re-lists to pick up pre-existing state — either way
        // the diff below sees the authoritative present, not an event replay.
        if self.started {
            self.informer.poll().await?;
        } else {
            self.informer.resync().await?;
            self.started = true;
        }
        stats.enqueued = self.observe();

        // Act — leaders only.
        if !stats.leader {
            return Ok(stats);
        }
        stats.drained = self.queue.drain_ready(now);
        while stats.processed < self.budget {
            let Some(key) = self.queue.take() else {
                break;
            };
            stats.processed += 1;
            match self.reconciler.reconcile(&key).await {
                Ok(Action::Done) => self.queue.forget(&key),
                Ok(Action::Requeue) => {
                    self.queue.forget(&key);
                    self.queue.add(&key); // in-processing → dirty → re-queued by done()
                }
                Ok(Action::RequeueAfter(delay)) => {
                    self.queue.forget(&key);
                    self.queue.requeue_after(&key, delay, now);
                }
                Err(_) => {
                    stats.failed += 1;
                    self.queue.retry(&key, now);
                }
            }
            self.queue.done(&key);
        }
        Ok(stats)
    }

    /// Diff the informer cache against the last-observed revisions; enqueue
    /// every created, changed, or deleted key. Level-triggered: the queue
    /// carries *which* keys to look at, never what happened to them.
    fn observe(&mut self) -> usize {
        let snapshot = self.informer.snapshot();
        let mut changed: Vec<String> = snapshot
            .iter()
            .filter(|(key, versioned)| self.known.get(*key) != Some(&versioned.mod_revision))
            .map(|(key, _)| key.clone())
            .collect();
        changed.extend(
            self.known
                .keys()
                .filter(|key| !snapshot.contains_key(*key))
                .cloned(),
        );
        self.known = snapshot
            .iter()
            .map(|(key, versioned)| (key.clone(), versioned.mod_revision))
            .collect();
        for key in &changed {
            self.queue.add(key);
        }
        changed.len()
    }

    /// Loop [`sync`](Controller::sync) every `interval` until `shutdown`
    /// flips true (or its sender drops).
    ///
    /// # Errors
    /// The first [`StoreError`] a sync pass hits — the supervisor restarts a
    /// controller; it does not limp with a broken read path (fail-closed).
    pub async fn run(
        &mut self,
        interval: Duration,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), StoreError> {
        loop {
            self.sync(Instant::now()).await?;
            tokio::select! {
                () = tokio::time::sleep(interval) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(());
                    }
                }
            }
        }
    }
}
