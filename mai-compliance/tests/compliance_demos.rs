//! Compliance Demo Suite (end-to-end scenarios).
//!
//! Automates the four acquisition demos under
//! `docs/acquisition/demos/` plus the audit-tamper and Trust Manifold
//! disconnected / expired scenarios required by plan §A.14. Each
//! scenario walks the full Lamprey stack: detection → composer →
//! audit → certified report.
//!
//! Tests live here (not in `mai-api/tests/`) because they exercise
//! compliance-engine semantics directly. The HTTP surface around the
//! same engines is already covered by
//! `mai-api/tests/compliance_integration.rs`.

#![allow(clippy::missing_docs_in_private_items)]

use std::sync::atomic::{AtomicU64, Ordering};

use mai_compliance::{
    AccessRole, ActorContext, AggregateDecision, AuditEntry, AuditLog, AuditRecordInput, BaaConfig,
    BaaEnforcer, CertifiedReport, ChainConfig, ChainError, ClassificationResult, ConsentStatus,
    CountryCode, CulturalFilter, EarDetector, GovernanceMetadata, ItarDetector,
    JurisdictionEvaluator, MlDsaBundleVerifier, ModuleDecision, ModuleId, OcapEvaluator,
    PersonType, PhiDetector, PolicyBundle, PolicyComposer, PolicyTemplate, PossessionStatus,
    ReportFormat, ReportManager, ReportRequest, ReportStatus, ReportType, RequestMetadata,
    RoutingDecision, TreatyDetector, TreatyDetectorConfig, TribalDataDetector, TrustContext,
    verify_chain,
};
use mai_core::airgap::ConnectivityState;

// ─── Shared harness ────────────────────────────────────────────────

/// Deterministic per-scenario environment. One per test — never
/// shared across tests so timestamps and audit ids stay isolated.
struct DemoEnv {
    composer: PolicyComposer,
    audit: AuditLog,
    reports: ReportManager,
    policy_version: String,
    clock: AtomicU64,
}

impl DemoEnv {
    fn with_template(template: PolicyTemplate) -> Self {
        let composer = PolicyComposer::new(template.composer_config());
        let audit = AuditLog::default();
        let reports = ReportManager::builder(audit.clone()).build();
        Self {
            composer,
            audit,
            reports,
            policy_version: format!("test.{}.001", template.as_str()),
            clock: AtomicU64::new(1_700_000_000_000_000_000),
        }
    }

    fn next_timestamp(&self) -> u64 {
        self.clock.fetch_add(1_000_000, Ordering::SeqCst)
    }

    fn record(&self, bundle: &PolicyBundle, decision: &AggregateDecision) -> AuditEntry {
        let ts = self.next_timestamp();
        let input = AuditRecordInput {
            request_id: &bundle.request.request_id,
            masked_request: b"<<masked>>",
            decision,
            bundle,
            policy_version: &self.policy_version,
            credential_event_id: Some(format!("cred_{ts}")),
            timestamp_unix_nanos: ts,
        };
        self.audit.record(input).expect("audit record failed").0
    }

    fn certify(&self, kind: ReportType, tenant: Option<String>) -> CertifiedReport {
        let now = self.next_timestamp();
        let req = ReportRequest {
            report_type: kind,
            from_unix_nanos: 0,
            to_unix_nanos: now + 1,
            tenant,
        };
        let (record, certified) = self
            .reports
            .generate_certified(req, ReportFormat::Json, &self.policy_version, now)
            .expect("report generation failed");
        assert_eq!(record.status, ReportStatus::Complete);
        certified
    }
}

fn make_bundle(tenant: &str, request_id: &str, level: &str, trust: TrustContext) -> PolicyBundle {
    PolicyBundle {
        request: RequestMetadata {
            request_id: request_id.to_string(),
            tenant_id: tenant.to_string(),
            timestamp_unix_ms: 1_700_000_000_000,
            source: "demo-suite".to_string(),
            model_hint: Some("lamprey/medical-local".to_string()),
        },
        trust,
        classification: ClassificationResult {
            level: level.to_string(),
            matched_patterns: vec![],
            entity_count: 0,
        },
    }
}

// ─── Demo 1 — Healthcare (HIPAA) ───────────────────────────────────

#[test]
fn test_hipaa_workflow() {
    let env = DemoEnv::with_template(PolicyTemplate::Healthcare);

    // 1. PHI detection on a representative healthcare prompt.
    let prompt = "Patient John Doe (MRN 123456) presented with chest pain \
                  on 2026-05-22. Recommend imaging?";
    let phi = PhiDetector::baseline().scan(prompt);
    assert!(
        !phi.hits.is_empty(),
        "PHI detector should flag patient identifiers"
    );

    // 2. BAA enforcement — Standard mode blocks any explicit PHI to cloud.
    let enforcer = BaaEnforcer::new(BaaConfig::default());
    let baa = enforcer.evaluate_for_cloud(&phi);
    assert!(!baa.allowed, "BAA Standard must refuse PHI for cloud");

    // 3. Normalise to a module decision and compose.
    let module_decision = ModuleDecision::from_hipaa(&baa);
    let aggregate = env.composer.compose([module_decision]);
    assert!(!aggregate.allowed, "aggregate must reflect HIPAA deny");
    assert!(
        aggregate.reasons.iter().any(|r| r
            .rule
            .as_deref()
            .is_some_and(|rule| rule.starts_with("hipaa"))
            || r.summary.to_lowercase().contains("phi")),
        "aggregate reasons must reference HIPAA / PHI — got {:?}",
        aggregate.reasons
    );

    // 4. Record the decision in the tamper-evident audit log.
    let bundle = make_bundle(
        "local-dev",
        "req_hipaa_001",
        "regulated",
        TrustContext::for_local_dev(),
    );
    let entry = env.record(&bundle, &aggregate);
    assert_eq!(entry.decision, RoutingDecision::from_aggregate(&aggregate));
    assert!(
        !entry
            .correlation
            .credential_event_id
            .as_deref()
            .unwrap_or("")
            .is_empty(),
        "correlation must carry a credential_event_id"
    );

    // 5. Generate a certified HIPAA report covering the window.
    let certified = env.certify(ReportType::HipaaAuditTrail, Some("local-dev".into()));
    assert_eq!(certified.document.format, ReportFormat::Json);
    assert!(
        !certified.content_hash_hex.is_empty(),
        "every certified report carries a content hash"
    );

    // 6. Audit chain must verify intact after the round-trip.
    env.audit
        .verify_full::<mai_compliance::MlDsaBundleVerifier>(None)
        .expect("audit chain integrity broken after HIPAA scenario");
}

// ─── Demo 2 — Defense (ITAR / EAR) ─────────────────────────────────

fn actor_non_us() -> ActorContext {
    ActorContext {
        country: Some(CountryCode::new("DE").expect("country code")),
        person_type: PersonType::NonUsPerson,
        deployment_profile: Some("defense".into()),
    }
}

#[test]
fn test_itar_workflow() {
    let env = DemoEnv::with_template(PolicyTemplate::Defense);

    // 1. Detect controlled technical data on a representative
    //    defense-domain prompt.
    let prompt = "Design notes for the F-22 air-superiority fighter, \
                  including stealth radar absorbing material lay-up.";
    let itar = ItarDetector::baseline().scan(prompt);
    let ear = EarDetector::baseline().scan(prompt);
    assert!(
        !itar.hits.is_empty(),
        "ITAR detector must flag this fighter-aircraft prompt"
    );

    // 2. Jurisdiction evaluation with a non-US actor must deny export.
    let evaluator = JurisdictionEvaluator::default();
    let trust = TrustContext::for_local_dev();
    let decision = evaluator.evaluate(&itar, &ear, &actor_non_us(), &trust);
    assert_eq!(
        decision.outcome,
        mai_compliance::Outcome::DenyExport,
        "ITAR + non-US actor must deny: {:?}",
        decision
    );

    // 3. Compose and assert local routing with allowed=false.
    let module = ModuleDecision::from_itar(&decision);
    let aggregate = env.composer.compose([module]);
    assert!(!aggregate.allowed, "ITAR DenyExport must propagate");

    // 4. Audit + report.
    let bundle = make_bundle("defense-tenant", "req_itar_001", "critical", trust);
    let entry = env.record(&bundle, &aggregate);
    assert!(entry.modules_applied.contains(&ModuleId::Itar));
    let certified = env.certify(
        ReportType::ItarComplianceSummary,
        Some("defense-tenant".into()),
    );
    assert!(!certified.content_hash_hex.is_empty());

    env.audit
        .verify_full::<MlDsaBundleVerifier>(None)
        .expect("audit chain integrity broken after ITAR scenario");
}

// ─── Demo 3 — Tribal Data Sovereignty (OCAP) ───────────────────────

fn tribal_council_gov() -> GovernanceMetadata {
    GovernanceMetadata {
        access_role: AccessRole::Council,
        possession: PossessionStatus::OnPremises,
        consent: ConsentStatus::Granted,
        tribal_owned: true,
        force_review: false,
    }
}

fn scan_ocap(
    text: &str,
) -> (
    mai_compliance::TribalDataReport,
    mai_compliance::TreatyReport,
    mai_compliance::CulturalReport,
) {
    (
        TribalDataDetector::baseline().scan(text),
        TreatyDetector::new(TreatyDetectorConfig::default()).scan(text),
        CulturalFilter::baseline().scan(text),
    )
}

#[test]
fn test_ocap_workflow() {
    let env = DemoEnv::with_template(PolicyTemplate::TribalGovernment);

    // 1. Tribal data + treaty references.
    let prompt = "Document captures traditional ecological knowledge held \
                  by the tribal council under the 1855 treaty.";
    let (tribal, treaty, cultural) = scan_ocap(prompt);
    assert!(tribal.has_any(), "tribal detector should flag this prompt");

    // 2. OCAP evaluation under Council role, on-prem possession,
    //    granted consent → RouteLocal.
    let evaluator = OcapEvaluator::default();
    let trust = TrustContext::for_local_dev();
    let decision = evaluator
        .evaluate(&tribal, &treaty, &cultural, &tribal_council_gov(), &trust)
        .expect("OCAP evaluation");
    assert_eq!(
        decision.outcome,
        mai_compliance::OcapOutcome::RouteLocal,
        "tribal data with consent must route local: {:?}",
        decision
    );
    assert!(decision.tribal_data_detected);

    // 3. Compose and record.
    let module = ModuleDecision::from_ocap(&decision);
    let aggregate = env.composer.compose([module]);
    assert!(aggregate.allowed, "local route should be allowed");
    let bundle = make_bundle("tribal-tenant", "req_ocap_001", "regulated", trust);
    let entry = env.record(&bundle, &aggregate);
    assert!(entry.modules_applied.contains(&ModuleId::Ocap));

    // 4. Certified OCAP governance report.
    let certified = env.certify(ReportType::OcapGovernance, Some("tribal-tenant".into()));
    assert!(!certified.content_hash_hex.is_empty());

    env.audit
        .verify_full::<MlDsaBundleVerifier>(None)
        .expect("audit chain integrity broken after OCAP scenario");
}

// ─── Demo 4 — Multi-Domain Conflict (HIPAA + ITAR + OCAP) ──────────

#[test]
fn test_multi_domain() {
    // Default composer config enables all three modules with the
    // canonical OCAP > ITAR > HIPAA priority chain. Templates each
    // narrow this — multi-domain needs the full surface.
    let env = {
        let mut env = DemoEnv::with_template(PolicyTemplate::TribalGovernment);
        env.composer = PolicyComposer::new(mai_compliance::ComposerConfig::default());
        env
    };

    // A single prompt mixing PHI, ITAR keyword, and tribal data.
    let prompt = "Patient John Doe (MRN 123456) is a member of the tribal \
                  council; consult the F-22 air-superiority fighter manual \
                  for medical evacuation protocols.";

    // Each module evaluates the same prompt independently.
    let phi = PhiDetector::baseline().scan(prompt);
    let baa = BaaEnforcer::new(BaaConfig::default()).evaluate_for_cloud(&phi);
    let hipaa = ModuleDecision::from_hipaa(&baa);

    let itar = ItarDetector::baseline().scan(prompt);
    let ear = EarDetector::baseline().scan(prompt);
    let jurisdiction = JurisdictionEvaluator::default().evaluate(
        &itar,
        &ear,
        &actor_non_us(),
        &TrustContext::for_local_dev(),
    );
    let itar_module = ModuleDecision::from_itar(&jurisdiction);

    let (tribal, treaty, cultural) = scan_ocap(prompt);
    let ocap = OcapEvaluator::default()
        .evaluate(
            &tribal,
            &treaty,
            &cultural,
            &tribal_council_gov(),
            &TrustContext::for_local_dev(),
        )
        .expect("OCAP evaluation");
    let ocap_module = ModuleDecision::from_ocap(&ocap);

    // Compose all three. Template TribalGovernment runs OCAP > HIPAA
    // and includes ITAR via composer config (Defense priority does
    // not include OCAP, hence the choice of template).
    let aggregate = env
        .composer
        .compose([hipaa.clone(), itar_module.clone(), ocap_module.clone()]);

    // Any-deny-wins: ITAR DenyExport forces allowed=false.
    assert!(!aggregate.allowed, "any deny must propagate to aggregate");

    // Reasons must cite every module that fired so a reviewer can
    // explain the outcome.
    let rule_codes: Vec<String> = aggregate
        .reasons
        .iter()
        .filter_map(|r| r.rule.clone())
        .collect();
    let modules_seen: Vec<_> = aggregate.modules_applied.clone();
    assert!(modules_seen.contains(&ModuleId::Ocap), "OCAP must appear");
    assert!(modules_seen.contains(&ModuleId::Itar), "ITAR must appear");
    // HIPAA is enabled by TribalGovernment; only included when the BAA
    // actually fires. PHI is explicit here, so it should be present.
    assert!(
        modules_seen.contains(&ModuleId::Hipaa),
        "HIPAA must appear when PHI is explicit — got {:?}",
        modules_seen
    );

    // Record and report rolls up to MonthlyDigest.
    let bundle = make_bundle(
        "multi-domain",
        "req_multi_001",
        "critical",
        TrustContext::for_local_dev(),
    );
    let entry = env.record(&bundle, &aggregate);
    assert_eq!(entry.decision, RoutingDecision::from_aggregate(&aggregate));
    let _ = env.certify(ReportType::MonthlyDigest, None);

    // Sanity check: at least one rule code is present, proving the
    // decision is explainable.
    assert!(
        !rule_codes.is_empty(),
        "aggregate must carry at least one rule code for explainability"
    );
}

// ─── Demo 5 — Audit Tamper Detection ───────────────────────────────

#[test]
fn test_audit_tamper() {
    let env = DemoEnv::with_template(PolicyTemplate::Standard);

    // Record three benign decisions to build a non-trivial chain.
    let trust = TrustContext::for_local_dev();
    let bundle = make_bundle("local-dev", "req_tamper_001", "internal", trust.clone());
    let neutral_decision = AggregateDecision {
        allowed: true,
        route: Some(mai_compliance::Destination::Local),
        flags: vec![],
        reasons: vec![mai_compliance::ComplianceReason::new(
            ModuleId::Hipaa,
            Some("hipaa.no_phi_detected".into()),
            "Neutral request",
        )],
        modules_applied: vec![ModuleId::Hipaa],
    };
    for _ in 0..3 {
        env.record(&bundle, &neutral_decision);
    }

    // Unmodified snapshot verifies clean.
    let snapshot = env.audit.store().entries();
    assert_eq!(snapshot.len(), 3);
    verify_chain::<MlDsaBundleVerifier>(&snapshot, &ChainConfig::default(), None)
        .expect("baseline chain must verify");

    // Tamper: rewrite entry #2's routing_reason. The chain link from
    // entry #2 to entry #3 must now break because content_hash(#2)
    // changes and entry #3.previous_hash no longer matches.
    let mut tampered = snapshot;
    tampered[1].routing_reason = "TAMPERED".to_string();
    let err = verify_chain::<MlDsaBundleVerifier>(&tampered, &ChainConfig::default(), None)
        .expect_err("tampered chain must fail verification");
    assert!(
        matches!(err, ChainError::LinkBroken { .. }),
        "expected LinkBroken, got {err:?}"
    );

    // Recording the break via the public API surfaces a Critical
    // escalation that the dashboard / SIEM bridge picks up.
    let escalations = env
        .audit
        .record_chain_break("test mutation detected by verify_chain");
    assert!(
        escalations
            .iter()
            .any(|e| e.severity() == mai_compliance::Severity::Critical),
        "chain break must escalate Critical — got {escalations:?}"
    );
}

// ─── Demo 6 — Trust Manifold (Disconnected + Expired) ──────────────

#[test]
fn test_trust_manifold_disconnected_and_expired() {
    // 6a. Disconnected: offline_mode flag is true, decision is recorded,
    //     and the trust snapshot on the audit entry reflects offline
    //     status.
    let env_offline = DemoEnv::with_template(PolicyTemplate::Defense);
    let mut offline_trust = TrustContext::for_local_dev();
    offline_trust.connectivity = ConnectivityState::AirGapped;
    assert!(offline_trust.offline_mode(), "AirGapped is offline");

    let (itar, ear) = (
        ItarDetector::baseline().scan("F-22 air-superiority fighter manual"),
        EarDetector::baseline().scan("F-22 air-superiority fighter manual"),
    );
    let actor_us = ActorContext {
        country: Some(CountryCode::us()),
        person_type: PersonType::UsPerson,
        deployment_profile: Some("airgap-demo".into()),
    };
    let decision_offline =
        JurisdictionEvaluator::default().evaluate(&itar, &ear, &actor_us, &offline_trust);
    let agg_offline = env_offline
        .composer
        .compose([ModuleDecision::from_itar(&decision_offline)]);
    let bundle_offline = make_bundle("airgap-demo", "req_offline_001", "critical", offline_trust);
    let entry_offline = env_offline.record(&bundle_offline, &agg_offline);
    assert_eq!(
        entry_offline.correlation.trust_bundle_version, bundle_offline.trust.trust_bundle_version,
        "trust_bundle_version must propagate into audit correlation"
    );

    // 6b. Expired / revocation-unknown: ITAR evaluator falls back to
    //     fail-closed because revocation status cannot be confirmed.
    let env_expired = DemoEnv::with_template(PolicyTemplate::Defense);
    let mut stale_trust = TrustContext::for_local_dev();
    stale_trust.revocation_status = mai_compliance::RevocationStatus::Unknown;
    stale_trust.trust_bundle_version = "2020.01.01.000".to_string(); // ancient
    let decision_stale =
        JurisdictionEvaluator::default().evaluate(&itar, &ear, &actor_us, &stale_trust);
    assert_eq!(
        decision_stale.outcome,
        mai_compliance::Outcome::DenyExport,
        "ITAR with Unknown revocation must fail closed: {:?}",
        decision_stale
    );
    let agg_stale = env_expired
        .composer
        .compose([ModuleDecision::from_itar(&decision_stale)]);
    assert!(!agg_stale.allowed);
    let bundle_stale = make_bundle("airgap-demo", "req_expired_001", "critical", stale_trust);
    env_expired.record(&bundle_stale, &agg_stale);

    // Report generation still works in degraded mode; the TrustSection
    // captures the expired bundle version for the regulator.
    let certified = env_expired.certify(
        ReportType::ItarComplianceSummary,
        Some("airgap-demo".into()),
    );
    assert!(!certified.content_hash_hex.is_empty());
}
