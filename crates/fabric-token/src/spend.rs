//! X1 — the spend ledger, promoted behind a trait.
//!
//! F3's contract is that a budget-carrying token is *atomically* metered. In a
//! single process the G9a in-memory ledger honors it; the moment a second
//! replica exists, per-process counters make the promise false under load — a
//! $100 cap bills up to ~$N×100 (addendum A2, Tier 2). This module is the fix:
//!
//! * [`SpendLedger`] — the trait the gateway's private ledger is promoted
//!   behind; [`LocalSpendLedger`] is that ledger verbatim (single-process
//!   behavior unchanged).
//! * [`LeasedSpendLedger`] — the shared implementation. Each replica **leases
//!   a bounded slice** of the shared budget from a [`LeaseStore`] (an atomic,
//!   compare-and-set-backed record) and then decrements **locally**: the hot
//!   path makes no shared-store round-trip; one acquisition amortizes over
//!   ~`slice / per-call-usage` calls. The store never leases past the cap, so
//!   the estate-wide spend can never exceed it; what a replica *can* strand is
//!   its unspent slice — worst-case cross-replica over-reservation is one
//!   slice per replica. **ε = replicas × slice**, a published number, not an
//!   accident.
//!
//! Fail-closed: an axis the shared pool cannot fund denies the spend; a store
//! error denies the spend (never "assume room").

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use fabric_contracts::Budget;

/// Accumulated usage across the three budget axes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Spent {
    pub tokens: u64,
    pub usd_cents: u64,
    pub tool_calls: u32,
}

/// Immutable accounting namespace shared by gateway, mission, and guard
/// enforcement. Every token descendant uses the root lineage id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReservationKey {
    pub tenant_id: String,
    pub root_lineage: String,
    pub mission_id: Option<String>,
    pub system: Option<String>,
}

impl ReservationKey {
    #[must_use]
    pub fn for_token(
        token: &fabric_contracts::TrustToken,
        mission_id: Option<String>,
        system: Option<String>,
    ) -> Self {
        Self {
            tenant_id: token.tenant_id.clone(),
            root_lineage: crate::lineage_key(token).to_owned(),
            mission_id,
            system,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReservationError {
    #[error("reservation would exceed an authority ceiling")]
    Exhausted,
    #[error("reservation is no longer pending")]
    NotPending,
}

#[derive(Debug, Default)]
struct ReservationState {
    committed: Spent,
    reserved: Spent,
    pending: HashMap<u64, Spent>,
}

#[derive(Debug, Default)]
struct ReservationInner {
    next_id: AtomicU64,
    state: Mutex<HashMap<ReservationKey, ReservationState>>,
}

/// Atomic reserve/commit/release ledger for authority ceilings. A reservation
/// releases automatically on drop, including async cancellation paths.
#[derive(Debug, Clone, Default)]
pub struct ReservationLedger {
    inner: Arc<ReservationInner>,
}

impl ReservationLedger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Atomically reserve `usage` against `cap`. Concurrent reservations count
    /// immediately, so check-then-charge races cannot cross the ceiling.
    pub fn reserve(
        &self,
        key: ReservationKey,
        cap: &Budget,
        usage: Spent,
    ) -> Result<Reservation, ReservationError> {
        let mut all = self.inner.state.lock().expect("reservation ledger lock");
        let state = all.entry(key.clone()).or_default();
        let in_flight = state.committed.saturating_add(state.reserved);
        let proposed = in_flight.saturating_add(usage);
        if exceeds(cap, proposed) {
            return Err(ReservationError::Exhausted);
        }
        let id = self
            .inner
            .next_id
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        state.reserved = state.reserved.saturating_add(usage);
        state.pending.insert(id, usage);
        Ok(Reservation {
            id,
            key,
            usage,
            inner: Arc::downgrade(&self.inner),
            settled: false,
        })
    }

    #[must_use]
    pub fn committed(&self, key: &ReservationKey) -> Spent {
        self.inner
            .state
            .lock()
            .expect("reservation ledger lock")
            .get(key)
            .map_or_else(Spent::default, |state| state.committed)
    }
}

/// One in-flight authority reservation. Call [`commit`](Self::commit) after a
/// successful side effect or [`release`](Self::release) on an explicit abort;
/// dropping an unsettled value releases it automatically.
#[derive(Debug)]
pub struct Reservation {
    id: u64,
    key: ReservationKey,
    usage: Spent,
    inner: Weak<ReservationInner>,
    settled: bool,
}

impl Reservation {
    #[must_use]
    pub fn key(&self) -> &ReservationKey {
        &self.key
    }

    #[must_use]
    pub fn usage(&self) -> Spent {
        self.usage
    }

    pub fn commit(mut self) -> Result<(), ReservationError> {
        self.settle(true)
    }

    pub fn release(mut self) -> Result<(), ReservationError> {
        self.settle(false)
    }

    fn settle(&mut self, commit: bool) -> Result<(), ReservationError> {
        let Some(inner) = self.inner.upgrade() else {
            self.settled = true;
            return Err(ReservationError::NotPending);
        };
        settle(&inner, &self.key, self.id, commit)?;
        self.settled = true;
        Ok(())
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        if let Some(inner) = self.inner.upgrade() {
            let _ = settle(&inner, &self.key, self.id, false);
        }
        self.settled = true;
    }
}

fn settle(
    inner: &ReservationInner,
    key: &ReservationKey,
    id: u64,
    commit: bool,
) -> Result<(), ReservationError> {
    let mut all = inner.state.lock().expect("reservation ledger lock");
    let state = all.get_mut(key).ok_or(ReservationError::NotPending)?;
    let usage = state
        .pending
        .remove(&id)
        .ok_or(ReservationError::NotPending)?;
    state.reserved = state.reserved.saturating_sub(usage);
    if commit {
        state.committed = state.committed.saturating_add(usage);
    }
    Ok(())
}

fn exceeds(cap: &Budget, usage: Spent) -> bool {
    (cap.token_cap > 0 && cap.tokens_spent.saturating_add(usage.tokens) > cap.token_cap)
        || (cap.usd_cap_cents > 0
            && cap.usd_spent_cents.saturating_add(usage.usd_cents) > cap.usd_cap_cents)
        || (cap.tool_call_cap > 0
            && cap.tool_calls_spent.saturating_add(usage.tool_calls) > cap.tool_call_cap)
}

impl Spent {
    /// Saturating per-axis sum.
    #[must_use]
    pub fn saturating_add(self, other: Spent) -> Spent {
        Spent {
            tokens: self.tokens.saturating_add(other.tokens),
            usd_cents: self.usd_cents.saturating_add(other.usd_cents),
            tool_calls: self.tool_calls.saturating_add(other.tool_calls),
        }
    }

    #[must_use]
    pub fn saturating_sub(self, other: Spent) -> Spent {
        Spent {
            tokens: self.tokens.saturating_sub(other.tokens),
            usd_cents: self.usd_cents.saturating_sub(other.usd_cents),
            tool_calls: self.tool_calls.saturating_sub(other.tool_calls),
        }
    }

    /// All axes zero.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.tokens == 0 && self.usd_cents == 0 && self.tool_calls == 0
    }
}

/// The ledger a gateway meters runtime spend through (X1: G9a promoted behind
/// a trait). `add` records one completed call's usage; `fold` folds a key's
/// accumulated usage into a budget's `*_spent` counters for the pre-flight.
pub trait SpendLedger: Send + Sync {
    fn add(&self, key: &str, usage: Spent);
    fn fold(&self, key: &str, budget: &mut Budget);
}

/// The single-process ledger — G9a's semantics, verbatim.
#[derive(Debug, Default)]
pub struct LocalSpendLedger {
    inner: Mutex<HashMap<String, Spent>>,
}

impl LocalSpendLedger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SpendLedger for LocalSpendLedger {
    fn add(&self, key: &str, usage: Spent) {
        let mut g = self.inner.lock().expect("spend ledger lock");
        let e = g.entry(key.to_owned()).or_default();
        *e = e.saturating_add(usage);
    }

    fn fold(&self, key: &str, budget: &mut Budget) {
        let g = self.inner.lock().expect("spend ledger lock");
        if let Some(s) = g.get(key) {
            budget.tokens_spent = budget.tokens_spent.saturating_add(s.tokens);
            budget.usd_spent_cents = budget.usd_spent_cents.saturating_add(s.usd_cents);
            budget.tool_calls_spent = budget.tool_calls_spent.saturating_add(s.tool_calls);
        }
    }
}

/// A spend-path failure. Fail-closed: the caller denies the spend.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SpendError(pub String);

/// The shared, atomic budget record leases are claimed from. Implementations
/// back this with a compare-and-set store (OpenBao KV v2 CAS in production);
/// `acquire` must be atomic across replicas: concurrent claims never lease
/// past the cap in total.
pub trait LeaseStore: Send + Sync {
    /// Claim up to `want` of `cap`'s remaining shared budget for `key`.
    /// Returns what was granted per axis: `min(want, cap − leased-so-far)` for
    /// a metered axis (cap > 0), `want` for an unmetered one (cap = 0).
    fn acquire(
        &self,
        key: &str,
        cap: &Budget,
        want: Spent,
    ) -> impl Future<Output = Result<Spent, SpendError>> + Send;
}

impl<T: LeaseStore> LeaseStore for Arc<T> {
    fn acquire(
        &self,
        key: &str,
        cap: &Budget,
        want: Spent,
    ) -> impl Future<Output = Result<Spent, SpendError>> + Send {
        T::acquire(self, key, cap, want)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct LeaseState {
    leased: Spent,
    spent: Spent,
    /// Axes the shared pool has refused (want > 0, granted 0). A fixed cap
    /// never refills — leases only grow — so dryness is permanent, and a dry
    /// axis denies locally with no further store round-trip.
    dry: DryAxes,
}

#[derive(Debug, Clone, Copy, Default)]
struct DryAxes {
    tokens: bool,
    usd_cents: bool,
    tool_calls: bool,
}

impl DryAxes {
    /// Any axis this request needs (want > 0) already known dry?
    fn blocks(self, want: Spent) -> bool {
        (want.tokens > 0 && self.tokens)
            || (want.usd_cents > 0 && self.usd_cents)
            || (want.tool_calls > 0 && self.tool_calls)
    }

    /// Record every axis that was wanted but granted nothing.
    fn mark(&mut self, want: Spent, granted: Spent) {
        self.tokens |= want.tokens > 0 && granted.tokens == 0;
        self.usd_cents |= want.usd_cents > 0 && granted.usd_cents == 0;
        self.tool_calls |= want.tool_calls > 0 && granted.tool_calls == 0;
    }
}

/// Whether `spent + usage` fits inside `leased` on every metered axis.
fn fits(state: LeaseState, usage: Spent, cap: &Budget) -> bool {
    (cap.token_cap == 0 || state.spent.tokens.saturating_add(usage.tokens) <= state.leased.tokens)
        && (cap.usd_cap_cents == 0
            || state.spent.usd_cents.saturating_add(usage.usd_cents) <= state.leased.usd_cents)
        && (cap.tool_call_cap == 0
            || state.spent.tool_calls.saturating_add(usage.tool_calls) <= state.leased.tool_calls)
}

/// What to request from the store: per metered axis with a shortfall,
/// `max(slice, deficit)`; nothing for axes that already fit or are unmetered.
fn wanted(state: LeaseState, usage: Spent, cap: &Budget, slice: Spent) -> Spent {
    let deficit_u64 = |spent: u64, use_: u64, leased: u64, slice: u64| {
        let need = spent.saturating_add(use_);
        if need > leased {
            slice.max(need - leased)
        } else {
            0
        }
    };
    Spent {
        tokens: if cap.token_cap == 0 {
            0
        } else {
            deficit_u64(
                state.spent.tokens,
                usage.tokens,
                state.leased.tokens,
                slice.tokens,
            )
        },
        usd_cents: if cap.usd_cap_cents == 0 {
            0
        } else {
            deficit_u64(
                state.spent.usd_cents,
                usage.usd_cents,
                state.leased.usd_cents,
                slice.usd_cents,
            )
        },
        tool_calls: if cap.tool_call_cap == 0 {
            0
        } else {
            let need = state.spent.tool_calls.saturating_add(usage.tool_calls);
            if need > state.leased.tool_calls {
                slice.tool_calls.max(need - state.leased.tool_calls)
            } else {
                0
            }
        },
    }
}

/// A needed axis (want > 0) that was granted nothing is dry — the spend can
/// never fit.
fn starved(want: Spent, granted: Spent) -> bool {
    (want.tokens > 0 && granted.tokens == 0)
        || (want.usd_cents > 0 && granted.usd_cents == 0)
        || (want.tool_calls > 0 && granted.tool_calls == 0)
}

/// The lease-based shared spend ledger (X1). Local decrements against a leased
/// slice; the shared store is touched only when the slice runs dry.
pub struct LeasedSpendLedger<S: LeaseStore> {
    store: S,
    slice: Spent,
    state: Mutex<HashMap<String, LeaseState>>,
}

impl<S: LeaseStore> LeasedSpendLedger<S> {
    /// `slice` is this replica's per-acquisition lease — its share of ε.
    #[must_use]
    pub fn new(store: S, slice: Spent) -> Self {
        Self {
            store,
            slice,
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Atomically check-and-commit one usage against the shared cap: approve
    /// and record it, or deny (budget exhausted). Local-first — the store is
    /// consulted only when the local lease cannot cover the usage.
    ///
    /// # Errors
    /// [`SpendError`] on a store failure (the caller must deny the spend).
    pub async fn try_spend(
        &self,
        key: &str,
        cap: &Budget,
        usage: Spent,
    ) -> Result<bool, SpendError> {
        loop {
            // Local attempt (lock never held across an await).
            let want = {
                let mut g = self.state.lock().expect("lease state lock");
                let st = g.entry(key.to_owned()).or_default();
                if fits(*st, usage, cap) {
                    st.spent = st.spent.saturating_add(usage);
                    return Ok(true);
                }
                let want = wanted(*st, usage, cap, self.slice);
                // An axis the pool already refused stays refused (a fixed cap
                // cannot refill): deny locally, no round-trip.
                if st.dry.blocks(want) {
                    return Ok(false);
                }
                want
            };
            // Slice dry: one amortized shared-store acquisition.
            let granted = self.store.acquire(key, cap, want).await?;
            let mut g = self.state.lock().expect("lease state lock");
            let st = g.entry(key.to_owned()).or_default();
            if starved(want, granted) {
                st.dry.mark(want, granted);
                return Ok(false);
            }
            st.leased = st.leased.saturating_add(granted);
            drop(g);
        }
    }

    /// This replica's recorded usage for `key`.
    #[must_use]
    pub fn spent(&self, key: &str) -> Spent {
        self.state
            .lock()
            .expect("lease state lock")
            .get(key)
            .map_or_else(Spent::default, |s| s.spent)
    }

    /// This replica's leased slice total for `key`.
    #[must_use]
    pub fn leased(&self, key: &str) -> Spent {
        self.state
            .lock()
            .expect("lease state lock")
            .get(key)
            .map_or_else(Spent::default, |s| s.leased)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usd(cents: u64) -> Spent {
        Spent {
            usd_cents: cents,
            ..Spent::default()
        }
    }

    #[test]
    fn local_ledger_accumulates_and_folds() {
        let led = LocalSpendLedger::new();
        led.add(
            "tok-1",
            Spent {
                tokens: 100,
                usd_cents: 5,
                tool_calls: 1,
            },
        );
        led.add(
            "tok-1",
            Spent {
                tokens: 50,
                usd_cents: 3,
                tool_calls: 0,
            },
        );
        let mut b = Budget {
            token_cap: 1000,
            usd_cap_cents: 100,
            tool_call_cap: 10,
            ..Default::default()
        };
        led.fold("tok-1", &mut b);
        assert_eq!(b.tokens_spent, 150);
        assert_eq!(b.usd_spent_cents, 8);
        assert_eq!(b.tool_calls_spent, 1);
        let mut other = Budget {
            token_cap: 1000,
            ..Default::default()
        };
        led.fold("unknown", &mut other);
        assert_eq!(other.tokens_spent, 0);
    }

    /// A shared pool with atomic (single-lock) grants + an acquisition counter.
    #[derive(Default)]
    struct FakePool {
        leased: Mutex<HashMap<String, Spent>>,
        acquisitions: Mutex<u32>,
    }

    impl LeaseStore for FakePool {
        fn acquire(
            &self,
            key: &str,
            cap: &Budget,
            want: Spent,
        ) -> impl Future<Output = Result<Spent, SpendError>> + Send {
            let granted = {
                let mut g = self.leased.lock().unwrap();
                *self.acquisitions.lock().unwrap() += 1;
                let leased = g.entry(key.to_owned()).or_default();
                let grant_u64 = |cap: u64, leased: u64, want: u64| {
                    if cap == 0 {
                        want
                    } else {
                        want.min(cap.saturating_sub(leased))
                    }
                };
                let grant = Spent {
                    tokens: grant_u64(cap.token_cap, leased.tokens, want.tokens),
                    usd_cents: grant_u64(cap.usd_cap_cents, leased.usd_cents, want.usd_cents),
                    tool_calls: if cap.tool_call_cap == 0 {
                        want.tool_calls
                    } else {
                        want.tool_calls
                            .min(cap.tool_call_cap.saturating_sub(leased.tool_calls))
                    },
                };
                *leased = leased.saturating_add(grant);
                grant
            };
            async move { Ok(granted) }
        }
    }

    #[tokio::test]
    async fn single_replica_stops_exactly_at_the_cap() {
        let cap = Budget {
            usd_cap_cents: 1_000,
            ..Default::default()
        };
        let pool = Arc::new(FakePool::default());
        let ledger = LeasedSpendLedger::new(Arc::clone(&pool), usd(100));
        let mut approved = 0u64;
        for _ in 0..100 {
            if ledger.try_spend("cap-a", &cap, usd(30)).await.unwrap() {
                approved += 30;
            }
        }
        assert!(approved <= 1_000, "never over the cap (spent {approved})");
        assert!(approved >= 900, "cap utilized to within one slice");
        // Once dry, it stays dry — and denies locally, with no store contact.
        let trips_at_dry = *pool.acquisitions.lock().unwrap();
        assert!(!ledger.try_spend("cap-a", &cap, usd(30)).await.unwrap());
        assert!(!ledger.try_spend("cap-a", &cap, usd(30)).await.unwrap());
        assert_eq!(
            *pool.acquisitions.lock().unwrap(),
            trips_at_dry,
            "post-exhaustion denials are local (a fixed cap cannot refill)"
        );
    }

    #[tokio::test]
    async fn three_replicas_never_over_spend_and_rarely_touch_the_store() {
        let cap = Budget {
            usd_cap_cents: 10_000,
            ..Default::default()
        };
        let pool = Arc::new(FakePool::default());
        let mut handles = Vec::new();
        for _ in 0..3 {
            let ledger = LeasedSpendLedger::new(Arc::clone(&pool), usd(500));
            let cap = cap.clone();
            handles.push(tokio::spawn(async move {
                let mut approved = 0u64;
                let mut calls = 0u32;
                for _ in 0..60 {
                    calls += 1;
                    if ledger.try_spend("cap-x", &cap, usd(100)).await.unwrap() {
                        approved += 100;
                    }
                }
                (approved, calls)
            }));
        }
        let mut total = 0u64;
        let mut calls = 0u32;
        for h in handles {
            let (a, c) = h.await.unwrap();
            total += a;
            calls += c;
        }
        // The F3 contract, cross-replica: never over the cap…
        assert!(total <= 10_000, "over-spend: {total} > 10000");
        // …and under-utilization bounded by ε = replicas × slice.
        assert!(total >= 10_000 - 3 * 500, "under-utilized: {total}");
        // Amortization: shared-store round-trips ≪ calls.
        let acquisitions = *pool.acquisitions.lock().unwrap();
        assert!(
            acquisitions < calls && acquisitions <= 30,
            "acquisitions {acquisitions} not amortized over {calls} calls"
        );
    }

    #[tokio::test]
    async fn unmetered_axes_never_block() {
        // Only USD is metered; tokens/tool-calls flow freely.
        let cap = Budget {
            usd_cap_cents: 200,
            ..Default::default()
        };
        let ledger = LeasedSpendLedger::new(Arc::new(FakePool::default()), usd(100));
        let usage = Spent {
            tokens: 1_000_000,
            usd_cents: 100,
            tool_calls: 99,
        };
        assert!(ledger.try_spend("k", &cap, usage).await.unwrap());
        assert!(ledger.try_spend("k", &cap, usage).await.unwrap());
        assert!(!ledger.try_spend("k", &cap, usage).await.unwrap());
    }

    #[test]
    fn reservation_barrier_never_exceeds_the_cap_under_concurrency() {
        let ledger = Arc::new(ReservationLedger::new());
        let cap = Budget {
            tool_call_cap: 10,
            ..Budget::default()
        };
        let key = ReservationKey {
            tenant_id: "tenant-a".into(),
            root_lineage: "root-a".into(),
            mission_id: Some("mission-a".into()),
            system: Some("system-a".into()),
        };
        let barrier = Arc::new(std::sync::Barrier::new(100));
        let mut workers = Vec::new();
        for _ in 0..100 {
            let ledger = Arc::clone(&ledger);
            let cap = cap.clone();
            let key = key.clone();
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                ledger
                    .reserve(
                        key,
                        &cap,
                        Spent {
                            tool_calls: 1,
                            ..Spent::default()
                        },
                    )
                    .ok()
                    .is_some_and(|reservation| reservation.commit().is_ok())
            }));
        }
        let admitted = workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap()))
            .sum::<usize>();
        assert_eq!(admitted, 10);
        assert_eq!(ledger.committed(&key).tool_calls, 10);
    }

    #[test]
    fn dropped_reservation_releases_capacity() {
        let ledger = ReservationLedger::new();
        let cap = Budget {
            usd_cap_cents: 50,
            ..Budget::default()
        };
        let key = ReservationKey {
            tenant_id: "tenant-a".into(),
            root_lineage: "root-a".into(),
            mission_id: None,
            system: None,
        };
        let pending = ledger.reserve(key.clone(), &cap, usd(50)).unwrap();
        assert_eq!(
            ledger.reserve(key.clone(), &cap, usd(1)).unwrap_err(),
            ReservationError::Exhausted
        );
        drop(pending);
        ledger
            .reserve(key.clone(), &cap, usd(50))
            .unwrap()
            .commit()
            .unwrap();
        assert_eq!(ledger.committed(&key).usd_cents, 50);
    }
}
