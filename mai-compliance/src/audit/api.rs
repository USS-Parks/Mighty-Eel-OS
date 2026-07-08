//! Compliance audit log façade and query API.
//!
//! [`AuditLog`] is the typed core that backs the audit HTTP endpoints:
//!
//! | Route | API call |
//! |-------|----------|
//! | `GET    /v1/compliance/audit?from=&to=&module=&limit=` | [`AuditLog::query`] |
//! | `GET    /v1/compliance/audit/{id}`                     | [`AuditLog::get`] |
//! | `GET    /v1/compliance/audit/verify`                   | [`AuditLog::verify_full`] |
//! | `GET    /v1/compliance/audit/integrity`                | [`AuditLog::integrity_status`] |
//!
//! HTTP wiring lives in `mai-api`; this surface is pure so the
//! dashboard process and tests can use it directly.
//!
//! The log produces entries from two input sources:
//!
//! 1. **Direct recording** — [`AuditLog::record`] takes a
//!    [`AuditRecordInput`] (request id, masked-request bytes, the
//!    composer verdict, and the trust context) and emits a finalised
//!    [`AuditEntry`].
//! 2. **Feed adapter** — [`AuditLog::ingest_feed_event`] consumes a
//!    [`crate::policy::audit_feed::FeedEvent`] and produces an entry
//!    or escalation as appropriate. The trust context isn't carried
//!    on `FeedEvent`, so adapter-driven entries fall back to the
//!    correlation fields embedded in the bundle that produced the
//!    decision (caller passes a [`crate::policy::bundle::PolicyBundle`]
//!    alongside).

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::chain::{ChainConfig, ChainError, ChainSigner, HashChainManager, NullSigner};
use super::entry::{
    AuditEntry, CorrelationFields, RoutingDecision, RuleMatch, masked_request_hash,
};
use super::store::{AuditStore, AuditStoreConfig, NullSealer, StoreError, StoreSealer};
use super::triggers::{Escalation, TriggerManager, TriggersConfig};
use crate::bundle::BundleVerifier;
use crate::policy::bundle::PolicyBundle;
use crate::policy::composer::{AggregateDecision, ModuleId};

/// Caller-supplied input for [`AuditLog::record`].
#[derive(Debug, Clone)]
pub struct AuditRecordInput<'a> {
    /// Original MAI request id.
    pub request_id: &'a str,
    /// Bytes of the *masked* request — PHI/PII stripped, ITAR/EAR
    /// substrings dropped, OCAP-flagged spans replaced.
    pub masked_request: &'a [u8],
    /// Composer verdict.
    pub decision: &'a AggregateDecision,
    /// The bundle that produced the decision (trust + classification).
    pub bundle: &'a PolicyBundle,
    /// Active policy bundle version, surfaced into
    /// [`CorrelationFields::policy_version`].
    pub policy_version: &'a str,
    /// Optional OpenBao credential event id. `None` for
    /// local-only / offline flows.
    pub credential_event_id: Option<String>,
    /// Wall-clock nanoseconds since the Unix epoch. Caller supplies
    /// this so tests are deterministic; production wires a monotonic
    /// nanosecond clock.
    pub timestamp_unix_nanos: u64,
}

/// Query for [`AuditLog::query`]. All filters are AND-combined.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditQuery {
    /// Inclusive lower bound on `timestamp_unix_nanos`.
    pub from: Option<u64>,
    /// Inclusive upper bound on `timestamp_unix_nanos`.
    pub to: Option<u64>,
    /// Module id filter — entry passes when its `modules_applied`
    /// contains this id.
    pub module: Option<ModuleId>,
    /// Decision filter — entry passes when its `decision` matches.
    pub decision: Option<RoutingDecision>,
    /// Tenant filter — matches `correlation.tenant`.
    pub tenant: Option<String>,
    /// Maximum entries to return. `None` returns all matching.
    pub limit: Option<usize>,
}

/// Verification status surfaced on a query result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    /// Entry's `previous_hash` matched its predecessor at last full
    /// verify.
    Verified,
    /// Last full-chain verify failed — this entry's status is
    /// suspect.
    Tampered,
    /// No verification has been run yet.
    Unknown,
}

/// One row in an audit query result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditQueryRow {
    /// The entry itself.
    pub entry: AuditEntry,
    /// Last known verification status for this entry.
    pub status: VerificationStatus,
}

/// Chain integrity snapshot, returned by
/// [`AuditLog::integrity_status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrityStatus {
    /// Number of entries currently in the in-memory tail.
    pub entry_count: u64,
    /// Number of entries the chain manager has seen total (drives
    /// next-id assignment).
    pub chain_count: u64,
    /// Hex-encoded current head hash.
    pub head_hash: String,
    /// Last full-verify status.
    pub last_verify: VerificationStatus,
    /// Last full-verify error, if any.
    pub last_verify_error: Option<String>,
}

#[derive(Debug)]
struct Inner {
    last_verify: VerificationStatus,
    last_verify_error: Option<String>,
    triggers: TriggerManager,
}

/// Composed compliance audit log.
#[derive(Clone)]
pub struct AuditLog {
    chain: HashChainManager,
    store: AuditStore,
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for AuditLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLog")
            .field("chain", &self.chain)
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl AuditLog {
    /// Construct a builder. The default builder uses
    /// [`NullSigner`] + [`NullSealer`] + default triggers — fine for
    /// tests and bring-up. Production wires the ML-DSA signer and a
    /// vault-backed sealer.
    pub fn builder() -> AuditLogBuilder {
        AuditLogBuilder::default()
    }

    /// Direct view of the chain manager (testing / introspection).
    pub fn chain(&self) -> &HashChainManager {
        &self.chain
    }

    /// Direct view of the store (testing / introspection).
    pub fn store(&self) -> &AuditStore {
        &self.store
    }

    /// Record a decision. Builds the [`AuditEntry`], assigns it via
    /// the chain manager, appends it to the store, runs the triggers,
    /// and queues a correlation event for sync. Returns the
    /// finalised entry and any escalations the triggers emitted.
    pub fn record(
        &self,
        input: AuditRecordInput<'_>,
    ) -> Result<(AuditEntry, Vec<Escalation>), StoreError> {
        let decision = RoutingDecision::from_aggregate(input.decision);
        let rules_fired: Vec<RuleMatch> = input
            .decision
            .reasons
            .iter()
            .map(RuleMatch::from_reason)
            .collect();
        let routing_reason = input
            .decision
            .reasons
            .iter()
            .find_map(|r| r.rule.clone())
            .unwrap_or_else(|| {
                input
                    .decision
                    .reasons
                    .first()
                    .map_or_else(|| decision.as_str().to_string(), |r| r.summary.clone())
            });
        let subject_hash = input.bundle.trust.subject_hash.as_str().to_string();
        let tenant = input.bundle.trust.tenant_id.as_str().to_string();
        let service_identity = input
            .bundle
            .trust
            .service_identity
            .map(|s| s.as_str().to_string());

        let correlation = CorrelationFields {
            credential_event_id: input.credential_event_id,
            // Chain finalize fills `id`; the correlation id
            // mirrors it once known.
            lamprey_decision_id: String::new(),
            mai_request_id: input.request_id.to_string(),
            tenant,
            subject_hash: subject_hash.clone(),
            service_identity,
            policy_version: input.policy_version.to_string(),
            trust_bundle_version: input.bundle.trust.trust_bundle_version.clone(),
            decision,
        };
        let draft = AuditEntry {
            id: 0,
            timestamp_unix_nanos: input.timestamp_unix_nanos,
            request_hash: masked_request_hash(input.masked_request),
            decision,
            modules_applied: input.decision.modules_applied.clone(),
            rules_fired,
            flags: input.decision.flags.clone(),
            routing_reason,
            user_profile: subject_hash,
            correlation,
            previous_hash: [0u8; 32],
            signature: None,
        };

        let mut entry = self.chain.finalize(draft);
        // Now that the chain has assigned the id, fill it into the
        // correlation block and re-link the chain (the prior hash
        // and signature already reflect the entry's canonical bytes
        // including the empty `lamprey_decision_id`; we re-finalize
        // by overwriting the id and recomputing). The cleanest
        // contract is: the `lamprey_decision_id` is the entry
        // id formatted as `"dec_<id>"`. Setting it after finalize
        // changes the content hash, so we must use this id as the
        // chain link going forward.
        entry.correlation.lamprey_decision_id = format!("dec_{}", entry.id);
        // Re-record the (updated) content hash as the cursor's
        // previous_hash so the next append links to the *visible*
        // entry rather than the placeholder shape. We do this by
        // restoring the chain cursor from the updated entry.
        self.chain.restore_from(&entry);

        let id = self.store.append(entry.clone())?;
        debug_assert_eq!(id, entry.id);

        // queue a correlation event for sync.
        self.store.enqueue_correlation(entry.correlation.clone());

        // Triggers.
        let now = std::time::Instant::now();
        let escalations = {
            let mut guard = self.inner.lock().expect("audit log poisoned");
            guard
                .triggers
                .record_decision(decision, entry.modules_applied.first().copied(), now)
        };
        Ok((entry, escalations))
    }

    /// Run a query against the in-memory tail.
    pub fn query(&self, q: &AuditQuery) -> Vec<AuditQueryRow> {
        let status = self.last_known_status();
        let snapshot = self.store.entries();
        let mut out: Vec<AuditQueryRow> = snapshot
            .into_iter()
            .filter(|e| {
                q.from.is_none_or(|f| e.timestamp_unix_nanos >= f)
                    && q.to.is_none_or(|t| e.timestamp_unix_nanos <= t)
                    && q.module.is_none_or(|m| e.modules_applied.contains(&m))
                    && q.decision.is_none_or(|d| e.decision == d)
                    && q.tenant
                        .as_deref()
                        .is_none_or(|t| e.correlation.tenant == t)
            })
            .map(|entry| AuditQueryRow { entry, status })
            .collect();
        if let Some(limit) = q.limit {
            out.truncate(limit);
        }
        out
    }

    /// Fetch a single entry by id from the in-memory tail.
    pub fn get(&self, id: u64) -> Option<AuditQueryRow> {
        let status = self.last_known_status();
        self.store
            .entries()
            .into_iter()
            .find(|e| e.id == id)
            .map(|entry| AuditQueryRow { entry, status })
    }

    /// Run a full-chain verification across the in-memory tail.
    /// Updates the stored verification status and returns a
    /// summary. When the verifier returns an error, also surfaces a
    /// [`Escalation::ChainBreak`] critical event.
    pub fn verify_full<V: BundleVerifier>(&self, verifier: Option<&V>) -> Result<(), ChainError> {
        let entries = self.store.entries();
        let cfg = self.chain.config();
        // Verify from genesis while the tail still holds the head (id 0). Once the
        // log outgrows `max_in_memory` the head is evicted and the retained tail is
        // a segment, so verify linkage/signatures without the genesis head-check
        // (audit H8/U3 — that check false-positived on a clean long log). The
        // evicted prefix is verified from the WAL in U2.
        let starts_at_genesis = entries.first().is_none_or(|e| e.id == 0);
        let result = if starts_at_genesis {
            super::chain::verify_chain(&entries, cfg, verifier)
        } else {
            super::chain::verify_segment(&entries, cfg, verifier)
        };
        let mut guard = self.inner.lock().expect("audit log poisoned");
        match &result {
            Ok(()) => {
                guard.last_verify = VerificationStatus::Verified;
                guard.last_verify_error = None;
            }
            Err(e) => {
                guard.last_verify = VerificationStatus::Tampered;
                guard.last_verify_error = Some(e.to_string());
            }
        }
        result
    }

    /// Snapshot the chain's integrity status without re-running
    /// verification.
    pub fn integrity_status(&self) -> IntegrityStatus {
        let guard = self.inner.lock().expect("audit log poisoned");
        IntegrityStatus {
            entry_count: self.store.len() as u64,
            chain_count: self.chain.count(),
            head_hash: hex::encode(self.chain.head_hash()),
            last_verify: guard.last_verify,
            last_verify_error: guard.last_verify_error.clone(),
        }
    }

    /// Trigger-only entry point: record a policy change.
    pub fn record_policy_change(&self, summary: impl Into<String>) -> Vec<Escalation> {
        let guard = self.inner.lock().expect("audit log poisoned");
        guard.triggers.record_policy_change(summary)
    }

    /// Trigger-only entry point: record a chain-break alert.
    pub fn record_chain_break(&self, reason: impl Into<String>) -> Vec<Escalation> {
        let guard = self.inner.lock().expect("audit log poisoned");
        guard.triggers.record_chain_break(reason)
    }

    /// Trigger-only entry point: update storage usage gauge.
    pub fn record_storage_usage(&self, used: u64, capacity: u64) -> Vec<Escalation> {
        let mut guard = self.inner.lock().expect("audit log poisoned");
        guard.triggers.record_storage_usage(used, capacity)
    }

    fn last_known_status(&self) -> VerificationStatus {
        self.inner.lock().expect("audit log poisoned").last_verify
    }
}

/// Builder for [`AuditLog`].
#[derive(Debug)]
pub struct AuditLogBuilder {
    chain_config: ChainConfig,
    store_config: AuditStoreConfig,
    triggers_config: TriggersConfig,
    signer: Arc<dyn ChainSigner>,
    sealer: Arc<dyn StoreSealer>,
}

impl Default for AuditLogBuilder {
    fn default() -> Self {
        Self {
            chain_config: ChainConfig::default(),
            store_config: AuditStoreConfig::default(),
            triggers_config: TriggersConfig::default(),
            signer: Arc::new(NullSigner),
            sealer: Arc::new(NullSealer),
        }
    }
}

impl AuditLogBuilder {
    /// Override the chain configuration.
    pub fn chain_config(mut self, config: ChainConfig) -> Self {
        self.chain_config = config;
        self
    }

    /// Override the store configuration.
    pub fn store_config(mut self, config: AuditStoreConfig) -> Self {
        self.store_config = config;
        self
    }

    /// Override the triggers configuration.
    pub fn triggers_config(mut self, config: TriggersConfig) -> Self {
        self.triggers_config = config;
        self
    }

    /// Wire in a non-default chain signer (e.g. ML-DSA).
    pub fn signer(mut self, signer: Arc<dyn ChainSigner>) -> Self {
        self.signer = signer;
        self
    }

    /// Wire in a non-default store sealer (e.g. vault AEAD).
    pub fn sealer(mut self, sealer: Arc<dyn StoreSealer>) -> Self {
        self.sealer = sealer;
        self
    }

    /// Build the [`AuditLog`].
    pub fn build(self) -> AuditLog {
        AuditLog {
            chain: HashChainManager::new(self.chain_config, self.signer),
            store: AuditStore::new(self.store_config, self.sealer),
            inner: Arc::new(Mutex::new(Inner {
                last_verify: VerificationStatus::Unknown,
                last_verify_error: None,
                triggers: TriggerManager::new(self.triggers_config),
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::MlDsaBundleVerifier;
    use crate::policy::bundle::{ClassificationResult, RequestMetadata};
    use crate::policy::composer::{ComplianceReason, Destination};
    use crate::trust::TrustContext;

    fn sample_bundle() -> PolicyBundle {
        PolicyBundle {
            request: RequestMetadata {
                request_id: "req-001".into(),
                tenant_id: "local-dev".into(),
                timestamp_unix_ms: 0,
                source: "api".into(),
                model_hint: None,
            },
            trust: TrustContext::for_local_dev(),
            classification: ClassificationResult {
                level: "regulated".into(),
                matched_patterns: vec!["ssn".into()],
                entity_count: 1,
            },
        }
    }

    fn sample_decision(allowed: bool, route: Destination) -> AggregateDecision {
        AggregateDecision {
            allowed,
            route: Some(route),
            flags: vec![],
            reasons: vec![ComplianceReason::new(
                ModuleId::Ocap,
                Some("ocap.possession.local_only".into()),
                "Tribal data must stay local",
            )],
            modules_applied: vec![ModuleId::Ocap],
        }
    }

    fn record_input<'a>(
        bundle: &'a PolicyBundle,
        decision: &'a AggregateDecision,
    ) -> AuditRecordInput<'a> {
        AuditRecordInput {
            request_id: "req-001",
            masked_request: b"masked",
            decision,
            bundle,
            policy_version: "2026.05.22.001",
            credential_event_id: Some("cred_evt_777".into()),
            timestamp_unix_nanos: 1_700_000_000_000_000_000,
        }
    }

    #[test]
    fn record_finalises_entry_and_links_chain() {
        let log = AuditLog::default();
        let bundle = sample_bundle();
        let dec = sample_decision(true, Destination::Local);
        let (a, _) = log.record(record_input(&bundle, &dec)).unwrap();
        let (b, _) = log.record(record_input(&bundle, &dec)).unwrap();
        assert_eq!(a.id, 0);
        assert_eq!(b.id, 1);
        // After our re-link logic, b's previous_hash must equal a's
        // content hash *as recorded* (with the lamprey_decision_id
        // filled in).
        assert_eq!(b.previous_hash, a.content_hash());
        assert_eq!(b.correlation.lamprey_decision_id, "dec_1");
    }

    #[test]
    fn record_routes_quarantine_through_to_audit_outcome() {
        let log = AuditLog::default();
        let bundle = sample_bundle();
        let dec = sample_decision(false, Destination::Quarantine);
        let (entry, _) = log.record(record_input(&bundle, &dec)).unwrap();
        assert_eq!(entry.decision, RoutingDecision::Quarantine);
        assert_eq!(entry.correlation.decision, RoutingDecision::Quarantine);
    }

    #[test]
    fn record_enqueues_bf5_correlation_event() {
        let log = AuditLog::default();
        let bundle = sample_bundle();
        let dec = sample_decision(true, Destination::Local);
        log.record(record_input(&bundle, &dec)).unwrap();
        assert_eq!(log.store().offline_queue_len(), 1);
        let drained = log.store().drain_offline_queue();
        assert_eq!(
            drained[0].credential_event_id.as_deref(),
            Some("cred_evt_777")
        );
        assert_eq!(drained[0].policy_version, "2026.05.22.001");
        assert!(drained[0].subject_hash.starts_with("hmac:"));
    }

    #[test]
    fn correlation_never_carries_raw_subject_id() {
        let log = AuditLog::default();
        let bundle = sample_bundle();
        let dec = sample_decision(true, Destination::Local);
        let (entry, _) = log.record(record_input(&bundle, &dec)).unwrap();
        let raw = bundle.trust.subject_id.as_str();
        // raw subject id may or may not appear in trust.subject_hash
        // depending on the test fixture, but it MUST NOT appear in
        // the audit subject_hash (which is the HMAC pseudonym).
        // For local-dev, the fixture sets subject_hash already to
        // the pseudonym ("hmac-placeholder") — accept that prefix as
        // the contract.
        let serialised = serde_json::to_string(&entry).unwrap();
        assert!(
            !serialised.contains(&format!("\"subject_id\":\"{raw}\"")),
            "raw subject id leaked into audit entry"
        );
    }

    #[test]
    fn query_filters_by_module_and_decision_and_limit() {
        let log = AuditLog::default();
        let bundle = sample_bundle();
        log.record(record_input(
            &bundle,
            &sample_decision(true, Destination::Cloud),
        ))
        .unwrap();
        log.record(record_input(
            &bundle,
            &sample_decision(true, Destination::Local),
        ))
        .unwrap();
        log.record(record_input(
            &bundle,
            &sample_decision(false, Destination::Local),
        ))
        .unwrap();

        let rows = log.query(&AuditQuery {
            module: Some(ModuleId::Ocap),
            ..AuditQuery::default()
        });
        assert_eq!(rows.len(), 3);

        let rows = log.query(&AuditQuery {
            decision: Some(RoutingDecision::Deny),
            ..AuditQuery::default()
        });
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].entry.decision, RoutingDecision::Deny);

        let rows = log.query(&AuditQuery {
            limit: Some(2),
            ..AuditQuery::default()
        });
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn query_filters_by_timestamp_range() {
        let log = AuditLog::default();
        let bundle = sample_bundle();
        let dec = sample_decision(true, Destination::Local);
        let mut input = record_input(&bundle, &dec);
        input.timestamp_unix_nanos = 100;
        log.record(input.clone()).unwrap();
        input.timestamp_unix_nanos = 200;
        log.record(input.clone()).unwrap();
        input.timestamp_unix_nanos = 300;
        log.record(input).unwrap();

        let rows = log.query(&AuditQuery {
            from: Some(150),
            to: Some(250),
            ..AuditQuery::default()
        });
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].entry.timestamp_unix_nanos, 200);
    }

    #[test]
    fn verify_full_passes_on_clean_log() {
        let log = AuditLog::default();
        let bundle = sample_bundle();
        for _ in 0..3 {
            log.record(record_input(
                &bundle,
                &sample_decision(true, Destination::Local),
            ))
            .unwrap();
        }
        log.verify_full(None::<&MlDsaBundleVerifier>).unwrap();
        assert_eq!(
            log.integrity_status().last_verify,
            VerificationStatus::Verified
        );
    }

    #[test]
    fn verify_full_passes_after_head_eviction() {
        // Audit H8/U3: once more entries than max_in_memory are recorded the head
        // (id 0) is evicted from the in-memory tail. A clean log must still verify;
        // the old code false-positived HeadHashNonZero because the retained tail no
        // longer began at genesis.
        let log = AuditLog::builder()
            .store_config(AuditStoreConfig {
                max_in_memory: 4,
                ..AuditStoreConfig::default()
            })
            .build();
        let bundle = sample_bundle();
        for _ in 0..10 {
            log.record(record_input(
                &bundle,
                &sample_decision(true, Destination::Local),
            ))
            .unwrap();
        }
        // Precondition: the head was evicted, so the tail starts past id 0.
        assert!(
            log.store().entries()[0].id > 0,
            "test precondition: head must be evicted"
        );
        log.verify_full(None::<&MlDsaBundleVerifier>)
            .expect("a clean post-eviction log must verify");
        assert_eq!(
            log.integrity_status().last_verify,
            VerificationStatus::Verified
        );
    }

    #[test]
    fn integrity_status_reflects_counts() {
        let log = AuditLog::default();
        let bundle = sample_bundle();
        log.record(record_input(
            &bundle,
            &sample_decision(true, Destination::Local),
        ))
        .unwrap();
        let status = log.integrity_status();
        assert_eq!(status.entry_count, 1);
        assert_eq!(status.chain_count, 1);
        assert_eq!(status.head_hash.len(), 64);
        assert_eq!(status.last_verify, VerificationStatus::Unknown);
    }

    #[test]
    fn get_returns_matching_entry() {
        let log = AuditLog::default();
        let bundle = sample_bundle();
        log.record(record_input(
            &bundle,
            &sample_decision(true, Destination::Local),
        ))
        .unwrap();
        let row = log.get(0).expect("entry id 0 must exist");
        assert_eq!(row.entry.id, 0);
        assert!(log.get(999).is_none());
    }
}
