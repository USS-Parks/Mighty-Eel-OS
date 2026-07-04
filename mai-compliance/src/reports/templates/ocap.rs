//! OCAP Governance Report template.
//!
//! Surfaces tribal data access decisions, treaty references, and
//! cultural-consent outcomes. Scopes the audit query to entries
//! involving the [`ModuleId::Ocap`] module.

use std::fmt::Write as _;

use crate::audit::{AuditQueryRow, RoutingDecision};
use crate::policy::composer::ModuleId;
use crate::reports::engine::{ReportSection, ReportType, TemplateContext};

use super::ReportTemplate;

/// OCAP tribal data governance report.
#[derive(Debug)]
pub struct OcapGovernanceReport;

impl ReportTemplate for OcapGovernanceReport {
    fn report_type(&self) -> ReportType {
        ReportType::OcapGovernance
    }

    fn scope_module(&self) -> Option<ModuleId> {
        Some(ModuleId::Ocap)
    }

    fn build(&self, ctx: TemplateContext<'_>) -> (String, Vec<ReportSection>) {
        let ocap = ctx
            .module_activity
            .iter()
            .find(|m| m.module == ModuleId::Ocap);
        let decisions = ocap.map_or(0, |m| m.decision_count);
        let denies = ocap.map_or(0, |m| m.deny_count);
        let local = ocap.map_or(0, |m| m.local_only_count);
        let summary = format!(
            "OCAP governance summary. {decisions} tribal-data decisions in \
             window; {local} held local-only; {denies} denied. \
             Treaty and cultural-consent breakdowns below."
        );

        let sections = vec![
            access_log_section(ctx.rows),
            violation_section(ctx.rows),
            treaty_section(ctx.rows),
        ];
        (summary, sections)
    }
}

fn access_log_section(rows: &[AuditQueryRow]) -> ReportSection {
    let mut body = String::new();
    let mut count = 0usize;
    for r in rows {
        if r.entry.modules_applied.contains(&ModuleId::Ocap) {
            count += 1;
            if count <= 50 {
                let _ = writeln!(
                    body,
                    "  - id={} ts={} decision={} reason={} tenant={}",
                    r.entry.id,
                    r.entry.timestamp_unix_nanos,
                    r.entry.decision.as_str(),
                    r.entry.routing_reason,
                    r.entry.correlation.tenant
                );
            }
        }
    }
    if count > 50 {
        let _ = writeln!(
            body,
            "  ... ({} more rows; see CSV / JSON for full list)",
            count - 50
        );
    }
    if count == 0 {
        body.push_str("No OCAP access decisions in window.\n");
    }
    ReportSection {
        heading: "Tribal data access log".into(),
        body,
    }
}

fn violation_section(rows: &[AuditQueryRow]) -> ReportSection {
    let mut body = String::new();
    let mut count = 0usize;
    for r in rows {
        if r.entry.modules_applied.contains(&ModuleId::Ocap)
            && matches!(
                r.entry.decision,
                RoutingDecision::Deny | RoutingDecision::Quarantine
            )
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
        body.push_str("No OCAP violations in window.\n");
    }
    ReportSection {
        heading: "OCAP violations".into(),
        body,
    }
}

fn treaty_section(rows: &[AuditQueryRow]) -> ReportSection {
    let mut body = String::new();
    let mut count = 0usize;
    for r in rows {
        if r.entry
            .rules_fired
            .iter()
            .any(|rm| rm.rule.as_deref().is_some_and(|x| x.contains("treaty")))
        {
            count += 1;
            let _ = writeln!(
                body,
                "  - id={} ts={} reason={}",
                r.entry.id, r.entry.timestamp_unix_nanos, r.entry.routing_reason
            );
        }
    }
    if count == 0 {
        body.push_str("No treaty-tagged decisions in window.\n");
    }
    ReportSection {
        heading: "Treaty references".into(),
        body,
    }
}
