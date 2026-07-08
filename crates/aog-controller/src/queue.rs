//! R1 — the controller workqueue: dedup, per-key exponential backoff, delayed
//! requeue. K8s-workqueue shaped, level-triggered: a queued key means
//! "reconcile this key at least once more", so adding a key that is already
//! queued coalesces into the one pending run (a duplicate event costs
//! nothing), and a key re-added *while it is being processed* is marked dirty
//! and re-queued when its run completes (no update lost between a
//! reconciler's read and its write).
//!
//! Time is always passed in by the caller (`now: Instant`) — the queue never
//! reads a clock — so retry and delay behavior is deterministic under test.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

/// Per-key exponential backoff: `base * 2^(n-1)` before the `n`-th
/// consecutive retry, capped at `max`. This is the R1 rate limit on a failing
/// key — a broken object retries ever more slowly instead of hot-looping the
/// controller, while one success ([`WorkQueue::forget`]) resets it.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub base: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            base: Duration::from_millis(200),
            max: Duration::from_secs(60),
        }
    }
}

impl Backoff {
    /// The delay before retry number `failures` (1-based). Zero means no delay.
    #[must_use]
    pub fn delay(&self, failures: u32) -> Duration {
        if failures == 0 {
            return Duration::ZERO;
        }
        let factor = 1u32.checked_shl(failures - 1).unwrap_or(u32::MAX);
        self.backoff_mul(factor)
    }

    fn backoff_mul(&self, factor: u32) -> Duration {
        self.base
            .checked_mul(factor)
            .map_or(self.max, |d| d.min(self.max))
    }
}

/// The dedup-ing, backoff-aware work queue driving one controller.
///
/// Lifecycle of a key: [`add`](WorkQueue::add) →
/// [`take`](WorkQueue::take) (caller reconciles) → exactly one of
/// [`forget`](WorkQueue::forget) (success),
/// [`retry`](WorkQueue::retry) (failure → delayed re-add with backoff), or
/// [`requeue_after`](WorkQueue::requeue_after) (voluntary re-run) → then
/// [`done`](WorkQueue::done). Delayed re-adds become due via
/// [`drain_ready`](WorkQueue::drain_ready).
#[derive(Debug, Default)]
pub struct WorkQueue {
    fifo: VecDeque<String>,
    queued: HashSet<String>,
    processing: HashSet<String>,
    dirty: HashSet<String>,
    failures: HashMap<String, u32>,
    delayed: Vec<(Instant, String)>,
    backoff: Backoff,
}

impl WorkQueue {
    #[must_use]
    pub fn new(backoff: Backoff) -> Self {
        Self {
            backoff,
            ..Self::default()
        }
    }

    /// Enqueue `key` for reconciliation. Coalesces: a key already queued is
    /// not queued twice; a key currently being processed is marked dirty and
    /// re-queued when [`done`](WorkQueue::done) is called for it.
    pub fn add(&mut self, key: &str) {
        if self.processing.contains(key) {
            self.dirty.insert(key.to_owned());
            return;
        }
        if self.queued.insert(key.to_owned()) {
            self.fifo.push_back(key.to_owned());
        }
    }

    /// Pop the next key to reconcile, marking it in-processing.
    pub fn take(&mut self) -> Option<String> {
        let key = self.fifo.pop_front()?;
        self.queued.remove(&key);
        self.processing.insert(key.clone());
        Some(key)
    }

    /// Mark a key's reconcile finished. If the key went dirty while it was
    /// being processed (a change landed mid-reconcile), it is re-queued so the
    /// newer state is observed — the no-lost-update half of level-triggering.
    pub fn done(&mut self, key: &str) {
        self.processing.remove(key);
        if self.dirty.remove(key) {
            self.add(key);
        }
    }

    /// Record a failed reconcile and schedule the delayed retry under the
    /// backoff policy. Returns the consecutive-failure count.
    pub fn retry(&mut self, key: &str, now: Instant) -> u32 {
        let n = self.failures.entry(key.to_owned()).or_insert(0);
        *n += 1;
        let due = now + self.backoff.delay(*n);
        self.delayed.push((due, key.to_owned()));
        *n
    }

    /// Schedule a voluntary re-run of `key` after `delay` (no failure counted).
    pub fn requeue_after(&mut self, key: &str, delay: Duration, now: Instant) {
        self.delayed.push((now + delay, key.to_owned()));
    }

    /// Reset the failure count for `key` (call on success).
    pub fn forget(&mut self, key: &str) {
        self.failures.remove(key);
    }

    /// Move every delayed key whose due time has arrived back into the queue.
    /// Returns how many came due (dedup may coalesce them into fewer entries).
    pub fn drain_ready(&mut self, now: Instant) -> usize {
        let mut ready = Vec::new();
        self.delayed.retain(|(due, key)| {
            if *due <= now {
                ready.push(key.clone());
                false
            } else {
                true
            }
        });
        for key in &ready {
            self.add(key);
        }
        ready.len()
    }

    /// Consecutive failures recorded for `key`.
    #[must_use]
    pub fn failures(&self, key: &str) -> u32 {
        self.failures.get(key).copied().unwrap_or(0)
    }

    /// Keys currently queued (excludes delayed and in-processing keys).
    #[must_use]
    pub fn len(&self) -> usize {
        self.fifo.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fifo.is_empty()
    }

    /// Keys scheduled for a future re-add.
    #[must_use]
    pub fn delayed_len(&self) -> usize {
        self.delayed.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_adds_coalesce() {
        let mut q = WorkQueue::default();
        q.add("a");
        q.add("a");
        q.add("a");
        assert_eq!(q.len(), 1);
        assert_eq!(q.take().as_deref(), Some("a"));
        assert_eq!(q.take(), None);
    }

    #[test]
    fn add_during_processing_marks_dirty_and_requeues_on_done() {
        let mut q = WorkQueue::default();
        q.add("a");
        let key = q.take().unwrap();
        // A change lands while "a" is being reconciled…
        q.add("a");
        assert!(q.is_empty(), "dirty key must not re-enter the queue early");
        // …so finishing the run re-queues it for the newer state.
        q.done(&key);
        assert_eq!(q.take().as_deref(), Some("a"));
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let b = Backoff {
            base: Duration::from_millis(10),
            max: Duration::from_millis(100),
        };
        assert_eq!(b.delay(0), Duration::ZERO);
        assert_eq!(b.delay(1), Duration::from_millis(10));
        assert_eq!(b.delay(2), Duration::from_millis(20));
        assert_eq!(b.delay(3), Duration::from_millis(40));
        assert_eq!(b.delay(5), Duration::from_millis(100), "capped at max");
        assert_eq!(b.delay(31), Duration::from_millis(100), "no overflow");
        assert_eq!(b.delay(u32::MAX), Duration::from_millis(100), "no overflow");
    }

    #[test]
    fn retry_schedules_delayed_and_drain_respects_due_time() {
        let mut q = WorkQueue::new(Backoff {
            base: Duration::from_millis(10),
            max: Duration::from_secs(1),
        });
        let now = Instant::now();
        q.add("a");
        let key = q.take().unwrap();
        assert_eq!(q.retry(&key, now), 1);
        q.done(&key);
        // Not due yet: nothing drains at `now`.
        assert_eq!(q.drain_ready(now), 0);
        assert!(q.is_empty());
        assert_eq!(q.delayed_len(), 1);
        // Due once the backoff delay has passed.
        assert_eq!(q.drain_ready(now + Duration::from_millis(10)), 1);
        assert_eq!(q.take().as_deref(), Some("a"));
    }

    #[test]
    fn failures_accumulate_and_forget_resets() {
        let mut q = WorkQueue::default();
        let now = Instant::now();
        q.add("a");
        let key = q.take().unwrap();
        assert_eq!(q.retry(&key, now), 1);
        assert_eq!(q.retry(&key, now), 2);
        assert_eq!(q.failures("a"), 2);
        q.forget(&key);
        assert_eq!(q.failures("a"), 0);
    }

    #[test]
    fn drained_duplicates_coalesce() {
        let mut q = WorkQueue::default();
        let now = Instant::now();
        // The same key scheduled twice (two failed replicas of one event)…
        q.requeue_after("a", Duration::ZERO, now);
        q.requeue_after("a", Duration::ZERO, now);
        // …drains as two due entries but one queued run.
        assert_eq!(q.drain_ready(now), 2);
        assert_eq!(q.len(), 1);
    }
}
