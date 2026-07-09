//! Local trust cache.
//!
//! When the Lamprey Trust Bridge is unreachable, the appliance falls
//! back to the most recent signed policy bundle and revocation snapshot
//! held locally. This module is the in-memory state model for that
//! cache; the on-disk format is (this module) and signature
//! verification is (see [`crate::bundle`] and `docs/compliance/TRUST-BUNDLE-SPEC.md`).
//!
//! Two refresh entry points:
//!
//! - [`LocalTrustCache::record_refresh`] — bare-data path used by tests
//!   and trusted in-process bootstrap. No signature verification.
//! - [`LocalTrustCache::record_signed_refresh`] — production path that
//!   accepts a [`crate::bundle::SignedPolicyBundle`] and a verifier.
//!   Verification failure leaves the cache untouched.
//!
//! # Connectivity derivation
//!
//! [`LocalTrustCache::evaluate`] returns the [`ConnectivityState`] the
//! policy runtime should use, given:
//!
//!   * the cache's last successful refresh timestamp,
//!   * the operator-configured warn and hard-expiry thresholds, and
//!   * the air-gap policy carried into the call.
//!
//! The hardware air-gap switch always wins — if the caller passes
//! [`ConnectivityState::AirGapped`], that's returned unchanged. The
//! freshness ladder is only consulted when the switch permits any
//! network traffic.
//!
//! ```text
//! AirGapped       (hardware switch wins)
//!     │
//!     ├─ now - last_refresh < warn    → Connected   (or Degraded if no live link)
//!     ├─ now - last_refresh < expiry  → StaleNotExpired
//!     └─ now - last_refresh >= expiry → Expired
//! ```
//!
//! # Emergency access
//!
//! [`LocalTrustCache::is_emergency_only`] returns true in
//! [`ConnectivityState::Expired`]. Callers that gate maintenance
//! endpoints on this method should require explicit admin
//! authentication; the emergency mode is intentionally narrow.
//!
//! # Offline audit queue
//!
//! Audit events generated while the cache is degraded, stale, or
//! air-gapped accumulate in an in-memory queue surfaced by
//! [`LocalTrustCache::offline_audit_backlog`]. The queue is flushed by
//! the audit subsystem when connectivity returns; the cache
//! itself does not transmit anything.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mai_core::airgap::ConnectivityState;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bundle::{BundleError, BundleVerifier, SignedPolicyBundle};

/// Result of a revocation lookup at the time the cache was last
/// refreshed. Pessimistic — `Unknown` means we have not seen a fresh
/// snapshot and the policy runtime should treat the claim as if it
/// might be revoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotStatus {
    /// Snapshot present and the subject's claim was valid.
    Valid,
    /// Snapshot present and the subject's claim was revoked.
    Revoked,
    /// No snapshot recorded for this subject.
    Unknown,
}

/// A single revocation snapshot for one claim id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationSnapshot {
    /// Claim id this snapshot refers to.
    pub claim_id: String,
    /// Status at the snapshot time.
    pub status: SnapshotStatus,
    /// Unix epoch seconds when the snapshot was taken.
    pub recorded_at_secs: u64,
}

/// Configurable freshness thresholds for the trust cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheThresholds {
    /// Age past which the cache is considered degraded (warn).
    /// Cloud-route decisions may still proceed but should be logged.
    pub warn_after: Duration,
    /// Age past which the cache is hard-expired. Only emergency local
    /// operations should proceed.
    pub expire_after: Duration,
}

impl Default for CacheThresholds {
    /// Sensible defaults: warn after 1 hour, expire after 24 hours.
    /// Operators override per deployment profile (see
    /// `docs/compliance/LOCAL-TRUST-CACHE.md` §3).
    fn default() -> Self {
        Self {
            warn_after: Duration::from_secs(60 * 60),
            expire_after: Duration::from_secs(60 * 60 * 24),
        }
    }
}

/// Errors at cache-construction or update time.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TrustCacheError {
    /// Caller supplied a refresh timestamp in the future relative to the
    /// cache's own clock; refused to prevent clock-skew exploits.
    #[error("refresh timestamp {0} is in the future")]
    FutureRefresh(u64),
    /// Caller supplied an expire-after smaller than warn-after, which
    /// would make the warn band empty.
    #[error("expire_after ({expire:?}) must be >= warn_after ({warn:?})")]
    ThresholdsInverted { warn: Duration, expire: Duration },
    /// The signed bundle handed to [`LocalTrustCache::record_signed_refresh`]
    /// failed verification (expired, tampered, unknown anchor, etc.).
    /// The wrapped [`BundleError`] carries the specific reason; the cache
    /// state is unchanged.
    #[error("signed bundle rejected: {0}")]
    BundleRejected(#[from] BundleError),
    /// The signed bundle's `tenant_id` did not match the expected tenant
    /// the cache is bound to. Refused to prevent cross-tenant policy
    /// injection.
    #[error("tenant mismatch: bundle is for {bundle_tenant:?}, cache is bound to {expected:?}")]
    TenantMismatch {
        expected: String,
        bundle_tenant: String,
    },
}

/// In-memory local trust cache.
///
/// Thread-unsafe by design — call sites that need concurrent access
/// wrap an `Arc<RwLock<LocalTrustCache>>`. The state model is small
/// enough that a single writer + many readers is the natural pattern.
#[derive(Debug, Clone)]
pub struct LocalTrustCache {
    thresholds: CacheThresholds,
    /// Unix epoch seconds of the most recent successful refresh from the
    /// upstream Trust Bridge. `None` until the first refresh lands.
    last_refresh_secs: Option<u64>,
    /// Currently held signed trust bundle version, if any.
    bundle_version: Option<String>,
    /// Per-claim revocation snapshots taken at the last refresh.
    revocations: BTreeMap<String, RevocationSnapshot>,
    /// Audit events that accumulated while degraded / stale / air-gapped.
    /// Cleared by [`Self::drain_offline_backlog`] when connectivity
    /// returns.
    offline_audit_backlog: Vec<String>,
}

impl LocalTrustCache {
    /// Construct an empty cache with the given freshness thresholds.
    pub fn new(thresholds: CacheThresholds) -> Result<Self, TrustCacheError> {
        if thresholds.expire_after < thresholds.warn_after {
            return Err(TrustCacheError::ThresholdsInverted {
                warn: thresholds.warn_after,
                expire: thresholds.expire_after,
            });
        }
        Ok(Self {
            thresholds,
            last_refresh_secs: None,
            bundle_version: None,
            revocations: BTreeMap::new(),
            offline_audit_backlog: Vec::new(),
        })
    }

    /// Record a successful refresh from the upstream Trust Bridge.
    ///
    /// **Bare-data path.** Use this only for tests or trusted in-process
    /// bootstrap. Production code paths must use
    /// [`Self::record_signed_refresh`] so unsigned or invalid bundles are
    /// rejected at the refresh boundary.
    ///
    /// `refresh_secs` must be at-or-before `now_secs`. Snapshots replace
    /// any previously-held entries for the same `claim_id`.
    pub fn record_refresh(
        &mut self,
        bundle_version: impl Into<String>,
        snapshots: Vec<RevocationSnapshot>,
        refresh_secs: u64,
        now_secs: u64,
    ) -> Result<(), TrustCacheError> {
        if refresh_secs > now_secs {
            return Err(TrustCacheError::FutureRefresh(refresh_secs));
        }
        self.bundle_version = Some(bundle_version.into());
        self.last_refresh_secs = Some(refresh_secs);
        for snap in snapshots {
            self.revocations.insert(snap.claim_id.clone(), snap);
        }
        Ok(())
    }

    /// Insert revocation snapshots without replacing the bundle version
    /// or refresh timestamp. Used by the background refresh loop to
    /// incrementally update revocation state between full bundle refreshes.
    ///
    /// Each snapshot replaces any previously-held entry for the same
    /// `claim_id`.
    pub fn record_revocations(&mut self, snapshots: Vec<RevocationSnapshot>) {
        for snap in snapshots {
            self.revocations.insert(snap.claim_id.clone(), snap);
        }
    }

    /// Verify a [`SignedPolicyBundle`] and, on success, apply its contents
    /// to the cache.
    ///
    /// Behavior on failure: the cache is left **completely untouched**.
    /// A bundle that fails verification — expired, tampered, signed by
    /// an unknown anchor, scoped to a different tenant — never clobbers
    /// the last-known-good state. The cache will age naturally past its
    /// warn/expire thresholds per the connectivity ladder.
    ///
    /// `expected_tenant` is the tenant this cache instance is bound to.
    /// Bundles scoped to any other tenant are refused with
    /// [`TrustCacheError::TenantMismatch`] to prevent cross-tenant policy
    /// injection. Pass `None` to skip the tenant check during bring-up
    /// of multi-tenant deployments.
    ///
    /// On success, `last_refresh_secs` is set to `bundle.metadata.issued_at_secs`
    /// and `bundle_version` is set to `bundle.metadata.version`.
    pub fn record_signed_refresh<V: BundleVerifier>(
        &mut self,
        bundle: &SignedPolicyBundle,
        verifier: &V,
        expected_tenant: Option<&str>,
        now_secs: u64,
    ) -> Result<(), TrustCacheError> {
        if let Some(expected) = expected_tenant
            && bundle.metadata.tenant_id != expected
        {
            return Err(TrustCacheError::TenantMismatch {
                expected: expected.to_string(),
                bundle_tenant: bundle.metadata.tenant_id.clone(),
            });
        }
        let payload = bundle.verified_payload(verifier, now_secs)?;
        // verified_payload guarantees issued_at_secs <= now_secs.
        self.bundle_version = Some(bundle.metadata.version.clone());
        self.last_refresh_secs = Some(bundle.metadata.issued_at_secs);
        for snap in &payload.revocations {
            self.revocations.insert(snap.claim_id.clone(), snap.clone());
        }
        Ok(())
    }

    /// Look up the revocation status for a claim id. Returns
    /// [`SnapshotStatus::Unknown`] when no snapshot exists.
    #[must_use]
    pub fn revocation_status(&self, claim_id: &str) -> SnapshotStatus {
        self.revocations
            .get(claim_id)
            .map_or(SnapshotStatus::Unknown, |s| s.status)
    }

    /// Snapshot of every claim currently held in the cache, in stable
    /// `claim_id` order. The returned vector is owned so callers do not
    /// hold the cache lock across the boundary; this is the canonical
    /// read path used by `GET /v1/trust/claims`.
    #[must_use]
    pub fn claims(&self) -> Vec<RevocationSnapshot> {
        self.revocations.values().cloned().collect()
    }

    /// Currently-held bundle version, if any.
    #[must_use]
    pub fn bundle_version(&self) -> Option<&str> {
        self.bundle_version.as_deref()
    }

    /// Most recent refresh time as Unix epoch seconds.
    #[must_use]
    pub fn last_refresh_secs(&self) -> Option<u64> {
        self.last_refresh_secs
    }

    /// Age of the most recent refresh, in seconds, relative to `now_secs`.
    /// Returns `None` if the cache has never been refreshed.
    #[must_use]
    pub fn age_secs(&self, now_secs: u64) -> Option<u64> {
        self.last_refresh_secs.map(|r| now_secs.saturating_sub(r))
    }

    /// Compute the connectivity state given `switch_state` (the hardware
    /// air-gap policy) and the current wall-clock time.
    ///
    /// `switch_state` wins when it is `AirGapped`. Otherwise the cache
    /// age decides between `Connected`, `Degraded`, `StaleNotExpired`,
    /// and `Expired`. A `live_link` flag distinguishes `Connected`
    /// (network reachable, cache fresh) from `Degraded` (cache fresh
    /// but live validation unavailable).
    #[must_use]
    pub fn evaluate(
        &self,
        switch_state: ConnectivityState,
        live_link: bool,
        now_secs: u64,
    ) -> ConnectivityState {
        if switch_state.is_air_gapped() {
            return ConnectivityState::AirGapped;
        }
        let Some(age) = self.age_secs(now_secs) else {
            // Never refreshed → treat as expired.
            return ConnectivityState::Expired;
        };
        let age = Duration::from_secs(age);
        if age >= self.thresholds.expire_after {
            return ConnectivityState::Expired;
        }
        if age >= self.thresholds.warn_after {
            return ConnectivityState::StaleNotExpired;
        }
        if live_link {
            ConnectivityState::Connected
        } else {
            ConnectivityState::Degraded
        }
    }

    /// True when the cache is in emergency-only mode (Expired). Callers
    /// that gate maintenance endpoints on this should additionally
    /// require explicit admin authentication.
    #[must_use]
    pub fn is_emergency_only(
        &self,
        switch_state: ConnectivityState,
        live_link: bool,
        now_secs: u64,
    ) -> bool {
        matches!(
            self.evaluate(switch_state, live_link, now_secs),
            ConnectivityState::Expired
        )
    }

    /// Push an audit event onto the offline backlog. The cache stores
    /// these as opaque strings; defines the JSON shape.
    pub fn enqueue_offline_audit(&mut self, event: impl Into<String>) {
        self.offline_audit_backlog.push(event.into());
    }

    /// Number of audit events waiting to be flushed.
    #[must_use]
    pub fn offline_audit_backlog(&self) -> usize {
        self.offline_audit_backlog.len()
    }

    /// Drain the offline audit backlog. Returns every queued event in
    /// FIFO order. The caller is responsible for
    /// re-queueing on flush failure.
    pub fn drain_offline_backlog(&mut self) -> Vec<String> {
        std::mem::take(&mut self.offline_audit_backlog)
    }

    /// Wall-clock helper for callers that don't have a clock injected.
    /// Returns Unix epoch seconds.
    #[must_use]
    pub fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thresholds(warn: u64, expire: u64) -> CacheThresholds {
        CacheThresholds {
            warn_after: Duration::from_secs(warn),
            expire_after: Duration::from_secs(expire),
        }
    }

    fn snap(claim: &str, status: SnapshotStatus, at: u64) -> RevocationSnapshot {
        RevocationSnapshot {
            claim_id: claim.to_string(),
            status,
            recorded_at_secs: at,
        }
    }

    #[test]
    fn thresholds_inverted_rejected() {
        let err = LocalTrustCache::new(thresholds(120, 60)).unwrap_err();
        assert!(matches!(err, TrustCacheError::ThresholdsInverted { .. }));
    }

    #[test]
    fn future_refresh_rejected() {
        let mut cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        let err = cache.record_refresh("v1", vec![], 1000, 500).unwrap_err();
        assert_eq!(err, TrustCacheError::FutureRefresh(1000));
    }

    #[test]
    fn unknown_revocation_for_unseen_claim() {
        let cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        assert_eq!(cache.revocation_status("c1"), SnapshotStatus::Unknown);
    }

    #[test]
    fn revocation_recorded_after_refresh() {
        let mut cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        cache
            .record_refresh(
                "bundle-2026.05.22",
                vec![
                    snap("c1", SnapshotStatus::Valid, 1000),
                    snap("c2", SnapshotStatus::Revoked, 1000),
                ],
                1000,
                1000,
            )
            .unwrap();
        assert_eq!(cache.revocation_status("c1"), SnapshotStatus::Valid);
        assert_eq!(cache.revocation_status("c2"), SnapshotStatus::Revoked);
        assert_eq!(cache.revocation_status("c3"), SnapshotStatus::Unknown);
        assert_eq!(cache.bundle_version(), Some("bundle-2026.05.22"));
    }

    #[test]
    fn evaluate_never_refreshed_is_expired() {
        let cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        assert_eq!(
            cache.evaluate(ConnectivityState::Connected, true, 1000),
            ConnectivityState::Expired
        );
    }

    #[test]
    fn evaluate_connected_when_fresh_and_live() {
        let mut cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        cache.record_refresh("v1", vec![], 1000, 1000).unwrap();
        assert_eq!(
            cache.evaluate(ConnectivityState::Connected, true, 1010),
            ConnectivityState::Connected
        );
    }

    #[test]
    fn evaluate_degraded_when_fresh_but_no_live_link() {
        let mut cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        cache.record_refresh("v1", vec![], 1000, 1000).unwrap();
        assert_eq!(
            cache.evaluate(ConnectivityState::Connected, false, 1010),
            ConnectivityState::Degraded
        );
    }

    #[test]
    fn evaluate_stale_in_warn_band() {
        let mut cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        cache.record_refresh("v1", vec![], 1000, 1000).unwrap();
        // Age 90s > warn (60s) but < expire (120s).
        assert_eq!(
            cache.evaluate(ConnectivityState::Connected, true, 1090),
            ConnectivityState::StaleNotExpired
        );
    }

    #[test]
    fn evaluate_expired_past_hard_threshold() {
        let mut cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        cache.record_refresh("v1", vec![], 1000, 1000).unwrap();
        // Age 150s >= expire (120s).
        assert_eq!(
            cache.evaluate(ConnectivityState::Connected, true, 1150),
            ConnectivityState::Expired
        );
        assert!(cache.is_emergency_only(ConnectivityState::Connected, true, 1150));
    }

    #[test]
    fn evaluate_air_gapped_overrides_everything() {
        let mut cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        cache.record_refresh("v1", vec![], 1000, 1000).unwrap();
        // Even with a perfectly fresh cache, AirGapped wins.
        assert_eq!(
            cache.evaluate(ConnectivityState::AirGapped, true, 1005),
            ConnectivityState::AirGapped
        );
    }

    #[test]
    fn offline_audit_backlog_drains_in_order() {
        let mut cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        cache.enqueue_offline_audit("event-1");
        cache.enqueue_offline_audit("event-2");
        assert_eq!(cache.offline_audit_backlog(), 2);
        let drained = cache.drain_offline_backlog();
        assert_eq!(drained, vec!["event-1".to_string(), "event-2".to_string()]);
        assert_eq!(cache.offline_audit_backlog(), 0);
    }

    // -----------------------------------------------------------------
    // signed refresh integration tests
    // -----------------------------------------------------------------

    use crate::bundle::{
        AcceptAllBundleVerifier, BundleMetadata, PolicyBundlePayload, RejectAllBundleVerifier,
        SignatureEnvelope, SignedPolicyBundle, payload_hash,
    };

    fn sample_bundle(
        tenant: &str,
        version: &str,
        issued: u64,
        expires: u64,
        revocations: Vec<RevocationSnapshot>,
    ) -> SignedPolicyBundle {
        SignedPolicyBundle {
            metadata: BundleMetadata {
                version: version.to_string(),
                issuer: "trust-bridge".to_string(),
                issued_at_secs: issued,
                expires_at_secs: expires,
                tenant_id: tenant.to_string(),
            },
            payload: PolicyBundlePayload { revocations },
            signature: SignatureEnvelope {
                algorithm: "ml-dsa-87".to_string(),
                public_key_id: "anchor".to_string(),
                // AcceptAllBundleVerifier ignores these bytes.
                bytes_hex: "00".to_string(),
            },
        }
    }

    #[test]
    fn signed_refresh_applies_payload_on_success() {
        let mut cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        let bundle = sample_bundle(
            "tribal-health-demo",
            "2026.05.22.001",
            1_000,
            2_000,
            vec![
                snap("c1", SnapshotStatus::Valid, 1_000),
                snap("c2", SnapshotStatus::Revoked, 1_000),
            ],
        );
        cache
            .record_signed_refresh(
                &bundle,
                &AcceptAllBundleVerifier,
                Some("tribal-health-demo"),
                1_500,
            )
            .unwrap();
        assert_eq!(cache.bundle_version(), Some("2026.05.22.001"));
        assert_eq!(cache.last_refresh_secs(), Some(1_000));
        assert_eq!(cache.revocation_status("c1"), SnapshotStatus::Valid);
        assert_eq!(cache.revocation_status("c2"), SnapshotStatus::Revoked);
    }

    #[test]
    fn signed_refresh_invalid_signature_preserves_state() {
        let mut cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        // Prime the cache with a previous good refresh.
        cache
            .record_refresh(
                "prior-version",
                vec![snap("c1", SnapshotStatus::Valid, 900)],
                900,
                1_000,
            )
            .unwrap();
        let bundle = sample_bundle(
            "tribal-health-demo",
            "new-version",
            1_000,
            2_000,
            vec![snap("c1", SnapshotStatus::Revoked, 1_000)],
        );
        // RejectAllBundleVerifier always fails sig verification.
        let err = cache
            .record_signed_refresh(
                &bundle,
                &RejectAllBundleVerifier,
                Some("tribal-health-demo"),
                1_500,
            )
            .unwrap_err();
        assert!(matches!(err, TrustCacheError::BundleRejected(_)));
        // Cache state must be unchanged.
        assert_eq!(cache.bundle_version(), Some("prior-version"));
        assert_eq!(cache.revocation_status("c1"), SnapshotStatus::Valid);
    }

    #[test]
    fn signed_refresh_expired_bundle_preserves_state() {
        let mut cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        cache.record_refresh("prior", vec![], 900, 1_000).unwrap();
        let bundle = sample_bundle("t", "new", 1_000, 2_000, vec![]);
        // now_secs = 3_000 is past expires_at_secs (2_000).
        let err = cache
            .record_signed_refresh(&bundle, &AcceptAllBundleVerifier, Some("t"), 3_000)
            .unwrap_err();
        assert!(matches!(err, TrustCacheError::BundleRejected(_)));
        assert_eq!(cache.bundle_version(), Some("prior"));
    }

    #[test]
    fn signed_refresh_tenant_mismatch_preserves_state() {
        let mut cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        cache.record_refresh("prior", vec![], 900, 1_000).unwrap();
        let bundle = sample_bundle("other-tenant", "new", 1_000, 2_000, vec![]);
        let err = cache
            .record_signed_refresh(
                &bundle,
                &AcceptAllBundleVerifier,
                Some("expected-tenant"),
                1_500,
            )
            .unwrap_err();
        match err {
            TrustCacheError::TenantMismatch {
                expected,
                bundle_tenant,
            } => {
                assert_eq!(expected, "expected-tenant");
                assert_eq!(bundle_tenant, "other-tenant");
            }
            other => panic!("expected TenantMismatch, got {other:?}"),
        }
        assert_eq!(cache.bundle_version(), Some("prior"));
    }

    #[test]
    fn signed_refresh_tenant_check_can_be_skipped() {
        let mut cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        let bundle = sample_bundle("any-tenant", "v1", 1_000, 2_000, vec![]);
        cache
            .record_signed_refresh(&bundle, &AcceptAllBundleVerifier, None, 1_500)
            .unwrap();
        assert_eq!(cache.bundle_version(), Some("v1"));
    }

    #[test]
    fn signed_refresh_uses_real_ml_dsa_verifier() {
        use crate::bundle::MlDsaBundleVerifier;
        use ml_dsa::signature::Signer;
        use ml_dsa::{B32, EncodedSigningKey, KeyGen, MlDsa87, Signature, SigningKey};

        const SK_LEN: usize = 4896;

        // Real keypair so we exercise the actual sig path.
        let mut seed = [0u8; 32];
        seed[0] = 42;
        let kp = MlDsa87::key_gen_internal(&B32::from(seed));
        let pk = kp.verifying_key().encode().to_vec();
        let sk_bytes = kp.signing_key().encode().to_vec();

        let metadata = BundleMetadata {
            version: "v1".to_string(),
            issuer: "trust-bridge".to_string(),
            issued_at_secs: 1_000,
            expires_at_secs: 2_000,
            tenant_id: "t".to_string(),
        };
        let payload = PolicyBundlePayload {
            revocations: vec![snap("c1", SnapshotStatus::Revoked, 1_000)],
        };
        let hash = payload_hash(&metadata, &payload).unwrap();
        let sk_arr: &[u8; SK_LEN] = sk_bytes.as_slice().try_into().unwrap();
        let sk_encoded = EncodedSigningKey::<MlDsa87>::from(*sk_arr);
        let sk = SigningKey::<MlDsa87>::decode(&sk_encoded);
        let sig: Signature<MlDsa87> = sk.sign(&hash);
        let bundle = SignedPolicyBundle {
            metadata,
            payload,
            signature: SignatureEnvelope {
                algorithm: "ml-dsa-87".to_string(),
                public_key_id: "real-anchor".to_string(),
                bytes_hex: hex::encode(sig.encode()),
            },
        };
        let verifier = MlDsaBundleVerifier::new().with_anchor("real-anchor", pk);
        let mut cache = LocalTrustCache::new(thresholds(60, 120)).unwrap();
        cache
            .record_signed_refresh(&bundle, &verifier, Some("t"), 1_500)
            .unwrap();
        assert_eq!(cache.revocation_status("c1"), SnapshotStatus::Revoked);
    }
}
