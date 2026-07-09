//! Edge admission with the W5 offline-safe cache. The node verifies a
//! runtime token **locally** (signature, expiry, revocation) and narrows the
//! route it may use to what the node's current connectivity safely allows
//! (`fabric-cache`). Fail-static (doctrine I-4): an unreachable control plane or
//! an air-gap only *reduces* privilege — the node keeps issuing safe, narrowed
//! decisions rather than failing, and never widens a route. Air-gapped nodes
//! deny cloud routes (I-8).

use chrono::{DateTime, Utc};

use fabric_cache::{ConnectivityState, Freshness, TtlPolicy};
use fabric_contracts::{Route, TrustToken};
use fabric_crypto::Verifier;
use fabric_revocation::RevocationSnapshot;

/// The node's local admission decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeDecision {
    /// Admitted at the narrowed `route`, under the current connectivity `state`.
    Allow {
        /// The route the caller may use — narrowed to the connectivity ceiling
        /// and the token's own allowance; never wider than requested.
        route: Route,
        /// The connectivity state that set the ceiling.
        state: ConnectivityState,
    },
    /// Refused — the token did not verify, has expired, or is revoked.
    Deny {
        /// Why the token was refused.
        reason: String,
    },
}

impl EdgeDecision {
    fn deny(reason: &str) -> Self {
        EdgeDecision::Deny {
            reason: reason.to_owned(),
        }
    }

    /// Whether the call was admitted.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, EdgeDecision::Allow { .. })
    }

    /// The admitted route, if any.
    #[must_use]
    pub fn route(&self) -> Option<Route> {
        match self {
            EdgeDecision::Allow { route, .. } => Some(*route),
            EdgeDecision::Deny { .. } => None,
        }
    }
}

/// The node edge: it holds the trust anchor key, the freshness TTL policy, and
/// the last-applied revocation snapshot, and admits calls locally.
#[derive(Debug, Clone)]
pub struct EdgeAdmission {
    anchor_public_key: Vec<u8>,
    ttl: TtlPolicy,
    revocation: Option<RevocationSnapshot>,
}

impl EdgeAdmission {
    /// Build an edge that verifies tokens against `anchor_public_key` and applies
    /// the `ttl` freshness policy.
    #[must_use]
    pub fn new(anchor_public_key: Vec<u8>, ttl: TtlPolicy) -> Self {
        Self {
            anchor_public_key,
            ttl,
            revocation: None,
        }
    }

    /// Apply the last-known revocation snapshot (already verified when received).
    #[must_use]
    pub fn with_revocation(mut self, snapshot: RevocationSnapshot) -> Self {
        self.revocation = Some(snapshot);
        self
    }

    /// Admit a call bearing `token` that wants `requested`, given the node's
    /// current `freshness`. Verifies locally, then narrows the route to the
    /// connectivity ceiling and the token's own allowance.
    pub fn admit(
        &self,
        token: &TrustToken,
        requested: Route,
        freshness: &Freshness,
        now: DateTime<Utc>,
        verifier: &dyn Verifier,
    ) -> EdgeDecision {
        if fabric_token::verify(token, verifier, &self.anchor_public_key).is_err() {
            return EdgeDecision::deny("token failed local verification");
        }
        if expired(token, now) {
            return EdgeDecision::deny("token expired");
        }
        if let Some(snapshot) = &self.revocation
            && (snapshot.is_token_revoked(&token.token_id)
                || snapshot.is_subject_revoked(&token.subject_hash))
        {
            return EdgeDecision::deny("token revoked");
        }
        let state = fabric_cache::evaluate(freshness, &self.ttl);
        let ceiling = fabric_cache::route_ceiling(state);
        let route = narrower(narrower(requested, ceiling), token_ceiling(token));
        EdgeDecision::Allow { route, state }
    }
}

/// Rank routes least → most permissive so they can be narrowed.
fn rank(route: Route) -> u8 {
    match route {
        Route::LocalOnly => 0,
        Route::LocalPreferred => 1,
        Route::CloudAllowed => 2,
    }
}

/// The more restrictive of two routes.
fn narrower(a: Route, b: Route) -> Route {
    if rank(a) <= rank(b) { a } else { b }
}

/// The most permissive route a token allows, or `LocalOnly` when it allows none
/// (fail-closed: an unscoped token gets the tightest route).
fn token_ceiling(token: &TrustToken) -> Route {
    token
        .allowed_routes
        .iter()
        .copied()
        .max_by_key(|&r| rank(r))
        .unwrap_or(Route::LocalOnly)
}

/// Whether `token` has expired as of `now` (fail-closed on an unparseable
/// expiry).
fn expired(token: &TrustToken, now: DateTime<Utc>) -> bool {
    match DateTime::parse_from_rfc3339(&token.expires_at) {
        Ok(exp) => now > exp.with_timezone(&Utc),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature};
    use fabric_crypto::Signer;
    use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};

    fn ttl() -> TtlPolicy {
        TtlPolicy {
            soft_ttl_secs: 60,
            hard_ttl_secs: 300,
        }
    }

    fn fresh(reachable: bool, age_secs: u64, air_gapped: bool) -> Freshness {
        Freshness {
            reachable,
            age_secs,
            air_gapped,
        }
    }

    fn token(signer: &dyn Signer, routes: Vec<Route>, expires_in: chrono::Duration) -> TrustToken {
        let now = Utc::now();
        let token = TrustToken {
            token_id: "rt:test".to_owned(),
            issued_at: now.to_rfc3339(),
            expires_at: (now + expires_in).to_rfc3339(),
            issuer: "test".to_owned(),
            trust_bundle_version: "loom".to_owned(),
            tenant_id: "acme".to_owned(),
            subject_id: None,
            subject_hash: "sub:test".to_owned(),
            service_identity: None,
            identity_id: None,
            roles: vec![],
            compliance_scopes: vec![],
            allowed_routes: routes,
            allowed_models: vec![],
            max_data_classification: Classification::Internal,
            country: None,
            person_type: None,
            offline_mode: false,
            revocation_status: RevocationStatus::Valid,
            budget: None,
            attenuation: Attenuation {
                parent_id: None,
                caveats: vec![],
            },
            signature: Signature {
                alg: String::new(),
                key_id: String::new(),
                value: String::new(),
            },
        };
        fabric_token::issue(token, signer).unwrap()
    }

    fn edge(anchor: &RustCryptoMlDsa87) -> EdgeAdmission {
        EdgeAdmission::new(anchor.public_key().to_vec(), ttl())
    }

    #[test]
    fn a_reachable_node_allows_cloud() {
        let anchor = RustCryptoMlDsa87::generate("a").unwrap();
        let tok = token(
            &anchor,
            vec![Route::CloudAllowed],
            chrono::Duration::hours(1),
        );
        let d = edge(&anchor).admit(
            &tok,
            Route::CloudAllowed,
            &fresh(true, 0, false),
            Utc::now(),
            &MlDsa87Verifier,
        );
        assert_eq!(d.route(), Some(Route::CloudAllowed));
    }

    #[test]
    fn an_unreachable_but_fresh_node_still_decides() {
        // Control plane unreachable but within soft TTL → Degraded → still cloud.
        let anchor = RustCryptoMlDsa87::generate("a").unwrap();
        let tok = token(
            &anchor,
            vec![Route::CloudAllowed],
            chrono::Duration::hours(1),
        );
        let d = edge(&anchor).admit(
            &tok,
            Route::CloudAllowed,
            &fresh(false, 10, false),
            Utc::now(),
            &MlDsa87Verifier,
        );
        assert!(d.is_allowed(), "the node keeps issuing decisions offline");
        assert_eq!(d.route(), Some(Route::CloudAllowed));
    }

    #[test]
    fn a_stale_node_narrows_to_local() {
        // Past hard TTL → Expired → LocalOnly (safe, narrowed).
        let anchor = RustCryptoMlDsa87::generate("a").unwrap();
        let tok = token(
            &anchor,
            vec![Route::CloudAllowed],
            chrono::Duration::hours(1),
        );
        let d = edge(&anchor).admit(
            &tok,
            Route::CloudAllowed,
            &fresh(false, 600, false),
            Utc::now(),
            &MlDsa87Verifier,
        );
        assert_eq!(d.route(), Some(Route::LocalOnly));
    }

    #[test]
    fn an_air_gapped_node_denies_cloud() {
        let anchor = RustCryptoMlDsa87::generate("a").unwrap();
        let tok = token(
            &anchor,
            vec![Route::CloudAllowed],
            chrono::Duration::hours(1),
        );
        let d = edge(&anchor).admit(
            &tok,
            Route::CloudAllowed,
            &fresh(true, 0, true),
            Utc::now(),
            &MlDsa87Verifier,
        );
        assert_eq!(
            d.route(),
            Some(Route::LocalOnly),
            "an air-gapped node narrows cloud to local"
        );
    }

    #[test]
    fn a_revoked_token_is_denied() {
        let anchor = RustCryptoMlDsa87::generate("a").unwrap();
        let tok = token(&anchor, vec![Route::LocalOnly], chrono::Duration::hours(1));
        let mut snapshot =
            RevocationSnapshot::new("s", "2026-01-01T00:00:00Z", "2027-01-01T00:00:00Z");
        snapshot.revoked_tokens.push("rt:test".to_owned());
        let edge = edge(&anchor).with_revocation(snapshot);
        let d = edge.admit(
            &tok,
            Route::LocalOnly,
            &fresh(true, 0, false),
            Utc::now(),
            &MlDsa87Verifier,
        );
        assert!(!d.is_allowed());
    }

    #[test]
    fn an_expired_token_is_denied() {
        let anchor = RustCryptoMlDsa87::generate("a").unwrap();
        let tok = token(
            &anchor,
            vec![Route::LocalOnly],
            chrono::Duration::seconds(-10),
        );
        let d = edge(&anchor).admit(
            &tok,
            Route::LocalOnly,
            &fresh(true, 0, false),
            Utc::now(),
            &MlDsa87Verifier,
        );
        assert!(!d.is_allowed());
    }

    #[test]
    fn a_tampered_token_is_denied() {
        let anchor = RustCryptoMlDsa87::generate("a").unwrap();
        let mut tok = token(
            &anchor,
            vec![Route::CloudAllowed],
            chrono::Duration::hours(1),
        );
        tok.tenant_id = "evil".to_owned(); // mutate after signing → signature breaks
        let d = edge(&anchor).admit(
            &tok,
            Route::CloudAllowed,
            &fresh(true, 0, false),
            Utc::now(),
            &MlDsa87Verifier,
        );
        assert!(!d.is_allowed());
    }
}
