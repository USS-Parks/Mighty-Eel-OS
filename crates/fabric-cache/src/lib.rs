//! `fabric-cache` — the WSF Ring-3 connectivity state machine.
//!
//! An appliance must keep making **safe** decisions when the Trust Bridge is
//! unreachable — its safety surface shrinks with staleness rather than failing
//! open. This crate maps trust-material freshness (age vs. soft/hard TTL, cloud
//! reachability, operator air-gap) to a [`ConnectivityState`], and each state to
//! a route ceiling and a new-session policy (TRUST-MANIFOLD §5). Pure logic — no
//! I/O — so the appliance daemon (Phase W) and tests share it.

use fabric_contracts::Route;

/// Connectivity / trust-freshness state, least → most restrictive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectivityState {
    /// Bridge reachable and cached material is fresh (within soft TTL).
    Connected,
    /// Bridge unreachable but cached material is still within soft TTL.
    Degraded,
    /// Past soft TTL, before hard TTL — continue, but no new long-lived sessions.
    Stale,
    /// Past hard TTL — cloud routes blocked, local-admin only.
    Expired,
    /// Operator-asserted air-gap; sticky. Local-only.
    AirGapped,
}

/// Soft / hard TTL window, in seconds. Past soft is [`ConnectivityState::Stale`];
/// past hard is [`ConnectivityState::Expired`].
#[derive(Debug, Clone, Copy)]
pub struct TtlPolicy {
    /// Age after which material is stale (warnings, no new sessions).
    pub soft_ttl_secs: u64,
    /// Age after which material is expired (cloud routes blocked).
    pub hard_ttl_secs: u64,
}

/// The freshness inputs the state machine evaluates.
#[derive(Debug, Clone, Copy)]
pub struct Freshness {
    /// Whether the Trust Bridge is currently reachable.
    pub reachable: bool,
    /// Age of the cached trust material, in seconds.
    pub age_secs: u64,
    /// Operator-asserted air-gap (sticky; cleared only by operator action).
    pub air_gapped: bool,
}

/// Evaluate the connectivity state. Air-gap wins; then expiry; then staleness;
/// then reachability distinguishes Connected from Degraded.
#[must_use]
pub fn evaluate(freshness: &Freshness, ttl: &TtlPolicy) -> ConnectivityState {
    if freshness.air_gapped {
        return ConnectivityState::AirGapped;
    }
    if freshness.age_secs > ttl.hard_ttl_secs {
        return ConnectivityState::Expired;
    }
    if freshness.age_secs > ttl.soft_ttl_secs {
        return ConnectivityState::Stale;
    }
    if freshness.reachable {
        ConnectivityState::Connected
    } else {
        ConnectivityState::Degraded
    }
}

/// The maximum route a state permits — the ceiling applied on top of a token's
/// own `allowed_routes`. Expired and air-gapped force local-only, so a stale or
/// offline appliance can never egress to the cloud.
#[must_use]
pub fn route_ceiling(state: ConnectivityState) -> Route {
    match state {
        ConnectivityState::Connected | ConnectivityState::Degraded => Route::CloudAllowed,
        ConnectivityState::Stale => Route::LocalPreferred,
        ConnectivityState::Expired | ConnectivityState::AirGapped => Route::LocalOnly,
    }
}

/// Whether a new long-lived session may be issued in this state. Only fully
/// fresh states (Connected/Degraded) allow it; stale and beyond do not.
#[must_use]
pub fn allows_new_sessions(state: ConnectivityState) -> bool {
    matches!(
        state,
        ConnectivityState::Connected | ConnectivityState::Degraded
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTL: TtlPolicy = TtlPolicy {
        soft_ttl_secs: 3600,
        hard_ttl_secs: 86_400,
    };

    fn fresh(reachable: bool, age_secs: u64, air_gapped: bool) -> Freshness {
        Freshness {
            reachable,
            age_secs,
            air_gapped,
        }
    }

    #[test]
    fn connected_when_fresh_and_reachable() {
        assert_eq!(
            evaluate(&fresh(true, 100, false), &TTL),
            ConnectivityState::Connected
        );
        assert_eq!(
            route_ceiling(ConnectivityState::Connected),
            Route::CloudAllowed
        );
        assert!(allows_new_sessions(ConnectivityState::Connected));
    }

    #[test]
    fn degraded_when_fresh_but_unreachable() {
        assert_eq!(
            evaluate(&fresh(false, 100, false), &TTL),
            ConnectivityState::Degraded
        );
        assert!(allows_new_sessions(ConnectivityState::Degraded));
    }

    #[test]
    fn stale_between_soft_and_hard() {
        let s = evaluate(&fresh(true, 7200, false), &TTL);
        assert_eq!(s, ConnectivityState::Stale);
        assert_eq!(route_ceiling(s), Route::LocalPreferred);
        assert!(!allows_new_sessions(s));
    }

    #[test]
    fn expired_past_hard_ttl_forces_local_only() {
        let s = evaluate(&fresh(true, 90_000, false), &TTL);
        assert_eq!(s, ConnectivityState::Expired);
        assert_eq!(route_ceiling(s), Route::LocalOnly);
        assert!(!allows_new_sessions(s));
    }

    #[test]
    fn air_gap_wins_over_everything() {
        // Even fresh + reachable, an air-gapped appliance is local-only.
        let s = evaluate(&fresh(true, 10, true), &TTL);
        assert_eq!(s, ConnectivityState::AirGapped);
        assert_eq!(route_ceiling(s), Route::LocalOnly);
        assert!(!allows_new_sessions(s));
    }
}
