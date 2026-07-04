//! Monthly Compliance Digest template.
//!
//! Executive-style cross-domain summary covering all compliance
//! modules in one short narrative. Pairs naturally with a 30-day
//! window but does not enforce one — the date range is whatever the
//! caller requests.

use std::fmt::Write as _;

use crate::reports::engine::{ReportSection, ReportType, TemplateContext};

use super::ReportTemplate;

/// Monthly executive compliance digest.
#[derive(Debug)]
pub struct MonthlyComplianceDigest;

impl ReportTemplate for MonthlyComplianceDigest {
    fn report_type(&self) -> ReportType {
        ReportType::MonthlyDigest
    }

    fn build(&self, ctx: TemplateContext<'_>) -> (String, Vec<ReportSection>) {
        let totals = ctx.decision_totals;
        let denies = totals.deny;
        let total = totals.total();
        let credential_backed = ctx.trust.credential_validation.credential_backed;
        let local_only = ctx.trust.credential_validation.local_only;
        let summary = format!(
            "Monthly compliance digest. {total} total compliance decisions; \
             {denies} denied; {} held for review. \
             {credential_backed} decisions cloud-credential-backed; \
             {local_only} local-only. Trust bundle versions in window: {}.",
            totals.quarantine,
            ctx.trust.trust_bundle_versions.join(", "),
        );

        let sections = vec![highlights_section(ctx), trust_health_section(ctx)];
        (summary, sections)
    }
}

fn highlights_section(ctx: TemplateContext<'_>) -> ReportSection {
    let mut body = String::new();
    let _ = writeln!(body, "Decision mix:");
    let _ = writeln!(body, "  allow      : {}", ctx.decision_totals.allow);
    let _ = writeln!(body, "  local_only : {}", ctx.decision_totals.local_only);
    let _ = writeln!(body, "  quarantine : {}", ctx.decision_totals.quarantine);
    let _ = writeln!(body, "  deny       : {}", ctx.decision_totals.deny);
    let _ = writeln!(body);
    let _ = writeln!(body, "Top three active modules:");
    let mut activity: Vec<_> = ctx.module_activity.iter().collect();
    activity.sort_by_key(|m| std::cmp::Reverse(m.decision_count));
    for m in activity.iter().take(3) {
        let _ = writeln!(
            body,
            "  {:<6} decisions={} deny={}",
            m.module.as_str(),
            m.decision_count,
            m.deny_count
        );
    }
    ReportSection {
        heading: "Highlights".into(),
        body,
    }
}

fn trust_health_section(ctx: TemplateContext<'_>) -> ReportSection {
    let mut body = String::new();
    let _ = writeln!(
        body,
        "Audit chain verification status: {:?}",
        ctx.trust.audit_verification
    );
    let _ = writeln!(
        body,
        "Revocation snapshot mix: valid={} revoked={} unknown={}",
        ctx.trust.revocation_snapshot.valid,
        ctx.trust.revocation_snapshot.revoked,
        ctx.trust.revocation_snapshot.unknown
    );
    let _ = writeln!(
        body,
        "Offline / degraded intervals observed: {}",
        ctx.trust.offline_intervals.len()
    );
    let _ = writeln!(
        body,
        "Distinct policy versions recorded: {}",
        ctx.trust.policy_version_history.len()
    );
    ReportSection {
        heading: "Trust health".into(),
        body,
    }
}
