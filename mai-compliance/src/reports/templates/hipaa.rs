//! HIPAA Audit Trail Report template.
//!
//! Surfaces every HIPAA-relevant policy decision in the window:
//! PHI accesses, BAA module verdicts, deny / quarantine counts, and
//! the chain of policy versions in effect. Per the §A.13 gate, the
//! engine layers a trust + credential section on top of this body.

use std::fmt::Write as _;

use crate::audit::{AuditQueryRow, RoutingDecision};
use crate::policy::composer::ModuleId;
use crate::reports::engine::{ReportSection, ReportType, TemplateContext};

use super::ReportTemplate;

/// HIPAA audit trail report.
#[derive(Debug)]
pub struct HipaaAuditTrail;

impl ReportTemplate for HipaaAuditTrail {
    fn report_type(&self) -> ReportType {
        ReportType::HipaaAuditTrail
    }

    fn scope_module(&self) -> Option<ModuleId> {
        Some(ModuleId::Hipaa)
    }

    fn build(&self, ctx: TemplateContext<'_>) -> (String, Vec<ReportSection>) {
        let hipaa_count = ctx
            .module_activity
            .iter()
            .find(|m| m.module == ModuleId::Hipaa)
            .map_or(0, |m| m.decision_count);
        let denies = ctx.decision_totals.deny;
        let summary = format!(
            "HIPAA audit trail for the window. {hipaa_count} HIPAA-scoped \
             decisions processed; {denies} denied; \
             {} quarantined for review. Trust verification status \
             carried in the trust section.",
            ctx.decision_totals.quarantine
        );

        let sections = vec![
            phi_access_section(ctx.rows),
            baa_decision_section(ctx),
            violation_section(ctx.rows),
        ];
        (summary, sections)
    }
}

fn phi_access_section(rows: &[AuditQueryRow]) -> ReportSection {
    let mut body = String::new();
    let phi_rows: Vec<&AuditQueryRow> = rows
        .iter()
        .filter(|r| r.entry.modules_applied.contains(&ModuleId::Hipaa))
        .collect();
    let _ = writeln!(body, "{} HIPAA-scoped decisions in window.", phi_rows.len());
    for r in phi_rows.iter().take(50) {
        let _ = writeln!(
            body,
            "  - id={} ts={} decision={} reason={}",
            r.entry.id,
            r.entry.timestamp_unix_nanos,
            r.entry.decision.as_str(),
            r.entry.routing_reason
        );
    }
    if phi_rows.len() > 50 {
        let _ = writeln!(
            body,
            "  ... ({} more rows; see CSV / JSON for full list)",
            phi_rows.len() - 50
        );
    }
    ReportSection {
        heading: "PHI access trail".into(),
        body,
    }
}

fn baa_decision_section(ctx: TemplateContext<'_>) -> ReportSection {
    let mut body = String::new();
    let hipaa = ctx
        .module_activity
        .iter()
        .find(|m| m.module == ModuleId::Hipaa);
    if let Some(m) = hipaa {
        let _ = writeln!(body, "HIPAA module activity:");
        let _ = writeln!(body, "  decisions      : {}", m.decision_count);
        let _ = writeln!(body, "  deny           : {}", m.deny_count);
        let _ = writeln!(body, "  local_only     : {}", m.local_only_count);
        let _ = writeln!(body, "  quarantine     : {}", m.quarantine_count);
    } else {
        let _ = writeln!(body, "No HIPAA module decisions recorded in window.");
    }
    ReportSection {
        heading: "BAA / module decisions".into(),
        body,
    }
}

fn violation_section(rows: &[AuditQueryRow]) -> ReportSection {
    let mut body = String::new();
    let mut count = 0usize;
    for r in rows {
        if matches!(
            r.entry.decision,
            RoutingDecision::Deny | RoutingDecision::Quarantine
        ) && r.entry.modules_applied.contains(&ModuleId::Hipaa)
        {
            count += 1;
            let _ = writeln!(
                body,
                "  - id={} ts={} decision={} reason={}",
                r.entry.id,
                r.entry.timestamp_unix_nanos,
                r.entry.decision.as_str(),
                r.entry.routing_reason
            );
        }
    }
    if count == 0 {
        body.push_str("No HIPAA violations in the window.\n");
    }
    ReportSection {
        heading: "HIPAA violations".into(),
        body,
    }
}
