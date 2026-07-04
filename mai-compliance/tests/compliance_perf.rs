//! Compliance performance baseline.
//!
//! Plan §1421 and roster §3845 set three numerical targets for the
//! acquisition release:
//!
//! - Composer P99 < 5 ms at sustained load
//! - Audit append throughput ≥ 1 000 entries/second sustained
//! - Report generation < 10 s for a 30-day data range
//!
//! This test exercises all three on a fixed corpus so the measured
//! numbers go directly into `docs/acquisition/READY.md`. By default
//! the sample sizes are small (CI-friendly); set `RUN_PERF_TESTS=1`
//! in the environment for the full sample sizes.
//!
//! Output is printed unconditionally so `cargo test -- --nocapture`
//! captures the numbers for the READY checklist.

#![allow(clippy::missing_docs_in_private_items)]
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mai_compliance::{
    AggregateDecision, AuditLog, AuditRecordInput, BaaConfig, BaaEnforcer, ClassificationResult,
    ComplianceReason, ComposerConfig, Destination, ItarDetector, JurisdictionEvaluator,
    ModuleDecision, ModuleId, PhiDetector, PolicyBundle, PolicyComposer, ReportFormat,
    ReportManager, ReportRequest, ReportType, RequestMetadata, TrustContext,
};

fn full_sample() -> bool {
    std::env::var("RUN_PERF_TESTS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn percentile(sorted: &[Duration], pct: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((pct / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn neutral_decision() -> AggregateDecision {
    AggregateDecision {
        allowed: true,
        route: Some(Destination::Local),
        flags: vec![],
        reasons: vec![ComplianceReason::new(
            ModuleId::Hipaa,
            Some("hipaa.no_phi_detected".into()),
            "Neutral perf sample",
        )],
        modules_applied: vec![ModuleId::Hipaa],
    }
}

fn bundle_for(tenant: &str, request_id: &str) -> PolicyBundle {
    PolicyBundle {
        request: RequestMetadata {
            request_id: request_id.into(),
            tenant_id: tenant.into(),
            timestamp_unix_ms: 1_700_000_000_000,
            source: "perf".into(),
            model_hint: None,
        },
        trust: TrustContext::for_local_dev(),
        classification: ClassificationResult {
            level: "regulated".into(),
            matched_patterns: vec![],
            entity_count: 0,
        },
    }
}

#[test]
fn composer_p99_under_5ms() {
    // The perf target is on the *composer fold* (priority sort +
    // any-deny-wins + most-restrictive-route). Detection-stage
    // latency is owned by individual detector tests (phi_perf etc.).
    // We pre-evaluate the modules once, then time only the compose
    // call — that is the "router overhead" budget from §3849.
    let composer = PolicyComposer::new(ComposerConfig::default());
    let baa = BaaEnforcer::new(BaaConfig::default());
    let phi_detector = PhiDetector::baseline();
    let itar_detector = ItarDetector::baseline();
    let jurisdiction = JurisdictionEvaluator::default();
    let trust = TrustContext::for_local_dev();
    let actor = mai_compliance::ActorContext::default();

    let texts: &[&str] = &[
        "Patient John Doe (MRN 123456) presented with chest pain.",
        "F-22 air-superiority fighter manual references.",
        "Tell me about the history of the Roman Empire.",
        "ECCN 5A002 strong cryptography module documentation.",
        "Lab results: HbA1c 5.8%, BP 130/85 mmHg, glucose 102.",
    ];
    let decisions: Vec<Vec<ModuleDecision>> = texts
        .iter()
        .map(|text| {
            let phi = phi_detector.scan(text);
            let baa_d = baa.evaluate_for_cloud(&phi);
            let hipaa = ModuleDecision::from_hipaa(&baa_d);
            let itar_report = itar_detector.scan(text);
            let ear_report = mai_compliance::EarDetector::baseline().scan(text);
            let jur = jurisdiction.evaluate(&itar_report, &ear_report, &actor, &trust);
            let itar = ModuleDecision::from_itar(&jur);
            vec![hipaa, itar]
        })
        .collect();

    let samples = if full_sample() { 50_000 } else { 5_000 };
    let mut latencies: Vec<Duration> = Vec::with_capacity(samples);
    for i in 0..samples {
        let inputs = decisions[i % decisions.len()].clone();
        let start = Instant::now();
        let _ = composer.compose(inputs);
        latencies.push(start.elapsed());
    }

    latencies.sort();
    let p50 = percentile(&latencies, 50.0);
    let p95 = percentile(&latencies, 95.0);
    let p99 = percentile(&latencies, 99.0);
    println!(
        "[composer] samples={samples} p50={:?} p95={:?} p99={:?}",
        p50, p95, p99
    );

    assert!(
        p99 < Duration::from_millis(5),
        "composer P99 must be under 5 ms, got {:?}",
        p99
    );
}

#[test]
fn audit_append_throughput_over_1000_per_sec() {
    let audit = AuditLog::default();
    let bundle = bundle_for("perf", "req_audit_perf");
    let decision = neutral_decision();
    let clock = AtomicU64::new(1_700_000_000_000_000_000);

    let entries = if full_sample() { 10_000 } else { 2_000 };
    let start = Instant::now();
    for i in 0..entries {
        let ts = clock.fetch_add(1_000_000, Ordering::SeqCst);
        let request_id = format!("req_{i}");
        let input = AuditRecordInput {
            request_id: &request_id,
            masked_request: b"<<masked>>",
            decision: &decision,
            bundle: &bundle,
            policy_version: "perf.001",
            credential_event_id: Some(format!("cred_{ts}")),
            timestamp_unix_nanos: ts,
        };
        audit.record(input).expect("audit record");
    }
    let elapsed = start.elapsed();
    let per_sec = entries as f64 / elapsed.as_secs_f64();
    println!(
        "[audit] entries={entries} elapsed={:?} throughput={:.0}/s",
        elapsed, per_sec
    );

    assert!(
        per_sec >= 1_000.0,
        "audit append throughput must be >= 1000/s, got {:.0}/s ({entries} in {:?})",
        per_sec,
        elapsed
    );
}

#[test]
fn report_generation_under_10_seconds() {
    let audit = AuditLog::default();
    let bundle = bundle_for("perf", "req_report_perf");
    let decision = neutral_decision();
    let clock = AtomicU64::new(1_700_000_000_000_000_000);

    // Seed ~1000 entries across a 30-day window. With 2.6M seconds
    // per 30 days and 1000 entries, that's roughly one entry every
    // 43 minutes — reasonable for an active tenant.
    let seed_count = if full_sample() { 1_000 } else { 200 };
    let window_nanos: u64 = 30 * 24 * 60 * 60 * 1_000_000_000;
    let stride = window_nanos / seed_count as u64;
    let base = clock.load(Ordering::SeqCst);
    for i in 0..seed_count {
        let ts = base + i as u64 * stride;
        let request_id = format!("req_{i}");
        let input = AuditRecordInput {
            request_id: &request_id,
            masked_request: b"<<masked>>",
            decision: &decision,
            bundle: &bundle,
            policy_version: "perf.001",
            credential_event_id: Some(format!("cred_{ts}")),
            timestamp_unix_nanos: ts,
        };
        audit.record(input).expect("audit record");
    }

    let mgr = ReportManager::builder(audit).build();
    let req = ReportRequest {
        report_type: ReportType::HipaaAuditTrail,
        from_unix_nanos: base,
        to_unix_nanos: base + window_nanos,
        tenant: None,
    };

    let start = Instant::now();
    let (_record, certified) = mgr
        .generate_certified(req, ReportFormat::Json, "perf.001", base + window_nanos + 1)
        .expect("report generation");
    let elapsed = start.elapsed();
    println!(
        "[report] seed_entries={seed_count} window=30d format=json elapsed={:?} content_hash_len={}",
        elapsed,
        certified.content_hash_hex.len()
    );

    assert!(
        elapsed < Duration::from_secs(10),
        "report generation must complete in <10s, got {:?}",
        elapsed
    );
}
