//! Air-gap and connectivity state for the MAI runtime.
//!
//! This module owns the canonical [`ConnectivityState`] enum that every
//! trust-aware crate consumes — `mai-adapters` for host validation,
//! `mai-api` for bind enforcement and status reporting, and
//! `mai-compliance` for [`TrustContext`] decisions. Centralising the enum
//! prevents the inference path and the compliance path from drifting into
//! two incompatible notions of "the system is air-gapped".
//!
//! # State model
//!
//! The five states model the union of hardware-air-gap
//! semantics and the trust-cache freshness semantics:
//!
//! | State | Trigger | Behaviour |
//! |---|---|---|
//! | [`Connected`](ConnectivityState::Connected) | network reachable, fresh trust bundle | live validation, cloud routes permitted |
//! | [`Degraded`](ConnectivityState::Degraded) | network reachable, signed cache only | local validation against cached bundle |
//! | [`StaleNotExpired`](ConnectivityState::StaleNotExpired) | cache aged past warn threshold | warn + continue with restrictions |
//! | [`Expired`](ConnectivityState::Expired) | cache aged past hard expiry | restrict to local admin / emergency mode |
//! | [`AirGapped`](ConnectivityState::AirGapped) | hardware switch engaged | local-only inference, no cloud route |
//!
//! # Air-gap default
//!
//! [`AirGapPolicy::default`] starts in [`ConnectivityState::AirGapped`].
//! Network connectivity is the exception that must be explicitly granted
//! by transitioning to [`ConnectivityState::Connected`], not the assumption.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

/// Canonical connectivity state shared across the inference and compliance
/// planes. Values are ordered from least-restrictive to most-restrictive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectivityState {
    /// Network reachable, fresh trust bundle, live revocation lookups.
    Connected,
    /// Network reachable but trust validation falls back to the signed
    /// local cache (recent OpenBao outage, partition, etc.).
    Degraded,
    /// Local trust cache is past the warn threshold but still within the
    /// hard expiry window. Local routes continue with restrictions.
    StaleNotExpired,
    /// Local trust cache has crossed the hard expiry window. Only emergency
    /// local-admin operations should proceed.
    Expired,
    /// Hardware air-gap switch is engaged. No outbound network traffic,
    /// regardless of trust-cache state. Local inference only.
    AirGapped,
}

impl Default for ConnectivityState {
    /// Air-gap is the secure default — the system assumes no network until
    /// a controller explicitly transitions to a less-restrictive state.
    fn default() -> Self {
        Self::AirGapped
    }
}

impl ConnectivityState {
    /// True iff the state permits any kind of cloud route (live or cached).
    /// `Expired` and `AirGapped` always return false.
    #[must_use]
    pub fn permits_cloud_route(self) -> bool {
        matches!(self, Self::Connected | Self::Degraded)
    }

    /// True iff the state forces local-only inference, with no cloud
    /// fallback. Includes the hard-expiry case in addition to the
    /// hardware switch.
    #[must_use]
    pub fn requires_local_only(self) -> bool {
        matches!(self, Self::Expired | Self::AirGapped)
    }

    /// True iff this is the hardware-air-gap state.
    #[must_use]
    pub fn is_air_gapped(self) -> bool {
        matches!(self, Self::AirGapped)
    }

    /// Backwards-compatible flag for code paths that still consult a
    /// boolean `offline_mode`. Returns true for anything that is not
    /// fully-connected.
    #[must_use]
    pub fn is_offline_mode(self) -> bool {
        !matches!(self, Self::Connected)
    }

    /// Human-readable label for status endpoints and audit logs.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Degraded => "degraded",
            Self::StaleNotExpired => "stale-not-expired",
            Self::Expired => "expired",
            Self::AirGapped => "air-gapped",
        }
    }
}

impl std::fmt::Display for ConnectivityState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Cross-component air-gap policy with watch-based state-change notifications.
///
/// Callers observe transitions via [`AirGapPolicy::subscribe`]; the
/// `watch::Receiver` emits the latest state on every transition without
/// missing intermediate values (only the latest is delivered, which is the
/// correct semantics for "the system is currently in state X").
///
/// The struct is cheap to clone — `Arc` internally.
#[derive(Debug, Clone)]
pub struct AirGapPolicy {
    inner: Arc<AirGapPolicyInner>,
}

#[derive(Debug)]
struct AirGapPolicyInner {
    tx: watch::Sender<ConnectivityState>,
}

impl AirGapPolicy {
    /// Construct a policy starting in the given state. Use
    /// [`AirGapPolicy::default`] (which is `AirGapped`) unless the caller
    /// has positive evidence of network availability.
    #[must_use]
    pub fn new(initial: ConnectivityState) -> Self {
        let (tx, _rx) = watch::channel(initial);
        Self {
            inner: Arc::new(AirGapPolicyInner { tx }),
        }
    }

    /// Read the current state.
    #[must_use]
    pub fn state(&self) -> ConnectivityState {
        *self.inner.tx.borrow()
    }

    /// Transition to a new state. Returns the previous state. Idempotent
    /// transitions (new == old) are still broadcast to subscribers so that
    /// "still in state X after re-verification" can be observed.
    pub fn set_state(&self, next: ConnectivityState) -> ConnectivityState {
        let prev = self.state();
        // `send` only fails when there are no receivers — which is fine,
        // we still update the value via send_replace.
        self.inner.tx.send_replace(next);
        prev
    }

    /// Subscribe to state changes. The receiver immediately reports the
    /// current state on first poll.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<ConnectivityState> {
        self.inner.tx.subscribe()
    }

    /// Shorthand for `self.state().permits_cloud_route()`.
    #[must_use]
    pub fn permits_cloud_route(&self) -> bool {
        self.state().permits_cloud_route()
    }

    /// Shorthand for `self.state().is_air_gapped()`.
    #[must_use]
    pub fn is_air_gapped(&self) -> bool {
        self.state().is_air_gapped()
    }
}

impl Default for AirGapPolicy {
    /// Default is `AirGapped` — the safe assumption when nothing has
    /// proven the network is available.
    fn default() -> Self {
        Self::new(ConnectivityState::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_air_gapped() {
        assert_eq!(ConnectivityState::default(), ConnectivityState::AirGapped);
        assert!(AirGapPolicy::default().is_air_gapped());
    }

    #[test]
    fn permits_cloud_route_only_for_connected_and_degraded() {
        assert!(ConnectivityState::Connected.permits_cloud_route());
        assert!(ConnectivityState::Degraded.permits_cloud_route());
        assert!(!ConnectivityState::StaleNotExpired.permits_cloud_route());
        assert!(!ConnectivityState::Expired.permits_cloud_route());
        assert!(!ConnectivityState::AirGapped.permits_cloud_route());
    }

    #[test]
    fn requires_local_only_for_expired_and_air_gapped() {
        assert!(!ConnectivityState::Connected.requires_local_only());
        assert!(!ConnectivityState::Degraded.requires_local_only());
        assert!(!ConnectivityState::StaleNotExpired.requires_local_only());
        assert!(ConnectivityState::Expired.requires_local_only());
        assert!(ConnectivityState::AirGapped.requires_local_only());
    }

    #[test]
    fn offline_mode_back_compat() {
        // The legacy boolean flag should be false only for `Connected`.
        assert!(!ConnectivityState::Connected.is_offline_mode());
        for s in [
            ConnectivityState::Degraded,
            ConnectivityState::StaleNotExpired,
            ConnectivityState::Expired,
            ConnectivityState::AirGapped,
        ] {
            assert!(s.is_offline_mode(), "{s} must report offline_mode=true");
        }
    }

    #[tokio::test]
    async fn policy_observes_transitions() {
        let policy = AirGapPolicy::new(ConnectivityState::Connected);
        let mut rx = policy.subscribe();
        assert_eq!(*rx.borrow(), ConnectivityState::Connected);

        let prev = policy.set_state(ConnectivityState::Degraded);
        assert_eq!(prev, ConnectivityState::Connected);
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), ConnectivityState::Degraded);

        policy.set_state(ConnectivityState::AirGapped);
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), ConnectivityState::AirGapped);
        assert!(policy.is_air_gapped());
    }

    #[test]
    fn display_uses_kebab_labels() {
        assert_eq!(ConnectivityState::AirGapped.to_string(), "air-gapped");
        assert_eq!(
            ConnectivityState::StaleNotExpired.to_string(),
            "stale-not-expired"
        );
    }
}
