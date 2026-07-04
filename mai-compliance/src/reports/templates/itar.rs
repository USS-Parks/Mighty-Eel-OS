//! ITAR / EAR Compliance Summary template.
//!
//! Surfaces export-controlled queries, jurisdiction decisions, and
//! deny / local-only counts. Scopes the audit query to entries that
//! involve the [`ModuleId::Itar`] module.

use std::fmt::Write as _;

use crate::audit::{AuditQueryRow, RoutingDecision};
use crate::policy::composer::ModuleId;
use crate::reports::engine::{ReportSection, ReportType, TemplateContext};

use super::ReportTemplate;

/// ITAR / EAR compliance summary.
#[derive(Debug)]
pub struct ItarComplianceSummary;

impl ReportTemplate for ItarComplianceSummary {
    fn report_type(&self) -> ReportType {
        ReportType::ItarComplianceSummary
    }

    fn scope_module(&self) -> Option<ModuleId> {
        Some(ModuleId::Itar)
    }

    fn build(&self, ctx: TemplateContext<'_>) -> (String, Vec<ReportSection>) {
        let itar = ctx
            .module_activity
            .iter()
            .find(|m| m.module == ModuleId::Itar);
        let decisions = itar.map_or(0, |m| m.decision_count);
        let denies = itar.map_or(0, |m| m.deny_count);
        let summary = format!(
            "ITAR/EAR compliance summary. {decisions} export-controlled \
             evaluations in window. {denies} denied; \
             {} forced to local-only route.",
            itar.map_or(0, |m| m.local_only_count)
        );

        let sections = vec![jurisdiction_section(ctx.rows), deny_section(ctx.rows)];
        (summary, sections)
    }
}

fn jurisdiction_section(rows: &[AuditQueryRow]) -> ReportSection {
    let mut body = String::new();
    let mut count = 0usize;
    for r in rows {
        if r.entry.modules_applied.contains(&ModuleId::Itar) {
            count += 1;
            if count <= 50 {
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
    }
    if count > 50 {
        let _ = writeln!(
            body,
            "  ... ({} more rows; see CSV / JSON for full list)",
            count - 50
        );
    }
    if count == 0 {
        body.push_str("No ITAR/EAR evaluations in window.\n");
    }
    ReportSection {
        heading: "Jurisdiction decisions".into(),
        body,
    }
}

fn deny_section(rows: &[AuditQueryRow]) -> ReportSection {
    let mut body = String::new();
    let mut count = 0usize;
    for r in rows {
        if r.entry.modules_applied.contains(&ModuleId::Itar)
            && matches!(r.entry.decision, RoutingDecision::Deny)
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
        body.push_str("No ITAR/EAR denies in window.\n");
    }
    ReportSection {
        heading: "Export-control denies".into(),
        body,
    }
}
