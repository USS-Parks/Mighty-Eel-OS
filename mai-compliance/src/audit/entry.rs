//! Compliance audit entry schema.
//!
//! [`AuditEntry`] is the unit of evidence the compliance audit log
//! produces: one row per policy decision, hash-chained to the previous
//! row via [`AuditEntry::previous_hash`] and optionally
//! signed periodically by the chain manager (see [`super::chain`]).
//!
//! (audit correlation, Appendix A §A.9) is embedded as
//! [`CorrelationFields`]: the metadata bridge between an OpenBao
//! credential event and the Lamprey decision it authorised. The shape
//! mirrors the JSON example in §A.9 verbatim so the cloud audit store
//! can ingest these entries without translation.
//!
//! ## Privacy invariants
//!
//! - `request_hash` is BLAKE3 over a **masked** request — PHI/PII
//!   stripped, ITAR/EAR substrings dropped, OCAP-flagged spans
//!   replaced with `<redacted>`. The raw request never appears in the
//!   audit log; the hash is only useful for correlation against the
//!   encrypted request store (separate subsystem).
//! - `correlation.subject_hash` is the HMAC pseudonym
//!   ([`crate::subject_hash::hmac_subject`]); raw subject ids never
//!   appear here.
//! - `routing_reason` is a stable rule identifier (e.g.
//!   `"itar.non_us_person"`), never the matched text.

use std::collections::BTreeMap;

use blake3::Hasher;
use serde::{Deserialize, Serialize};

use crate::policy::composer::{
    AggregateDecision, ComplianceFlag, ComplianceReason, Destination, ModuleId,
};

/// Width of the BLAKE3 chain link hash, in bytes.
pub const CHAIN_HASH_LEN: usize = 32;

/// Width of an ML-DSA-87 signature, in bytes. Surfaced here so
/// downstream consumers can size buffers without depending on the
/// `ml-dsa` crate directly.
pub const SIGNATURE_LEN: usize = 4627;

/// Routing decision recorded on an audit entry.
///
/// Mirrors [`Destination`] but stored as its own enum so the wire
/// format can grow new outcomes (`escalate`, `held`, …) without
/// reshaping the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingDecision {
    /// Request was allowed to proceed to the cloud route.
    Allow,
    /// Request was forced to the local appliance.
    LocalOnly,
    /// Request was held pending human review.
    Quarantine,
    /// Request was refused entirely.
    Deny,
}

impl RoutingDecision {
    /// Wire-format identifier (matches the serde tag).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::LocalOnly => "local_only_allowed",
            Self::Quarantine => "quarantine",
            Self::Deny => "deny",
        }
    }

    /// Derive the audit-side outcome from a composer
    /// [`AggregateDecision`]. Maps the (`allowed`, `route`) pair to
    /// the four-outcome audit vocabulary.
    pub fn from_aggregate(decision: &AggregateDecision) -> Self {
        match (decision.allowed, decision.route) {
            // An explicit Cloud route from an allowed request.
            (true, Some(Destination::Cloud)) => Self::Allow,
            // A Local route, OR no module vetted the request (route `None`): fail
            // closed to local-only. An unvetted request must never be recorded (or
            // routed) as cloud-eligible (audit G1) — previously `None` collapsed
            // onto `Allow`, mislabelling a disabled-module egress as permitted.
            (true, Some(Destination::Local) | None) => Self::LocalOnly,
            // Quarantine implies "held for review" regardless of the
            // allowed flag — the composer always sets allowed=false
            // when route=Quarantine today, but the audit vocabulary
            // collapses both sides cleanly onto Quarantine.
            (_, Some(Destination::Quarantine)) => Self::Quarantine,
            (false, _) => Self::Deny,
        }
    }
}

/// One rule that contributed to the recorded decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleMatch {
    /// Module that surfaced the rule.
    pub module: ModuleId,
    /// Stable rule identifier (e.g. `"ocap.cultural.elder"`). `None`
    /// means the underlying module did not surface one.
    pub rule: Option<String>,
    /// Human-readable summary.
    pub summary: String,
}

impl RuleMatch {
    /// Build from a composer-level [`ComplianceReason`].
    pub fn from_reason(reason: &ComplianceReason) -> Self {
        Self {
            module: reason.module,
            rule: reason.rule.clone(),
            summary: reason.summary.clone(),
        }
    }
}

/// correlation fields (Appendix A §A.9).
///
/// These bridge a credential event in the cloud trust system to a
/// Lamprey decision recorded locally. The JSON shape matches the cloud
/// trust system's correlation-event schema exactly so the cloud store
/// can deserialise without a translation layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrelationFields {
    /// OpenBao credential event that authorised the request. `None`
    /// when the decision originated from a local-only flow (no claim
    /// exchange happened).
    pub credential_event_id: Option<String>,
    /// Unique id assigned by the audit chain when the entry is
    /// recorded. Stable across replay.
    pub lamprey_decision_id: String,
    /// Original MAI request id from
    /// [`crate::policy::bundle::RequestMetadata`].
    pub mai_request_id: String,
    /// Tenant the decision belongs to.
    pub tenant: String,
    /// HMAC pseudonym of the subject id. MUST begin with `"hmac:"`.
    pub subject_hash: String,
    /// Service identity that ran the decision (e.g.
    /// `"lamprey-router"`).
    pub service_identity: Option<String>,
    /// Active policy bundle version at decision time.
    pub policy_version: String,
    /// Trust bundle version the subject's claim was issued against.
    pub trust_bundle_version: String,
    /// Recorded decision verb (mirror of [`AuditEntry::decision`] in
    /// the cloud schema). Stored here so the correlation event is
    /// self-contained when synced as metadata-only.
    pub decision: RoutingDecision,
}

impl CorrelationFields {
    /// True when no `credential_event_id` was supplied — used by
    /// dashboards to flag local-only / offline decisions.
    pub fn is_local_only(&self) -> bool {
        self.credential_event_id.is_none()
    }
}

/// A single tamper-evident audit log entry.
///
/// Hash chain semantics: `previous_hash` is the BLAKE3 of the
/// previous entry's canonical bytes (see
/// [`AuditEntry::canonical_bytes`]). The first entry in a chain uses
/// the all-zero `previous_hash`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Monotonic per-shard id (the chain assigns this).
    pub id: u64,
    /// Nanosecond-precision wall-clock time at record time.
    pub timestamp_unix_nanos: u64,
    /// BLAKE3 of the masked request bytes. The mask strips PHI/PII;
    /// the unmasked text never appears in the log.
    #[serde(with = "hex_array_32")]
    pub request_hash: [u8; CHAIN_HASH_LEN],
    /// Routing outcome.
    pub decision: RoutingDecision,
    /// Module ids whose decisions were folded into the outcome.
    pub modules_applied: Vec<ModuleId>,
    /// Per-rule audit trail.
    pub rules_fired: Vec<RuleMatch>,
    /// Composer-surfaced flags, recorded for dashboards.
    #[serde(default)]
    pub flags: Vec<ComplianceFlag>,
    /// Primary routing reason. Stable rule id when one was surfaced.
    pub routing_reason: String,
    /// HMAC pseudonym of the user profile / subject. Distinct from
    /// `correlation.subject_hash` only in that the latter is the
    /// wire field; here we keep both equal so the entry is
    /// self-contained.
    pub user_profile: String,
    /// correlation block.
    pub correlation: CorrelationFields,
    /// BLAKE3 of the previous entry's canonical bytes. Zero for
    /// chain head.
    #[serde(with = "hex_array_32")]
    pub previous_hash: [u8; CHAIN_HASH_LEN],
    /// Periodic ML-DSA signature over the running chain head. Set on
    /// every Nth entry by the chain manager; `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "hex_signature_opt")]
    pub signature: Option<Vec<u8>>,
}

impl AuditEntry {
    /// Compute the canonical bytes used as input to the chain hash
    /// and any periodic signature. Excludes the entry's own
    /// `signature` field (signing must be over the entry's content,
    /// not its signature) and re-serialises every field in a stable
    /// JSON shape.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let view = CanonicalView::from(self);
        serde_json::to_vec(&view).expect("canonical serialisation never fails")
    }

    /// BLAKE3 of [`Self::canonical_bytes`]; what the next entry
    /// records as its `previous_hash`.
    pub fn content_hash(&self) -> [u8; CHAIN_HASH_LEN] {
        let mut h = Hasher::new();
        h.update(&self.canonical_bytes());
        *h.finalize().as_bytes()
    }

    /// True when this entry is the head of a chain (its
    /// `previous_hash` is all zeros).
    pub fn is_chain_head(&self) -> bool {
        self.previous_hash == [0u8; CHAIN_HASH_LEN]
    }
}

#[derive(Serialize)]
struct CanonicalView<'a> {
    id: u64,
    timestamp_unix_nanos: u64,
    #[serde(with = "hex_array_32")]
    request_hash: [u8; CHAIN_HASH_LEN],
    decision: RoutingDecision,
    modules_applied: &'a [ModuleId],
    rules_fired: &'a [RuleMatch],
    flags: &'a [ComplianceFlag],
    routing_reason: &'a str,
    user_profile: &'a str,
    correlation: &'a CorrelationFields,
    #[serde(with = "hex_array_32")]
    previous_hash: [u8; CHAIN_HASH_LEN],
    // `signature` deliberately omitted.
}

impl<'a> From<&'a AuditEntry> for CanonicalView<'a> {
    fn from(e: &'a AuditEntry) -> Self {
        Self {
            id: e.id,
            timestamp_unix_nanos: e.timestamp_unix_nanos,
            request_hash: e.request_hash,
            decision: e.decision,
            modules_applied: &e.modules_applied,
            rules_fired: &e.rules_fired,
            flags: &e.flags,
            routing_reason: &e.routing_reason,
            user_profile: &e.user_profile,
            correlation: &e.correlation,
            previous_hash: e.previous_hash,
        }
    }
}

/// Helper: compute the masked-request hash. Callers must pass the
/// *already-masked* bytes — the audit module deliberately does not
/// own the masking step (it's domain-specific and lives in the
/// router / classifier output).
pub fn masked_request_hash(masked: &[u8]) -> [u8; CHAIN_HASH_LEN] {
    let mut h = Hasher::new();
    h.update(masked);
    *h.finalize().as_bytes()
}

/// Helper map alias used by query results that want O(log n)
/// id-based lookup. Re-exported here to keep the audit submodules
/// dependency-free.
pub type EntriesById = BTreeMap<u64, AuditEntry>;

mod hex_array_32 {
    use super::CHAIN_HASH_LEN;
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    pub fn serialize<S: Serializer>(bytes: &[u8; CHAIN_HASH_LEN], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; CHAIN_HASH_LEN], D::Error> {
        let s = String::deserialize(d)?;
        let bytes = hex::decode(&s).map_err(D::Error::custom)?;
        if bytes.len() != CHAIN_HASH_LEN {
            return Err(D::Error::custom(format!(
                "expected {CHAIN_HASH_LEN}-byte hex, got {}",
                bytes.len()
            )));
        }
        let mut out = [0u8; CHAIN_HASH_LEN];
        out.copy_from_slice(&bytes);
        Ok(out)
    }
}

mod hex_signature_opt {
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    // serde's `with = "..."` requires this exact signature
    // (`&Option<T>`), so we opt out of the `ref_option` lint here.
    #[allow(clippy::ref_option)]
    pub fn serialize<S: Serializer>(sig: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match sig {
            Some(bytes) => s.serialize_str(&hex::encode(bytes)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        let opt: Option<String> = Option::deserialize(d)?;
        match opt {
            None => Ok(None),
            Some(s) => hex::decode(&s).map(Some).map_err(D::Error::custom),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_correlation() -> CorrelationFields {
        CorrelationFields {
            credential_event_id: Some("cred_evt_123".into()),
            lamprey_decision_id: "dec_456".into(),
            mai_request_id: "req_789".into(),
            tenant: "tribal-health-demo".into(),
            subject_hash: "hmac:0123".into(),
            service_identity: Some("lamprey-router".into()),
            policy_version: "2026.05.22.001".into(),
            trust_bundle_version: "2026.05.22.001".into(),
            decision: RoutingDecision::LocalOnly,
        }
    }

    fn fake_entry(id: u64, prev: [u8; CHAIN_HASH_LEN]) -> AuditEntry {
        AuditEntry {
            id,
            timestamp_unix_nanos: 1_700_000_000_000_000_000,
            request_hash: masked_request_hash(b"masked request body"),
            decision: RoutingDecision::LocalOnly,
            modules_applied: vec![ModuleId::Ocap, ModuleId::Hipaa],
            rules_fired: vec![RuleMatch {
                module: ModuleId::Ocap,
                rule: Some("ocap.possession.local_only".into()),
                summary: "Tribal data must stay local".into(),
            }],
            flags: vec![],
            routing_reason: "ocap.possession.local_only".into(),
            user_profile: "hmac:abcd".into(),
            correlation: fake_correlation(),
            previous_hash: prev,
            signature: None,
        }
    }

    #[test]
    fn routing_decision_maps_from_aggregate() {
        let cloud_allow = AggregateDecision {
            allowed: true,
            route: Some(Destination::Cloud),
            flags: vec![],
            reasons: vec![],
            modules_applied: vec![ModuleId::Hipaa],
        };
        assert_eq!(
            RoutingDecision::from_aggregate(&cloud_allow),
            RoutingDecision::Allow
        );

        let local_allow = AggregateDecision {
            allowed: true,
            route: Some(Destination::Local),
            flags: vec![],
            reasons: vec![],
            modules_applied: vec![ModuleId::Ocap],
        };
        assert_eq!(
            RoutingDecision::from_aggregate(&local_allow),
            RoutingDecision::LocalOnly
        );

        let quarantine = AggregateDecision {
            allowed: false,
            route: Some(Destination::Quarantine),
            flags: vec![],
            reasons: vec![],
            modules_applied: vec![ModuleId::Ocap],
        };
        assert_eq!(
            RoutingDecision::from_aggregate(&quarantine),
            RoutingDecision::Quarantine
        );

        let deny = AggregateDecision {
            allowed: false,
            route: Some(Destination::Local),
            flags: vec![],
            reasons: vec![],
            modules_applied: vec![ModuleId::Itar],
        };
        assert_eq!(
            RoutingDecision::from_aggregate(&deny),
            RoutingDecision::Deny
        );
    }

    #[test]
    fn unvetted_none_route_fails_closed_to_local() {
        // Audit G1: an empty/unvetted decision set (no module ran -> route None)
        // must NOT map to Allow (cloud-eligible); it fails closed to LocalOnly.
        let unvetted = AggregateDecision {
            allowed: true,
            route: None,
            flags: vec![],
            reasons: vec![],
            modules_applied: vec![],
        };
        assert_eq!(
            RoutingDecision::from_aggregate(&unvetted),
            RoutingDecision::LocalOnly
        );
    }

    #[test]
    fn entry_roundtrips_through_json() {
        let entry = fake_entry(1, [0u8; CHAIN_HASH_LEN]);
        let json = serde_json::to_string(&entry).expect("serialise");
        let back: AuditEntry = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(entry, back);
    }

    #[test]
    fn canonical_bytes_exclude_signature() {
        let mut entry = fake_entry(1, [0u8; CHAIN_HASH_LEN]);
        let no_sig = entry.canonical_bytes();
        entry.signature = Some(vec![0xAB; SIGNATURE_LEN]);
        let with_sig = entry.canonical_bytes();
        assert_eq!(no_sig, with_sig);
    }

    #[test]
    fn content_hash_changes_when_field_tampered() {
        let original = fake_entry(1, [0u8; CHAIN_HASH_LEN]);
        let mut tampered = original.clone();
        tampered.routing_reason = "tampered".into();
        assert_ne!(original.content_hash(), tampered.content_hash());
    }

    #[test]
    fn chain_head_detection() {
        let head = fake_entry(1, [0u8; CHAIN_HASH_LEN]);
        let later = fake_entry(2, head.content_hash());
        assert!(head.is_chain_head());
        assert!(!later.is_chain_head());
    }

    #[test]
    fn correlation_is_local_only_when_no_credential_event() {
        let mut c = fake_correlation();
        c.credential_event_id = None;
        assert!(c.is_local_only());
    }

    #[test]
    fn rule_match_from_reason_preserves_fields() {
        let reason = ComplianceReason {
            module: ModuleId::Itar,
            rule: Some("itar.non_us_person".into()),
            summary: "Non-US person".into(),
        };
        let rm = RuleMatch::from_reason(&reason);
        assert_eq!(rm.module, ModuleId::Itar);
        assert_eq!(rm.rule.as_deref(), Some("itar.non_us_person"));
        assert_eq!(rm.summary, "Non-US person");
    }

    #[test]
    fn signature_roundtrips_when_present() {
        let mut entry = fake_entry(1, [0u8; CHAIN_HASH_LEN]);
        entry.signature = Some(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let json = serde_json::to_string(&entry).unwrap();
        let back: AuditEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.signature, entry.signature);
    }
}
