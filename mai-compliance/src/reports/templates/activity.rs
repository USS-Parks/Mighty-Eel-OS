//! System Activity Summary template.
//!
//! Module-agnostic: routing stats, module health roll-up, and the
//! aggregate decision mix. Useful as the everyday operations report
//! the dashboard surfaces by default.

use std::fmt::Write as _;

use crate::reports::engine::{ReportSection, ReportType, TemplateContext};

use super::ReportTemplate;

/// System activity summary.
#[derive(Debug)]
pub struct SystemActivitySummary;

impl ReportTemplate for SystemActivitySummary {
    fn report_type(&self) -> ReportType {
        ReportType::SystemActivity
    }

    fn build(&self, ctx: TemplateContext<'_>) -> (String, Vec<ReportSection>) {
        let totals = ctx.decision_totals;
        let summary = format!(
            "System activity for window. {} total decisions across all \
             compliance modules ({} allow, {} local_only, {} quarantine, {} deny).",
            totals.total(),
            totals.allow,
            totals.local_only,
            totals.quarantine,
            totals.deny
        );

        let sections = vec![per_module_section(ctx), top_reasons_section(ctx)];
        (summary, sections)
    }
}

fn per_module_section(ctx: TemplateContext<'_>) -> ReportSection {
    let mut body = String::new();
    if ctx.module_activity.is_empty() {
        body.push_str("No module activity recorded in window.\n");
    } else {
        for m in ctx.module_activity {
            let _ = writeln!(
                body,
                "  module={:<6} decisions={} deny={} local_only={} quarantine={}",
                m.module.as_str(),
                m.decision_count,
                m.deny_count,
                m.local_only_count,
                m.quarantine_count
            );
        }
    }
    ReportSection {
        heading: "Per-module activity".into(),
        body,
    }
}

fn top_reasons_section(ctx: TemplateContext<'_>) -> ReportSection {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for r in ctx.rows {
        *counts.entry(r.entry.routing_reason.clone()).or_insert(0) += 1;
    }
    let mut sorted: Vec<(String, u64)> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut body = String::new();
    if sorted.is_empty() {
        body.push_str("No decisions recorded in window.\n");
    } else {
        for (reason, count) in sorted.iter().take(10) {
            let _ = writeln!(body, "  {count:>6} × {reason}");
        }
    }
    ReportSection {
        heading: "Top routing reasons".into(),
        body,
    }
}
