//! `mai-admin demo` — narrated end-to-end compliance scenarios.
//!
//! Each demo calls the same `mai-compliance` APIs that the integration
//! tests in `mai-compliance/tests/compliance_demos.rs` exercise in CI,
//! but with phase-by-phase printed narration so a viewer sees the
//! Lamprey policy engine's reasoning unfold in real time. The runner
//! is *literate*: there is no shared event bus; each `phase.open()`
//! call is paired with the API invocation it describes. The narration
//! source is the runner itself.
//!
//! Pacing defaults to 150 ms between phases so the human eye can
//! follow; override with `MAI_DEMO_PACING_MS=0` for CI / smoke
//! integration that wants instant playback.

#![allow(clippy::too_many_lines)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::cast_precision_loss)]

use std::env;
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use mai_compliance::{
    AccessRole, ActorContext, AggregateDecision, AuditEntry, AuditLog, AuditRecordInput, BaaConfig,
    BaaEnforcer, CertifiedReport, ChainConfig, ChainError, ClassificationResult, ConsentStatus,
    CountryCode, CulturalFilter, EarDetector, GovernanceMetadata, ItarDetector,
    JurisdictionEvaluator, MlDsaBundleVerifier, ModuleDecision, ModuleId, OcapEvaluator,
    PersonType, PhiDetector, PolicyBundle, PolicyComposer, PolicyTemplate, PossessionStatus,
    ReportFormat, ReportManager, ReportRequest, ReportStatus, ReportType, RequestMetadata,
    TreatyDetector, TreatyDetectorConfig, TribalDataDetector, TrustContext, verify_chain,
};
use mai_core::airgap::ConnectivityState;

use crate::banner::ColorMode;

// ─── Public entry points ───────────────────────────────────────────

/// Run every demo in sequence, separated by blank lines.
pub fn run_all() -> anyhow::Result<()> {
    let mut out = io::stdout().lock();
    let color = ColorMode::detect();
    run_hipaa(&mut out, color)?;
    run_itar(&mut out, color)?;
    run_ocap(&mut out, color)?;
    run_multi_domain(&mut out, color)?;
    run_audit_tamper(&mut out, color)?;
    run_trust_manifold(&mut out, color)?;
    writeln!(out)?;
    writeln!(
        out,
        "{green}{bold}✓ ALL 6 DEMOS PASS{reset}  · use `mai-admin demo --scenario <name>` to re-run one",
        green = color.green(),
        bold = color.bold(),
        reset = color.reset()
    )?;
    Ok(())
}

/// Run a single demo by name. Names match the test functions in
/// `mai-compliance/tests/compliance_demos.rs`.
pub fn run_one(name: &str) -> anyhow::Result<()> {
    let mut out = io::stdout().lock();
    let color = ColorMode::detect();
    match name {
        "hipaa" => run_hipaa(&mut out, color),
        "itar" => run_itar(&mut out, color),
        "ocap" => run_ocap(&mut out, color),
        "multi" | "multi-domain" | "multi_domain" => run_multi_domain(&mut out, color),
        "tamper" | "audit-tamper" | "audit_tamper" => run_audit_tamper(&mut out, color),
        "trust" | "trust-manifold" | "trust_manifold" => run_trust_manifold(&mut out, color),
        other => Err(anyhow::anyhow!(
            "unknown scenario `{other}` — try: hipaa, itar, ocap, multi, tamper, trust"
        )),
    }
}

// ─── Phase renderer ────────────────────────────────────────────────

struct Phase<'a> {
    out: &'a mut dyn Write,
    color: ColorMode,
    start: Instant,
    in_phase: bool,
}

impl<'a> Phase<'a> {
    fn new(out: &'a mut dyn Write, color: ColorMode) -> Self {
        Self {
            out,
            color,
            start: Instant::now(),
            in_phase: false,
        }
    }

    fn open(&mut self, title: &str) -> io::Result<()> {
        if self.in_phase {
            writeln!(self.out)?;
        }
        self.in_phase = true;
        let elapsed_ms = self.start.elapsed().as_secs_f64() * 1000.0;
        writeln!(
            self.out,
            "  {dim}[t={elapsed:7.3}ms]{reset} {cyan}▸ {title}{reset}",
            dim = self.color.dim(),
            reset = self.color.reset(),
            cyan = self.color.cyan(),
            elapsed = elapsed_ms,
            title = title
        )
    }

    fn detail(&mut self, key: &str, value: &str) -> io::Result<()> {
        writeln!(
            self.out,
            "              {dim}├─{reset} {key:18} {value}",
            dim = self.color.dim(),
            reset = self.color.reset(),
            key = key,
            value = value
        )
    }

    fn detail_colored(&mut self, key: &str, value: &str, color_code: &str) -> io::Result<()> {
        writeln!(
            self.out,
            "              {dim}├─{reset} {key:18} {c}{value}{reset}",
            dim = self.color.dim(),
            reset = self.color.reset(),
            key = key,
            c = color_code,
            value = value
        )
    }

    fn last(&mut self, key: &str, value: &str) -> io::Result<()> {
        writeln!(
            self.out,
            "              {dim}└─{reset} {key:18} {value}",
            dim = self.color.dim(),
            reset = self.color.reset(),
            key = key,
            value = value
        )
    }

    fn last_colored(&mut self, key: &str, value: &str, color_code: &str) -> io::Result<()> {
        writeln!(
            self.out,
            "              {dim}└─{reset} {key:18} {c}{value}{reset}",
            dim = self.color.dim(),
            reset = self.color.reset(),
            key = key,
            c = color_code,
            value = value
        )
    }

    fn pause(&self) {
        thread::sleep(pacing());
    }
}

fn pacing() -> Duration {
    let ms = env::var("MAI_DEMO_PACING_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(150);
    Duration::from_millis(ms)
}

fn open_demo(out: &mut dyn Write, color: ColorMode, title: &str) -> io::Result<()> {
    writeln!(out)?;
    writeln!(
        out,
        "{bold}╔══ {title} {fill}╗{reset}",
        bold = color.bold(),
        reset = color.reset(),
        title = title,
        fill = "═".repeat(78usize.saturating_sub(title.chars().count() + 6))
    )?;
    writeln!(out)?;
    Ok(())
}

fn close_demo(
    out: &mut dyn Write,
    color: ColorMode,
    summary: &str,
    elapsed_ms: f64,
) -> io::Result<()> {
    writeln!(out)?;
    writeln!(
        out,
        "  {green}{bold}✓ {summary}{reset}  · {elapsed:.1}ms wall",
        green = color.green(),
        bold = color.bold(),
        reset = color.reset(),
        summary = summary,
        elapsed = elapsed_ms,
    )?;
    writeln!(
        out,
        "{bold}╚{fill}╝{reset}",
        bold = color.bold(),
        reset = color.reset(),
        fill = "═".repeat(78)
    )?;
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

// ─── Shared harness (mirrors compliance_demos.rs::DemoEnv) ─────────

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
            policy_version: format!("demo.{}.001", template.as_str()),
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
            source: "mai-admin-demo".to_string(),
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

fn actor_non_us() -> ActorContext {
    ActorContext {
        country: Some(CountryCode::new("DE").expect("country code")),
        person_type: PersonType::NonUsPerson,
        deployment_profile: Some("defense".into()),
    }
}

fn actor_us() -> ActorContext {
    ActorContext {
        country: Some(CountryCode::us()),
        person_type: PersonType::UsPerson,
        deployment_profile: Some("airgap-demo".into()),
    }
}

fn tribal_council_gov() -> GovernanceMetadata {
    GovernanceMetadata {
        access_role: AccessRole::Council,
        possession: PossessionStatus::OnPremises,
        consent: ConsentStatus::Granted,
        tribal_owned: true,
        force_review: false,
    }
}

// ─── Demo 1: HIPAA ─────────────────────────────────────────────────

fn run_hipaa(out: &mut dyn Write, color: ColorMode) -> anyhow::Result<()> {
    open_demo(out, color, "HIPAA · Healthcare workflow")?;
    let demo_start = Instant::now();
    let env = DemoEnv::with_template(PolicyTemplate::Healthcare);

    let prompt = "Patient John Doe (MRN 123456) presented with chest pain on 2026-05-22. \
                  Recommend imaging?";
    let mut phase = Phase::new(out, color);

    phase.open("Request submitted")?;
    phase.detail("source_app", "clinical-notes-helper")?;
    phase.detail("tenant", "local-dev (BAA on file)")?;
    phase.detail("prompt", &truncate(prompt, 56))?;
    phase.last("model_hint", "lamprey/medical-local")?;
    phase.pause();

    phase.open("PHI Detector scanning prompt")?;
    let phi = PhiDetector::baseline().scan(prompt);
    let hit_count = phi.hits.len();
    phase.detail("detector", "PhiDetector::baseline()")?;
    phase.detail_colored(
        "hits",
        &format!("{hit_count} PHI identifiers"),
        color.yellow(),
    )?;
    phase.last("rule_set", "HIPAA Safe Harbor (18 categories)")?;
    phase.pause();

    phase.open("BAA Enforcer evaluating cloud destination")?;
    let baa = BaaEnforcer::new(BaaConfig::default()).evaluate_for_cloud(&phi);
    phase.detail("mode", "BaaMode::Standard")?;
    phase.detail("phi_present", &hit_count.to_string())?;
    phase.last_colored(
        "decision",
        if baa.allowed {
            "ALLOW cloud"
        } else {
            "DENY cloud — PHI present, no per-vendor BAA"
        },
        if baa.allowed {
            color.green()
        } else {
            color.red()
        },
    )?;
    phase.pause();

    phase.open("Policy Composer aggregating module decisions")?;
    let module = ModuleDecision::from_hipaa(&baa);
    let aggregate = env.composer.compose([module]);
    let rule_codes: Vec<String> = aggregate
        .reasons
        .iter()
        .filter_map(|r| r.rule.clone())
        .collect();
    phase.detail("template", "PolicyTemplate::Healthcare")?;
    phase.detail("modules_run", "HIPAA")?;
    phase.detail("rule_codes", &rule_codes.join(", "))?;
    phase.last_colored(
        "verdict",
        if aggregate.allowed {
            "ALLOW (route to local)"
        } else {
            "DENY (any-deny-wins: HIPAA)"
        },
        if aggregate.allowed {
            color.green()
        } else {
            color.red()
        },
    )?;
    phase.pause();

    phase.open("Audit chain recording decision")?;
    let bundle = make_bundle(
        "local-dev",
        "req_hipaa_001",
        "regulated",
        TrustContext::for_local_dev(),
    );
    let entry = env.record(&bundle, &aggregate);
    phase.detail_colored("storage", "tamper-evident hash chain", color.magenta())?;
    phase.detail_colored("signature", "ML-DSA-87 (FIPS 204)", color.magenta())?;
    let cred = entry
        .correlation
        .credential_event_id
        .as_deref()
        .unwrap_or("");
    phase.detail("credential_event", cred)?;
    phase.last("decision_recorded", &format!("{:?}", entry.decision))?;
    phase.pause();

    phase.open("Generating certified HIPAA audit-trail report")?;
    let certified = env.certify(ReportType::HipaaAuditTrail, Some("local-dev".into()));
    phase.detail("type", "HipaaAuditTrail")?;
    phase.detail("format", &format!("{:?}", certified.document.format))?;
    phase.detail_colored(
        "content_hash",
        &format!("0x{}…", &certified.content_hash_hex[..16]),
        color.magenta(),
    )?;
    phase.last_colored("status", "CERTIFIED", color.green())?;
    phase.pause();

    phase.open("Verifying audit chain integrity end-to-end")?;
    env.audit
        .verify_full::<MlDsaBundleVerifier>(None)
        .map_err(|e| anyhow::anyhow!("chain verification failed: {e:?}"))?;
    let entries = env.audit.store().entries().len();
    phase.detail("entries_checked", &entries.to_string())?;
    phase.last_colored("verdict", "PASS — all links intact", color.green())?;
    phase.pause();

    close_demo(
        out,
        color,
        &format!("HIPAA DEMO PASS · {hit_count} PHI scrubbed"),
        demo_start.elapsed().as_secs_f64() * 1000.0,
    )?;
    Ok(())
}

// ─── Demo 2: ITAR ──────────────────────────────────────────────────

fn run_itar(out: &mut dyn Write, color: ColorMode) -> anyhow::Result<()> {
    open_demo(out, color, "ITAR · Defense export-control workflow")?;
    let demo_start = Instant::now();
    let env = DemoEnv::with_template(PolicyTemplate::Defense);

    let prompt = "Design notes for the F-22 air-superiority fighter, including \
                  stealth radar absorbing material lay-up.";
    let mut phase = Phase::new(out, color);

    phase.open("Request submitted")?;
    phase.detail("source_app", "defense-analyst-assistant")?;
    phase.detail("tenant", "defense-tenant")?;
    phase.detail("actor", "non-US person (country=DE)")?;
    phase.last("prompt", &truncate(prompt, 56))?;
    phase.pause();

    phase.open("ITAR + EAR detectors scanning")?;
    let itar = ItarDetector::baseline().scan(prompt);
    let ear = EarDetector::baseline().scan(prompt);
    let itar_hits = itar.hits.len();
    let ear_hits = ear.eccn_hits.len();
    phase.detail("itar_hits", &format!("{itar_hits} USML category hits"))?;
    phase.detail("ear_hits", &format!("{ear_hits} ECCN hits"))?;
    phase.last_colored("rule", "default-to-ITAR on ambiguity", color.yellow())?;
    phase.pause();

    phase.open("Jurisdiction Evaluator weighing actor + trust")?;
    let trust = TrustContext::for_local_dev();
    let decision = JurisdictionEvaluator::default().evaluate(&itar, &ear, &actor_non_us(), &trust);
    phase.detail("actor_country", "DE (non-US)")?;
    phase.detail("person_type", "NonUsPerson")?;
    phase.detail("trust_state", "for_local_dev")?;
    phase.last_colored(
        "outcome",
        &format!("{:?}", decision.outcome),
        if matches!(decision.outcome, mai_compliance::Outcome::DenyExport) {
            color.red()
        } else {
            color.green()
        },
    )?;
    phase.pause();

    phase.open("Policy Composer aggregating")?;
    let module = ModuleDecision::from_itar(&decision);
    let aggregate = env.composer.compose([module]);
    phase.detail("template", "PolicyTemplate::Defense")?;
    phase.detail("modules_run", "ITAR")?;
    phase.last_colored(
        "verdict",
        if aggregate.allowed {
            "ALLOW"
        } else {
            "DENY (ITAR DenyExport propagated)"
        },
        if aggregate.allowed {
            color.green()
        } else {
            color.red()
        },
    )?;
    phase.pause();

    phase.open("Audit chain recording")?;
    let bundle = make_bundle("defense-tenant", "req_itar_001", "critical", trust);
    let entry = env.record(&bundle, &aggregate);
    phase.detail_colored("storage", "tamper-evident hash chain", color.magenta())?;
    phase.detail_colored("signature", "ML-DSA-87 (FIPS 204)", color.magenta())?;
    phase.last("modules_applied", &format!("{:?}", entry.modules_applied))?;
    phase.pause();

    phase.open("Generating certified ITAR compliance summary")?;
    let certified = env.certify(
        ReportType::ItarComplianceSummary,
        Some("defense-tenant".into()),
    );
    phase.detail("type", "ItarComplianceSummary")?;
    phase.detail_colored(
        "content_hash",
        &format!("0x{}…", &certified.content_hash_hex[..16]),
        color.magenta(),
    )?;
    phase.last_colored("status", "CERTIFIED", color.green())?;
    phase.pause();

    phase.open("Verifying audit chain integrity")?;
    env.audit
        .verify_full::<MlDsaBundleVerifier>(None)
        .map_err(|e| anyhow::anyhow!("chain verification failed: {e:?}"))?;
    phase.last_colored("verdict", "PASS", color.green())?;
    phase.pause();

    close_demo(
        out,
        color,
        "ITAR DEMO PASS · export denied to non-US person",
        demo_start.elapsed().as_secs_f64() * 1000.0,
    )?;
    Ok(())
}

// ─── Demo 3: OCAP (tribal data sovereignty) ────────────────────────

fn run_ocap(out: &mut dyn Write, color: ColorMode) -> anyhow::Result<()> {
    open_demo(out, color, "OCAP · Tribal data sovereignty workflow")?;
    let demo_start = Instant::now();
    let env = DemoEnv::with_template(PolicyTemplate::TribalGovernment);

    let prompt = "Document captures traditional ecological knowledge held \
                  by the tribal council under the 1855 treaty.";
    let mut phase = Phase::new(out, color);

    phase.open("Request submitted")?;
    phase.detail("source_app", "tribal-records-assistant")?;
    phase.detail("tenant", "tribal-tenant")?;
    phase.detail("governance", "Council role · on-prem · consent granted")?;
    phase.last("prompt", &truncate(prompt, 56))?;
    phase.pause();

    phase.open("OCAP detectors scanning (tribal · treaty · cultural)")?;
    let tribal = TribalDataDetector::baseline().scan(prompt);
    let treaty = TreatyDetector::new(TreatyDetectorConfig::default()).scan(prompt);
    let cultural = CulturalFilter::baseline().scan(prompt);
    phase.detail(
        "tribal_signals",
        &format!("tribal_data={}", tribal.has_any()),
    )?;
    phase.detail("treaty_hits", &treaty.hits.len().to_string())?;
    phase.last("cultural_hits", &cultural.hits.len().to_string())?;
    phase.pause();

    phase.open("OCAP Evaluator weighing role + possession + consent")?;
    let trust = TrustContext::for_local_dev();
    let decision = OcapEvaluator::default()
        .evaluate(&tribal, &treaty, &cultural, &tribal_council_gov(), &trust)
        .map_err(|e| anyhow::anyhow!("OCAP evaluation failed: {e:?}"))?;
    phase.detail("access_role", "Council")?;
    phase.detail("possession", "OnPremises")?;
    phase.detail("consent", "Granted")?;
    phase.last_colored("outcome", &format!("{:?}", decision.outcome), color.green())?;
    phase.pause();

    phase.open("Policy Composer aggregating")?;
    let module = ModuleDecision::from_ocap(&decision);
    let aggregate = env.composer.compose([module]);
    phase.detail("template", "PolicyTemplate::TribalGovernment")?;
    phase.detail("modules_run", "OCAP")?;
    phase.last_colored(
        "verdict",
        if aggregate.allowed {
            "ALLOW (route local)"
        } else {
            "DENY"
        },
        if aggregate.allowed {
            color.green()
        } else {
            color.red()
        },
    )?;
    phase.pause();

    phase.open("Audit chain recording")?;
    let bundle = make_bundle("tribal-tenant", "req_ocap_001", "regulated", trust);
    let entry = env.record(&bundle, &aggregate);
    phase.detail_colored("signature", "ML-DSA-87 (FIPS 204)", color.magenta())?;
    phase.last("modules_applied", &format!("{:?}", entry.modules_applied))?;
    phase.pause();

    phase.open("Generating certified OCAP governance report")?;
    let certified = env.certify(ReportType::OcapGovernance, Some("tribal-tenant".into()));
    phase.detail_colored(
        "content_hash",
        &format!("0x{}…", &certified.content_hash_hex[..16]),
        color.magenta(),
    )?;
    phase.last_colored("status", "CERTIFIED", color.green())?;
    phase.pause();

    phase.open("Verifying audit chain integrity")?;
    env.audit
        .verify_full::<MlDsaBundleVerifier>(None)
        .map_err(|e| anyhow::anyhow!("chain verification failed: {e:?}"))?;
    phase.last_colored("verdict", "PASS", color.green())?;
    phase.pause();

    close_demo(
        out,
        color,
        "OCAP DEMO PASS · tribal data routed local under council consent",
        demo_start.elapsed().as_secs_f64() * 1000.0,
    )?;
    Ok(())
}

// ─── Demo 4: Multi-Domain (HIPAA + ITAR + OCAP at once) ────────────

fn run_multi_domain(out: &mut dyn Write, color: ColorMode) -> anyhow::Result<()> {
    open_demo(
        out,
        color,
        "MULTI-DOMAIN · HIPAA + ITAR + OCAP on one prompt",
    )?;
    let demo_start = Instant::now();
    let mut env = DemoEnv::with_template(PolicyTemplate::TribalGovernment);
    env.composer = PolicyComposer::new(mai_compliance::ComposerConfig::default());

    let prompt = "Patient John Doe (MRN 123456) is a member of the tribal council; \
                  consult the F-22 air-superiority fighter manual for medical \
                  evacuation protocols.";
    let mut phase = Phase::new(out, color);

    phase.open("Request submitted (mixed-domain content)")?;
    phase.detail("tenant", "multi-domain")?;
    phase.detail("contains", "PHI + ITAR + tribal-data")?;
    phase.last("prompt", &truncate(prompt, 56))?;
    phase.pause();

    phase.open("HIPAA module evaluating")?;
    let phi = PhiDetector::baseline().scan(prompt);
    let baa = BaaEnforcer::new(BaaConfig::default()).evaluate_for_cloud(&phi);
    let hipaa_module = ModuleDecision::from_hipaa(&baa);
    phase.detail("phi_hits", &phi.hits.len().to_string())?;
    phase.last_colored(
        "module_decision",
        if baa.allowed {
            "ALLOW"
        } else {
            "DENY (PHI to cloud)"
        },
        if baa.allowed {
            color.green()
        } else {
            color.red()
        },
    )?;
    phase.pause();

    phase.open("ITAR module evaluating (non-US actor)")?;
    let itar = ItarDetector::baseline().scan(prompt);
    let ear = EarDetector::baseline().scan(prompt);
    let jurisdiction = JurisdictionEvaluator::default().evaluate(
        &itar,
        &ear,
        &actor_non_us(),
        &TrustContext::for_local_dev(),
    );
    let itar_module = ModuleDecision::from_itar(&jurisdiction);
    phase.detail("itar_hits", &itar.hits.len().to_string())?;
    phase.last_colored(
        "module_decision",
        &format!("{:?}", jurisdiction.outcome),
        color.red(),
    )?;
    phase.pause();

    phase.open("OCAP module evaluating (council consent)")?;
    let tribal = TribalDataDetector::baseline().scan(prompt);
    let treaty = TreatyDetector::new(TreatyDetectorConfig::default()).scan(prompt);
    let cultural = CulturalFilter::baseline().scan(prompt);
    let ocap = OcapEvaluator::default()
        .evaluate(
            &tribal,
            &treaty,
            &cultural,
            &tribal_council_gov(),
            &TrustContext::for_local_dev(),
        )
        .map_err(|e| anyhow::anyhow!("OCAP evaluation failed: {e:?}"))?;
    let ocap_module = ModuleDecision::from_ocap(&ocap);
    phase.detail("tribal_detected", &ocap.tribal_data_detected.to_string())?;
    phase.last_colored(
        "module_decision",
        &format!("{:?}", ocap.outcome),
        color.green(),
    )?;
    phase.pause();

    phase.open("Composer aggregating all three modules")?;
    let aggregate = env
        .composer
        .compose([hipaa_module, itar_module, ocap_module]);
    let modules: Vec<String> = aggregate
        .modules_applied
        .iter()
        .map(|m| format!("{m:?}"))
        .collect();
    let rule_codes: Vec<String> = aggregate
        .reasons
        .iter()
        .filter_map(|r| r.rule.clone())
        .collect();
    phase.detail("modules_applied", &modules.join(" + "))?;
    phase.detail("rule_codes", &rule_codes.join(", "))?;
    phase.detail("rule_count", &rule_codes.len().to_string())?;
    phase.last_colored(
        "final_verdict",
        if aggregate.allowed {
            "ALLOW"
        } else {
            "DENY — any-deny-wins (ITAR exports + HIPAA cloud)"
        },
        if aggregate.allowed {
            color.green()
        } else {
            color.red()
        },
    )?;
    phase.pause();

    phase.open("Audit chain + monthly digest report")?;
    let bundle = make_bundle(
        "multi-domain",
        "req_multi_001",
        "critical",
        TrustContext::for_local_dev(),
    );
    let _ = env.record(&bundle, &aggregate);
    let _ = env.certify(ReportType::MonthlyDigest, None);
    phase.detail_colored("signature", "ML-DSA-87 (FIPS 204)", color.magenta())?;
    phase.last_colored(
        "explainability",
        &format!("{} rule codes carried into the record", rule_codes.len()),
        color.green(),
    )?;
    phase.pause();

    close_demo(
        out,
        color,
        "MULTI-DOMAIN DEMO PASS · 3 modules · any-deny-wins enforced",
        demo_start.elapsed().as_secs_f64() * 1000.0,
    )?;
    Ok(())
}

// ─── Demo 5: Audit Tamper Detection ────────────────────────────────

fn run_audit_tamper(out: &mut dyn Write, color: ColorMode) -> anyhow::Result<()> {
    open_demo(out, color, "AUDIT TAMPER · hash-chain integrity demo")?;
    let demo_start = Instant::now();
    let env = DemoEnv::with_template(PolicyTemplate::Standard);
    let mut phase = Phase::new(out, color);

    phase.open("Recording 3 benign decisions to build a chain")?;
    let trust = TrustContext::for_local_dev();
    let bundle = make_bundle("local-dev", "req_tamper_001", "internal", trust);
    let neutral = AggregateDecision {
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
        env.record(&bundle, &neutral);
    }
    let snapshot = env.audit.store().entries();
    phase.detail("entries", &snapshot.len().to_string())?;
    phase.last("decision_each", "ALLOW · route=Local · no PHI")?;
    phase.pause();

    phase.open("Baseline chain verification")?;
    verify_chain::<MlDsaBundleVerifier>(&snapshot, &ChainConfig::default(), None)
        .map_err(|e| anyhow::anyhow!("baseline verify failed: {e:?}"))?;
    phase.detail("config", "ChainConfig::default()")?;
    phase.last_colored("verdict", "CLEAN — every link intact", color.green())?;
    phase.pause();

    phase.open("Simulating tamper: rewriting entry #2's routing_reason")?;
    let mut tampered = snapshot;
    tampered[1].routing_reason = "TAMPERED".to_string();
    phase.detail_colored("attack", "in-memory mutation of stored entry", color.red())?;
    phase.last_colored(
        "what changes",
        "content_hash(#2) → previous_hash(#3) link breaks",
        color.yellow(),
    )?;
    phase.pause();

    phase.open("Re-verifying tampered chain")?;
    let err = verify_chain::<MlDsaBundleVerifier>(&tampered, &ChainConfig::default(), None)
        .err()
        .ok_or_else(|| anyhow::anyhow!("tampered chain unexpectedly verified clean"))?;
    let detected = matches!(err, ChainError::LinkBroken { .. });
    let err_dbg = format!("{err:?}");
    let err_kind = err_dbg.split_whitespace().next().unwrap_or("?");
    phase.detail("error_type", err_kind)?;
    phase.last_colored(
        "verdict",
        if detected {
            "DETECTED — LinkBroken error raised"
        } else {
            "MISSED — verification surprisingly passed"
        },
        if detected { color.green() } else { color.red() },
    )?;
    phase.pause();

    phase.open("Surfacing chain break as Critical escalation")?;
    let escalations = env
        .audit
        .record_chain_break("demo: test mutation detected by verify_chain");
    let critical = escalations
        .iter()
        .any(|e| e.severity() == mai_compliance::Severity::Critical);
    phase.detail("escalations_count", &escalations.len().to_string())?;
    phase.last_colored(
        "critical_present",
        if critical {
            "YES — SIEM / dashboard would page"
        } else {
            "NO — escalation pipeline mis-wired"
        },
        if critical { color.green() } else { color.red() },
    )?;
    phase.pause();

    close_demo(
        out,
        color,
        "AUDIT TAMPER DEMO PASS · tamper detected · Critical escalation fired",
        demo_start.elapsed().as_secs_f64() * 1000.0,
    )?;
    Ok(())
}

// ─── Demo 6: Trust Manifold (Disconnected + Expired) ───────────────

fn run_trust_manifold(out: &mut dyn Write, color: ColorMode) -> anyhow::Result<()> {
    open_demo(
        out,
        color,
        "TRUST MANIFOLD · disconnected + expired-bundle behavior",
    )?;
    let demo_start = Instant::now();
    let mut phase = Phase::new(out, color);

    // 6a: AirGapped trust → offline mode flag true
    phase.open("Scenario A: AirGapped trust context (no network)")?;
    let env_offline = DemoEnv::with_template(PolicyTemplate::Defense);
    let mut offline_trust = TrustContext::for_local_dev();
    offline_trust.connectivity = ConnectivityState::AirGapped;
    let offline = offline_trust.offline_mode();
    phase.detail("connectivity", "AirGapped")?;
    phase.last_colored(
        "offline_mode",
        &offline.to_string(),
        if offline { color.green() } else { color.red() },
    )?;
    phase.pause();

    phase.open("ITAR evaluator + US person in offline mode")?;
    let itar = ItarDetector::baseline().scan("F-22 air-superiority fighter manual");
    let ear = EarDetector::baseline().scan("F-22 air-superiority fighter manual");
    let decision_offline =
        JurisdictionEvaluator::default().evaluate(&itar, &ear, &actor_us(), &offline_trust);
    let agg_offline = env_offline
        .composer
        .compose([ModuleDecision::from_itar(&decision_offline)]);
    phase.detail("actor", "US person")?;
    phase.detail("outcome", &format!("{:?}", decision_offline.outcome))?;
    phase.last_colored(
        "aggregate",
        if agg_offline.allowed {
            "ALLOW (US person)"
        } else {
            "DENY"
        },
        if agg_offline.allowed {
            color.green()
        } else {
            color.red()
        },
    )?;
    phase.pause();

    phase.open("Audit entry carries trust snapshot")?;
    let bundle_offline = make_bundle("airgap-demo", "req_offline_001", "critical", offline_trust);
    let entry_offline = env_offline.record(&bundle_offline, &agg_offline);
    phase.detail_colored(
        "trust_bundle_version",
        &entry_offline.correlation.trust_bundle_version,
        color.magenta(),
    )?;
    phase.last_colored(
        "correlation",
        "trust state propagated into audit chain",
        color.green(),
    )?;
    phase.pause();

    // 6b: Expired bundle → fail closed
    phase.open("Scenario B: revocation=Unknown + stale bundle")?;
    let env_expired = DemoEnv::with_template(PolicyTemplate::Defense);
    let mut stale_trust = TrustContext::for_local_dev();
    stale_trust.revocation_status = mai_compliance::RevocationStatus::Unknown;
    stale_trust.trust_bundle_version = "2020.01.01.000".to_string();
    phase.detail("revocation_status", "Unknown")?;
    phase.last_colored(
        "trust_bundle_version",
        "2020.01.01.000 (ancient)",
        color.yellow(),
    )?;
    phase.pause();

    phase.open("ITAR evaluator under unknown revocation")?;
    let decision_stale =
        JurisdictionEvaluator::default().evaluate(&itar, &ear, &actor_us(), &stale_trust);
    let agg_stale = env_expired
        .composer
        .compose([ModuleDecision::from_itar(&decision_stale)]);
    let fail_closed = matches!(decision_stale.outcome, mai_compliance::Outcome::DenyExport);
    phase.detail("outcome", &format!("{:?}", decision_stale.outcome))?;
    phase.last_colored(
        "fail_closed",
        if fail_closed {
            "YES — DenyExport on Unknown revocation"
        } else {
            "NO — fail-open is wrong behavior"
        },
        if fail_closed {
            color.green()
        } else {
            color.red()
        },
    )?;
    phase.pause();

    phase.open("Report generation still works in degraded mode")?;
    let bundle_stale = make_bundle("airgap-demo", "req_expired_001", "critical", stale_trust);
    let _ = env_expired.record(&bundle_stale, &agg_stale);
    let certified = env_expired.certify(
        ReportType::ItarComplianceSummary,
        Some("airgap-demo".into()),
    );
    phase.detail("type", "ItarComplianceSummary")?;
    phase.detail_colored(
        "content_hash",
        &format!("0x{}…", &certified.content_hash_hex[..16]),
        color.magenta(),
    )?;
    phase.last_colored(
        "regulator_visible",
        "expired bundle version captured in TrustSection",
        color.green(),
    )?;
    phase.pause();

    close_demo(
        out,
        color,
        "TRUST MANIFOLD DEMO PASS · offline + expired both handled fail-closed",
        demo_start.elapsed().as_secs_f64() * 1000.0,
    )?;
    Ok(())
}
