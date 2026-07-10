//! `wsf-cache` — the WSF Ring-3 appliance trust cache (daemon core).
//!
//! An appliance must keep making **safe** decisions when the Trust Bridge is
//! unreachable, and must **never egress to the cloud** once stale or air-gapped.
//! This crate holds the last-known-good trust material (the trust-anchor public
//! key + a signed revocation snapshot), tracks freshness, and — for a presented
//! token — verifies it **entirely offline**, then **clamps its routes** down to
//! the connectivity ceiling (`fabric-cache`): a token cleared for the cloud is
//! narrowed to local-only when the bridge is gone or the operator has asserted an
//! air-gap. Its safety surface shrinks with staleness rather than failing open.
//!
//! Pure/offline by design — the whole point is to run with no network. It reuses
//! `fabric-cache` (state machine), `fabric-revocation` (offline revocation), and
//! `fabric-token` (offline verify).

use chrono::{DateTime, Utc};
use fabric_cache::{
    ConnectivityState, Freshness, TtlPolicy, allows_new_sessions, evaluate, route_ceiling,
};
use fabric_contracts::{Route, TrustToken};
use fabric_crypto::providers::MlDsa87Verifier;
use fabric_revocation::RevocationSnapshot;

/// Failures from cache operations.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// A refresh carried a revocation snapshot whose signature did not verify.
    #[error("revocation snapshot signature invalid: {0}")]
    RevocationInvalid(String),
}

/// The decision the appliance issues for a token in the current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The evaluated connectivity state.
    pub connectivity: ConnectivityState,
    /// The route ceiling this state imposes.
    pub route_ceiling: Route,
    /// Whether the token verified offline (signature, expiry, revocation).
    pub token_valid: bool,
    /// The token's routes clamped to the ceiling; empty if the token is invalid.
    pub effective_routes: Vec<Route>,
    /// Whether a new long-lived session may be opened (fresh states + valid token).
    pub allows_new_session: bool,
    /// Human-readable reason (for audit / operator display).
    pub reason: String,
}

/// The Ring-3 trust cache. Holds one trust anchor (the bridge's ML-DSA public
/// key — it signs both tokens and revocation snapshots) and the freshness state.
pub struct Ring3Cache {
    verifier: MlDsa87Verifier,
    trust_anchor: Vec<u8>,
    revocation: Option<RevocationSnapshot>,
    last_refresh_secs: u64,
    ttl: TtlPolicy,
    reachable: bool,
    air_gapped: bool,
}

impl Ring3Cache {
    /// A cache bootstrapped with the trust anchor and a TTL policy. Starts with
    /// no cached revocation snapshot, bridge unreachable, not air-gapped.
    #[must_use]
    pub fn new(trust_anchor: Vec<u8>, ttl: TtlPolicy) -> Self {
        Self {
            verifier: MlDsa87Verifier,
            trust_anchor,
            revocation: None,
            last_refresh_secs: 0,
            ttl,
            reachable: false,
            air_gapped: false,
        }
    }

    /// Apply a bridge poll: verify the revocation snapshot against the anchor,
    /// store it, stamp freshness, mark the bridge reachable. Rejected (unchanged)
    /// if the snapshot signature does not verify.
    ///
    /// # Errors
    /// [`CacheError::RevocationInvalid`] if the snapshot signature fails.
    pub fn refresh(
        &mut self,
        revocation: RevocationSnapshot,
        now_secs: u64,
    ) -> Result<(), CacheError> {
        fabric_revocation::verify(&revocation, &self.verifier, &self.trust_anchor)
            .map_err(|e| CacheError::RevocationInvalid(e.to_string()))?;
        self.revocation = Some(revocation);
        self.last_refresh_secs = now_secs;
        self.reachable = true;
        Ok(())
    }

    /// Record that the bridge poll failed (bridge unreachable). Freshness then
    /// decays with age.
    pub fn mark_unreachable(&mut self) {
        self.reachable = false;
    }

    /// Assert an operator air-gap. **Sticky** — only [`clear_air_gap`] lifts it.
    pub fn set_air_gapped(&mut self) {
        self.air_gapped = true;
    }

    /// Lift the air-gap (operator action).
    pub fn clear_air_gap(&mut self) {
        self.air_gapped = false;
    }

    /// The connectivity state at `now_secs`.
    #[must_use]
    pub fn connectivity(&self, now_secs: u64) -> ConnectivityState {
        // Clock fail-closed (audit P2): a clock at or before the last refresh did
        // not advance — a rollback, a rewound/stopped clock, or a pre-epoch `now`
        // (which `decide` maps to 0). `saturating_sub` would floor the age to 0,
        // read as freshest = online, and re-open cloud egress. Treat non-advancing
        // time as maximally stale so freshness decays to Expired (LocalOnly), never
        // widens. Routed through `evaluate` so a sticky air-gap still takes
        // precedence.
        let age_secs = now_secs
            .checked_sub(self.last_refresh_secs)
            .unwrap_or(u64::MAX);
        evaluate(
            &Freshness {
                reachable: self.reachable,
                age_secs,
                air_gapped: self.air_gapped,
            },
            &self.ttl,
        )
    }

    /// Decide the safe, narrowed routing for `token` at `now`. Verifies the token
    /// offline (signature + expiry + cached revocation), then clamps its routes to
    /// the connectivity ceiling. A stale/offline/air-gapped appliance can never
    /// widen past local-only.
    #[must_use]
    pub fn decide(&self, token: &TrustToken, now: DateTime<Utc>) -> Decision {
        let now_secs = u64::try_from(now.timestamp()).unwrap_or(0);
        let state = self.connectivity(now_secs);
        let ceiling = route_ceiling(state);

        let (token_valid, reason) = self.verify_offline(token, now);
        let effective_routes = if token_valid {
            let mut routes = Vec::new();
            for r in &token.allowed_routes {
                let clamped = clamp_route(*r, ceiling);
                if !routes.contains(&clamped) {
                    routes.push(clamped);
                }
            }
            routes
        } else {
            Vec::new()
        };

        Decision {
            connectivity: state,
            route_ceiling: ceiling,
            token_valid,
            effective_routes,
            allows_new_session: token_valid && allows_new_sessions(state),
            reason,
        }
    }

    fn verify_offline(&self, token: &TrustToken, now: DateTime<Utc>) -> (bool, String) {
        if fabric_token::verify(token, &self.verifier, &self.trust_anchor).is_err() {
            return (false, "signature or revocation-status invalid".to_string());
        }
        if fabric_token::is_expired(token, now).unwrap_or(true) {
            return (false, "token expired".to_string());
        }
        if let Some(snap) = &self.revocation {
            // The complete predicate — token id, subject, signing key, issuer,
            // bundle version, tenant, and service identity — so an offline
            // appliance honors a key-compromise or tenant-deprovision revocation,
            // not just token/subject (the same check every other consumer uses).
            if let Some(dim) = snap.revokes(token) {
                return (false, format!("revoked ({dim}) by cached snapshot"));
            }
        }
        (true, "ok".to_string())
    }
}

fn route_rank(route: Route) -> u8 {
    match route {
        Route::LocalOnly => 0,
        Route::LocalPreferred => 1,
        Route::CloudAllowed => 2,
    }
}

/// Clamp `route` down to `ceiling`: a route more permissive than the ceiling is
/// narrowed to the ceiling (a cloud route under a local-only ceiling → local-only).
fn clamp_route(route: Route, ceiling: Route) -> Route {
    if route_rank(route) <= route_rank(ceiling) {
        route
    } else {
        ceiling
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature};
    use fabric_crypto::Signer;
    use fabric_crypto::providers::RustCryptoMlDsa87;

    const TTL: TtlPolicy = TtlPolicy {
        soft_ttl_secs: 3600,
        hard_ttl_secs: 86_400,
    };

    fn token(signer: &RustCryptoMlDsa87, routes: Vec<Route>) -> TrustToken {
        let now = Utc::now();
        let t = TrustToken {
            token_id: "tok_cache".to_string(),
            issued_at: now.to_rfc3339(),
            expires_at: (now + chrono::Duration::hours(1)).to_rfc3339(),
            issuer: "wsf-trust-bridge".to_string(),
            trust_bundle_version: "2026.07.03".to_string(),
            tenant_id: "tenant-a".to_string(),
            subject_id: None,
            subject_hash: "hmac-sha256:demo".to_string(),
            service_identity: None,
            identity_id: None,
            roles: vec![],
            compliance_scopes: vec![],
            allowed_routes: routes,
            allowed_models: vec![],
            max_data_classification: Classification::Restricted,
            country: None,
            person_type: None,
            offline_mode: false,
            revocation_status: RevocationStatus::Valid,
            budget: None,
            attenuation: Attenuation::default(),
            signature: Signature {
                alg: String::new(),
                key_id: String::new(),
                value: String::new(),
            },
        };
        fabric_token::issue(t, signer).unwrap()
    }

    fn now_secs() -> u64 {
        u64::try_from(Utc::now().timestamp()).unwrap()
    }

    #[test]
    fn fresh_and_reachable_allows_cloud() {
        let signer = RustCryptoMlDsa87::generate("anchor").unwrap();
        let mut cache = Ring3Cache::new(signer.public_key().to_vec(), TTL);
        let snap = fabric_revocation::sign(
            RevocationSnapshot::new("s1", "2026-07-03T00:00:00Z", "2027-01-01T00:00:00Z"),
            &signer,
        )
        .unwrap();
        cache.refresh(snap, now_secs()).unwrap();

        let tok = token(&signer, vec![Route::CloudAllowed]);
        let d = cache.decide(&tok, Utc::now());
        assert_eq!(d.connectivity, ConnectivityState::Connected);
        assert!(d.token_valid);
        assert_eq!(d.effective_routes, vec![Route::CloudAllowed]);
    }

    #[test]
    fn bridge_unreachable_past_hard_ttl_narrows_cloud_to_local() {
        let signer = RustCryptoMlDsa87::generate("anchor").unwrap();
        let mut cache = Ring3Cache::new(signer.public_key().to_vec(), TTL);
        let snap = fabric_revocation::sign(
            RevocationSnapshot::new("s1", "2026-07-03T00:00:00Z", "2027-01-01T00:00:00Z"),
            &signer,
        )
        .unwrap();
        // Refresh a full day+ ago, bridge now unreachable.
        cache.refresh(snap, now_secs() - 90_000).unwrap();
        cache.mark_unreachable();

        let tok = token(&signer, vec![Route::CloudAllowed]);
        let d = cache.decide(&tok, Utc::now());
        assert_eq!(d.connectivity, ConnectivityState::Expired);
        assert!(d.token_valid, "the appliance still issues a decision");
        assert_eq!(
            d.effective_routes,
            vec![Route::LocalOnly],
            "cloud narrowed to local"
        );
        assert!(!d.allows_new_session);
    }

    #[test]
    fn clock_rollback_does_not_reopen_cloud() {
        // Audit P2: refresh fresh, then present a clock rolled back to before the
        // refresh. Freshness must not floor to 0 (Online) and re-open cloud — it
        // fails closed.
        let signer = RustCryptoMlDsa87::generate("anchor").unwrap();
        let mut cache = Ring3Cache::new(signer.public_key().to_vec(), TTL);
        let snap = fabric_revocation::sign(
            RevocationSnapshot::new("s1", "2026-07-03T00:00:00Z", "2027-01-01T00:00:00Z"),
            &signer,
        )
        .unwrap();
        let refresh_secs = now_secs();
        cache.refresh(snap, refresh_secs).unwrap();

        let rolled_back =
            DateTime::from_timestamp(i64::try_from(refresh_secs).unwrap() - 90_000, 0).unwrap();
        let tok = token(&signer, vec![Route::CloudAllowed]);
        let d = cache.decide(&tok, rolled_back);
        assert_eq!(d.connectivity, ConnectivityState::Expired);
        assert_eq!(d.route_ceiling, Route::LocalOnly);
        assert!(!d.effective_routes.contains(&Route::CloudAllowed));
    }

    #[test]
    fn pre_epoch_clock_is_expired() {
        // Audit P2: a pre-epoch (negative) timestamp is nonsense; it must not read
        // as freshest (age 0). It fails closed to Expired / local-only.
        let signer = RustCryptoMlDsa87::generate("anchor").unwrap();
        let mut cache = Ring3Cache::new(signer.public_key().to_vec(), TTL);
        let snap = fabric_revocation::sign(
            RevocationSnapshot::new("s1", "2026-07-03T00:00:00Z", "2027-01-01T00:00:00Z"),
            &signer,
        )
        .unwrap();
        cache.refresh(snap, now_secs()).unwrap();

        let pre_epoch = DateTime::from_timestamp(-1_000_000, 0).unwrap();
        let tok = token(&signer, vec![Route::CloudAllowed]);
        let d = cache.decide(&tok, pre_epoch);
        assert_eq!(d.connectivity, ConnectivityState::Expired);
        assert!(!d.effective_routes.contains(&Route::CloudAllowed));
    }

    #[test]
    fn air_gap_denies_cloud_routes() {
        let signer = RustCryptoMlDsa87::generate("anchor").unwrap();
        let mut cache = Ring3Cache::new(signer.public_key().to_vec(), TTL);
        let snap = fabric_revocation::sign(
            RevocationSnapshot::new("s1", "2026-07-03T00:00:00Z", "2027-01-01T00:00:00Z"),
            &signer,
        )
        .unwrap();
        cache.refresh(snap, now_secs()).unwrap();
        cache.set_air_gapped();

        let tok = token(&signer, vec![Route::CloudAllowed, Route::LocalPreferred]);
        let d = cache.decide(&tok, Utc::now());
        assert_eq!(d.connectivity, ConnectivityState::AirGapped);
        assert_eq!(d.route_ceiling, Route::LocalOnly);
        assert_eq!(
            d.effective_routes,
            vec![Route::LocalOnly],
            "no cloud egress under air-gap"
        );
    }

    #[test]
    fn revoked_token_denied_offline() {
        let signer = RustCryptoMlDsa87::generate("anchor").unwrap();
        let mut cache = Ring3Cache::new(signer.public_key().to_vec(), TTL);
        let tok = token(&signer, vec![Route::CloudAllowed]);
        // A snapshot that revokes this token.
        let mut snap =
            RevocationSnapshot::new("s1", "2026-07-03T00:00:00Z", "2027-01-01T00:00:00Z");
        snap.revoked_tokens.push(tok.token_id.clone());
        let snap = fabric_revocation::sign(snap, &signer).unwrap();
        cache.refresh(snap, now_secs()).unwrap();

        let d = cache.decide(&tok, Utc::now());
        assert!(!d.token_valid);
        assert!(d.effective_routes.is_empty());
        assert!(d.reason.contains("revoked"));
    }

    #[test]
    fn revoked_signing_key_denied_offline() {
        // The dimension the old two-check code ignored: a compromised signing
        // key revoked in the snapshot must deny the token offline.
        let signer = RustCryptoMlDsa87::generate("anchor").unwrap();
        let mut cache = Ring3Cache::new(signer.public_key().to_vec(), TTL);
        let tok = token(&signer, vec![Route::CloudAllowed]);
        let mut snap =
            RevocationSnapshot::new("s1", "2026-07-03T00:00:00Z", "2027-01-01T00:00:00Z");
        snap.revoked_signing_keys.push(tok.signature.key_id.clone());
        let snap = fabric_revocation::sign(snap, &signer).unwrap();
        cache.refresh(snap, now_secs()).unwrap();

        let d = cache.decide(&tok, Utc::now());
        assert!(!d.token_valid, "a revoked signing key must deny offline");
        assert!(d.effective_routes.is_empty());
        assert!(d.reason.contains("revoked"));
    }

    #[test]
    fn refresh_rejects_forged_snapshot() {
        let signer = RustCryptoMlDsa87::generate("anchor").unwrap();
        let forger = RustCryptoMlDsa87::generate("forger").unwrap();
        let mut cache = Ring3Cache::new(signer.public_key().to_vec(), TTL);
        // Signed by the wrong key → refresh must reject.
        let snap = fabric_revocation::sign(
            RevocationSnapshot::new("s1", "2026-07-03T00:00:00Z", "2027-01-01T00:00:00Z"),
            &forger,
        )
        .unwrap();
        assert!(cache.refresh(snap, now_secs()).is_err());
    }

    #[test]
    fn bad_signature_token_denied() {
        let anchor = RustCryptoMlDsa87::generate("anchor").unwrap();
        let other = RustCryptoMlDsa87::generate("other").unwrap();
        let cache = Ring3Cache::new(anchor.public_key().to_vec(), TTL);
        // Token signed by a different key than the cached anchor.
        let tok = token(&other, vec![Route::CloudAllowed]);
        let d = cache.decide(&tok, Utc::now());
        assert!(!d.token_valid);
        assert!(d.effective_routes.is_empty());
    }
}
