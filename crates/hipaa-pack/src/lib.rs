//! HIPAA policy pack v1 (D4).
//!
//! The v1 vertical slice, wired end-to-end: a **PHI request is governed** (detected
//! by the mai-compliance `PhiDetector`, pinned to **local-only** routing so it never
//! egresses to a cloud provider), its metadata-only receipt lands in the **WSF
//! ledger** (`wsf-ledger`, W4), and an **audit-ready HIPAA evidence pack** is
//! exported that maps those receipts to the HIPAA **§164.312 technical safeguards**
//! and **verifies off-host** with the public key alone.
//!
//! The pack reuses W4's signed [`EvidencePack`] verbatim — the HIPAA layer adds the
//! control mapping, which is a **deterministic function of the signed receipts**, so
//! a regulator re-derives it from the evidence rather than trusting it. Nothing here
//! is mocked: PHI detection is the real detector, the pack signature is real
//! ML-DSA-87. (The live seal/gateway path is D5; this proves the HIPAA mapping.)

use mai_compliance::PhiDetector;
use serde::{Deserialize, Serialize};
use wsf_ledger::{EvidencePack, Ledger, LedgerEntry, LedgerError, verify_pack};

use fabric_crypto::Verifier;
use fabric_proof::{GENESIS_HASH, chain_link};

/// A HIPAA §164.312 technical safeguard this pack evidences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HipaaControl {
    /// §164.312(a) — access control (only authorized tokens act).
    AccessControl,
    /// §164.312(b) — audit controls (a tamper-evident record of every action).
    AuditControls,
    /// §164.312(c) — integrity (the record cannot be altered undetectably).
    Integrity,
    /// §164.312(e) — transmission security (PHI is not sent to a third party).
    TransmissionSecurity,
}

impl HipaaControl {
    /// The four technical safeguards, in citation order.
    pub const ALL: [HipaaControl; 4] = [
        HipaaControl::AccessControl,
        HipaaControl::AuditControls,
        HipaaControl::Integrity,
        HipaaControl::TransmissionSecurity,
    ];

    /// The CFR citation.
    #[must_use]
    pub fn citation(self) -> &'static str {
        match self {
            HipaaControl::AccessControl => "45 CFR §164.312(a)(1)",
            HipaaControl::AuditControls => "45 CFR §164.312(b)",
            HipaaControl::Integrity => "45 CFR §164.312(c)(1)",
            HipaaControl::TransmissionSecurity => "45 CFR §164.312(e)(1)",
        }
    }

    /// A short human title.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            HipaaControl::AccessControl => "Access control",
            HipaaControl::AuditControls => "Audit controls",
            HipaaControl::Integrity => "Integrity",
            HipaaControl::TransmissionSecurity => "Transmission security",
        }
    }
}

/// Where a request may route under the HIPAA rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhiRoute {
    LocalOnly,
    CloudAllowed,
}

/// The outcome of governing a request under the HIPAA rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhiGovernance {
    /// Whether PHI was detected in the request.
    pub phi_detected: bool,
    /// The route the rule pins the request to.
    pub route: PhiRoute,
    /// Whether the cloud destination was denied (true whenever PHI is present).
    pub cloud_denied: bool,
}

/// The HIPAA v1 pack rule.
pub struct HipaaPack;

impl HipaaPack {
    /// The pack id (stamped on every exported evidence pack).
    pub const ID: &'static str = "HIPAA-v1";

    /// Govern a request under the HIPAA rule: **PHI present → local-only, cloud
    /// denied**; otherwise the cloud route is permitted. This is the enforceable
    /// half of the pack (the §164.312(e) transmission-security safeguard).
    #[must_use]
    pub fn govern(detector: &PhiDetector, text: &str) -> PhiGovernance {
        let phi_detected = detector.scan(text).has_any();
        PhiGovernance {
            phi_detected,
            route: if phi_detected {
                PhiRoute::LocalOnly
            } else {
                PhiRoute::CloudAllowed
            },
            cloud_denied: phi_detected,
        }
    }
}

/// Evidence for one HIPAA control: whether it is satisfied and which receipt
/// sequence numbers evidence it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlEvidence {
    pub control: HipaaControl,
    pub citation: String,
    pub title: String,
    pub satisfied: bool,
    pub evidence_seqs: Vec<u64>,
}

impl ControlEvidence {
    fn new(control: HipaaControl, satisfied: bool, evidence_seqs: Vec<u64>) -> Self {
        Self {
            control,
            citation: control.citation().to_string(),
            title: control.title().to_string(),
            satisfied,
            evidence_seqs,
        }
    }
}

/// A HIPAA-mapped evidence pack: the signed W4 [`EvidencePack`] plus the §164.312
/// control mapping derived from its receipts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HipaaEvidencePack {
    /// Always [`HipaaPack::ID`].
    pub pack_id: String,
    /// One entry per §164.312 technical safeguard.
    pub controls: Vec<ControlEvidence>,
    /// The underlying signed receipt pack (off-host verifiable on its own).
    pub pack: EvidencePack,
}

fn field<'a>(entry: &'a LedgerEntry, key: &str) -> Option<&'a str> {
    entry.receipt.get(key).and_then(serde_json::Value::as_str)
}

/// PHI/sensitive data that was pinned to a local route — the transmission-security
/// evidence (the payload was never handed to a third party).
fn is_sensitive_local(entry: &LedgerEntry) -> bool {
    field(entry, "route") == Some("local_only")
        && matches!(
            field(entry, "classification"),
            Some("restricted" | "controlled" | "secret" | "phi")
        )
}

/// Re-derive whether the entry chain links from genesis — the Integrity check,
/// recomputed from the (signed) entries so a verifier does not trust a stored flag.
fn chain_ok(entries: &[LedgerEntry]) -> bool {
    let mut prev = GENESIS_HASH;
    for e in entries {
        let (Some(ph), Some(eh)) = (decode32(&e.previous_hash), decode32(&e.entry_hash)) else {
            return false;
        };
        if ph != prev {
            return false;
        }
        prev = chain_link(&prev, &eh);
    }
    true
}

fn decode32(s: &str) -> Option<[u8; 32]> {
    hex::decode(s).ok()?.try_into().ok()
}

/// Map a ledger's receipts to the HIPAA §164.312 technical safeguards they evidence.
#[must_use]
pub fn map_controls(entries: &[LedgerEntry]) -> Vec<ControlEvidence> {
    let has_any = !entries.is_empty();
    let integrity_ok = has_any && chain_ok(entries);

    let access: Vec<u64> = entries
        .iter()
        .filter(|e| field(e, "token_id").is_some())
        .map(|e| e.seq)
        .collect();
    let audit: Vec<u64> = entries.iter().map(|e| e.seq).collect();
    let integrity: Vec<u64> = if integrity_ok {
        entries.iter().map(|e| e.seq).collect()
    } else {
        Vec::new()
    };
    let transmission: Vec<u64> = entries
        .iter()
        .filter(|e| is_sensitive_local(e))
        .map(|e| e.seq)
        .collect();

    vec![
        ControlEvidence::new(HipaaControl::AccessControl, !access.is_empty(), access),
        ControlEvidence::new(HipaaControl::AuditControls, has_any, audit),
        ControlEvidence::new(HipaaControl::Integrity, integrity_ok, integrity),
        ControlEvidence::new(
            HipaaControl::TransmissionSecurity,
            !transmission.is_empty(),
            transmission,
        ),
    ]
}

/// Export a HIPAA evidence pack from a ledger: the signed W4 pack + the §164.312
/// control mapping.
///
/// # Errors
/// Propagates [`LedgerError`] from the underlying pack export.
pub fn export_hipaa_pack(
    ledger: &Ledger,
    generated_at: impl Into<String>,
) -> Result<HipaaEvidencePack, LedgerError> {
    let controls = map_controls(ledger.entries());
    let pack = ledger.export_pack(generated_at)?;
    Ok(HipaaEvidencePack {
        pack_id: HipaaPack::ID.to_string(),
        controls,
        pack,
    })
}

/// Verify a HIPAA evidence pack off-host: the underlying signature verifies **and**
/// the control mapping re-derives identically from the signed receipts.
#[must_use]
pub fn verify_hipaa_pack(
    hp: &HipaaEvidencePack,
    verifier: &dyn Verifier,
    public_key: &[u8],
) -> bool {
    verify_pack(&hp.pack, verifier, public_key)
        && map_controls(&hp.pack.entries) == hp.controls
        && hp.pack_id == HipaaPack::ID
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_crypto::Signer;
    use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
    use std::sync::Arc;

    fn ledger() -> Ledger {
        Ledger::new(Arc::new(RustCryptoMlDsa87::generate("hipaa-key").unwrap()))
    }

    fn phi_receipt() -> serde_json::Value {
        serde_json::json!({
            "token_id": "tok_1",
            "classification": "controlled",
            "route": "local_only",
            "policy": "deny_cloud",
            "provider": "local",
            "decision": "allow",
        })
    }

    #[test]
    fn phi_request_is_governed_local_only() {
        let detector = PhiDetector::baseline();
        let gov = HipaaPack::govern(&detector, "patient SSN 123-45-6789 admitted today");
        assert!(gov.phi_detected);
        assert_eq!(gov.route, PhiRoute::LocalOnly);
        assert!(gov.cloud_denied, "PHI is never routed to cloud");
    }

    #[test]
    fn a_benign_request_may_route_cloud() {
        let detector = PhiDetector::baseline();
        let gov = HipaaPack::govern(&detector, "what is the capital of France?");
        assert!(!gov.phi_detected);
        assert_eq!(gov.route, PhiRoute::CloudAllowed);
        assert!(!gov.cloud_denied);
    }

    #[test]
    fn phi_request_produces_an_auditable_hipaa_pack() {
        // The end-to-end gate: govern a PHI request, receipt it, export a HIPAA pack,
        // and prove every §164.312 safeguard is evidenced + the pack verifies off-host.
        let detector = PhiDetector::baseline();
        let gov = HipaaPack::govern(&detector, "patient SSN 123-45-6789");
        assert!(gov.phi_detected && gov.route == PhiRoute::LocalOnly);

        let mut l = ledger();
        l.ingest("aog-gateway", phi_receipt()).unwrap();

        let hp = export_hipaa_pack(&l, "2026-07-04T00:00:00Z").unwrap();
        assert_eq!(hp.pack_id, "HIPAA-v1");
        assert_eq!(hp.controls.len(), 4);
        for c in &hp.controls {
            assert!(c.satisfied, "{} ({}) not evidenced", c.title, c.citation);
        }
        // Transmission security is evidenced by the PHI-local receipt specifically.
        let ts = hp
            .controls
            .iter()
            .find(|c| c.control == HipaaControl::TransmissionSecurity)
            .unwrap();
        assert_eq!(ts.evidence_seqs, vec![0]);

        // Off-host verification: public key only.
        assert!(verify_hipaa_pack(&hp, &MlDsa87Verifier, l.public_key()));
        let other = RustCryptoMlDsa87::generate("other").unwrap();
        assert!(!verify_hipaa_pack(
            &hp,
            &MlDsa87Verifier,
            other.public_key()
        ));
    }

    #[test]
    fn a_tampered_pack_fails_hipaa_verification() {
        let mut l = ledger();
        l.ingest("aog-gateway", phi_receipt()).unwrap();
        let mut hp = export_hipaa_pack(&l, "2026-07-04T00:00:00Z").unwrap();
        // Flip the routing decision after signing — the tamper is caught.
        hp.pack.entries[0].receipt = serde_json::json!({
            "token_id": "tok_1", "classification": "controlled", "route": "cloud_allowed",
        });
        assert!(!verify_hipaa_pack(&hp, &MlDsa87Verifier, l.public_key()));
    }

    #[test]
    fn transmission_security_unsatisfied_without_a_phi_local_receipt() {
        // A cloud-routed public request evidences audit + integrity + access, but not
        // transmission security (nothing PHI was pinned local).
        let mut l = ledger();
        l.ingest(
            "aog-gateway",
            serde_json::json!({ "token_id": "tok_1", "classification": "public", "route": "cloud_allowed" }),
        )
        .unwrap();
        let controls = map_controls(l.entries());
        let ts = controls
            .iter()
            .find(|c| c.control == HipaaControl::TransmissionSecurity)
            .unwrap();
        assert!(!ts.satisfied);
        let audit = controls
            .iter()
            .find(|c| c.control == HipaaControl::AuditControls)
            .unwrap();
        assert!(audit.satisfied);
    }
}
