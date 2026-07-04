//! Retention-based report pruning.
//!
//! Reports are not part of the audit chain — losing them does not
//! break the audit log's tamper-evidence property — but regulators
//! expect a clear retention policy. This module enforces one:
//!
//! - Per [`super::engine::ReportType`] retention horizons (configured
//!   via `config/compliance/reports.toml`).
//! - Reports marked `protected = true` (e.g. those handed to an
//!   external regulator) are never auto-deleted.
//! - Auto-pruning returns a [`PruneOutcome`] summary so the caller
//!   can emit a `report.pruned` audit event.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::api::{ReportRecord, ReportScheduleId};
use super::engine::ReportType;

/// Configuration for [`ReportPruner`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneConfig {
    /// Per-report-type retention horizon in days. Missing keys fall
    /// back to [`Self::default_retention_days`].
    #[serde(default)]
    pub retention_by_type: HashMap<String, u32>,
    /// Default retention horizon (days) for any report type not
    /// listed in [`Self::retention_by_type`].
    #[serde(default = "PruneConfig::default_retention_days")]
    pub default_retention_days: u32,
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self {
            retention_by_type: HashMap::new(),
            default_retention_days: Self::default_retention_days(),
        }
    }
}

impl PruneConfig {
    /// Default retention: 7 years (HIPAA minimum).
    pub fn default_retention_days() -> u32 {
        7 * 365
    }

    /// Retention horizon for the given report type in nanoseconds.
    pub fn retention_nanos(&self, report_type: &ReportType) -> u64 {
        let days = self
            .retention_by_type
            .get(report_type.as_str())
            .copied()
            .unwrap_or(self.default_retention_days);
        u64::from(days) * 24 * 60 * 60 * 1_000_000_000
    }
}

/// Summary of one pruning pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneOutcome {
    /// Total reports examined.
    pub examined: u64,
    /// Reports that hit retention and were deleted.
    pub deleted: u64,
    /// Reports that hit retention but were kept (protected = true).
    pub kept_protected: u64,
    /// Schedule ids whose backing reports were deleted (so the
    /// scheduler can clear stale handles).
    pub schedules_touched: Vec<ReportScheduleId>,
}

/// Pruner over an in-memory store of [`ReportRecord`]s.
///
/// Operates on a slice the caller owns; the [`super::api::ReportManager`]
/// uses this internally but external callers can also invoke it
/// directly against an archived directory's index.
#[derive(Debug, Clone)]
pub struct ReportPruner {
    config: PruneConfig,
}

impl ReportPruner {
    /// Build a pruner with the given config.
    pub fn new(config: PruneConfig) -> Self {
        Self { config }
    }

    /// Pruner using default retention.
    pub fn with_default_config() -> Self {
        Self::new(PruneConfig::default())
    }

    /// Active configuration.
    pub fn config(&self) -> &PruneConfig {
        &self.config
    }

    /// Run a pruning pass against the supplied records, removing
    /// expired-and-not-protected entries in place.
    pub fn prune(&self, records: &mut Vec<ReportRecord>, now_unix_nanos: u64) -> PruneOutcome {
        let mut outcome = PruneOutcome {
            examined: records.len() as u64,
            ..PruneOutcome::default()
        };
        let mut keep = Vec::with_capacity(records.len());
        for record in records.drain(..) {
            let retention = self.config.retention_nanos(&record.request.report_type);
            let age = now_unix_nanos.saturating_sub(record.created_at_unix_nanos);
            if age <= retention {
                keep.push(record);
                continue;
            }
            if record.protected {
                outcome.kept_protected += 1;
                keep.push(record);
                continue;
            }
            outcome.deleted += 1;
            if let Some(sched) = record.schedule_id.clone() {
                outcome.schedules_touched.push(sched);
            }
        }
        *records = keep;
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reports::api::{ReportRequest, ReportStatus};

    fn fake_record(
        id: &str,
        report_type: ReportType,
        created: u64,
        protected: bool,
    ) -> ReportRecord {
        ReportRecord {
            id: id.into(),
            request: ReportRequest {
                report_type,
                from_unix_nanos: 0,
                to_unix_nanos: 1_000,
                tenant: None,
            },
            status: ReportStatus::Complete,
            created_at_unix_nanos: created,
            completed_at_unix_nanos: Some(created),
            output_format: super::super::engine::ReportFormat::Json,
            content_hash_hex: Some("0".repeat(64)),
            error: None,
            protected,
            schedule_id: None,
            body_bytes: Some(b"{}".to_vec()),
            signature_hex: None,
        }
    }

    #[test]
    fn default_retention_is_seven_years() {
        let cfg = PruneConfig::default();
        let want = 7 * 365 * 24 * 60 * 60 * 1_000_000_000;
        assert_eq!(cfg.retention_nanos(&ReportType::HipaaAuditTrail), want);
    }

    #[test]
    fn per_type_retention_overrides_default() {
        let mut cfg = PruneConfig::default();
        cfg.retention_by_type.insert("system_activity".into(), 30);
        let want = 30u64 * 24 * 60 * 60 * 1_000_000_000;
        assert_eq!(cfg.retention_nanos(&ReportType::SystemActivity), want);
    }

    #[test]
    fn prune_removes_expired_and_keeps_recent() {
        let cfg = PruneConfig {
            default_retention_days: 1,
            ..Default::default()
        };
        let pruner = ReportPruner::new(cfg);
        let mut records = vec![
            fake_record("r1", ReportType::SystemActivity, 0, false),
            fake_record(
                "r2",
                ReportType::SystemActivity,
                2 * 24 * 60 * 60 * 1_000_000_000,
                false,
            ),
        ];
        let now = 3u64 * 24 * 60 * 60 * 1_000_000_000;
        let outcome = pruner.prune(&mut records, now);
        assert_eq!(outcome.examined, 2);
        assert_eq!(outcome.deleted, 1);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "r2");
    }

    #[test]
    fn protected_records_are_never_deleted() {
        let cfg = PruneConfig {
            default_retention_days: 0,
            ..Default::default()
        };
        let pruner = ReportPruner::new(cfg);
        let mut records = vec![fake_record("p1", ReportType::SystemActivity, 0, true)];
        let outcome = pruner.prune(&mut records, 10_000);
        assert_eq!(outcome.deleted, 0);
        assert_eq!(outcome.kept_protected, 1);
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn pruner_returns_schedule_ids_for_deleted_records() {
        let cfg = PruneConfig {
            default_retention_days: 0,
            ..Default::default()
        };
        let pruner = ReportPruner::new(cfg);
        let mut r = fake_record("r3", ReportType::SystemActivity, 0, false);
        r.schedule_id = Some(ReportScheduleId("sched-1".into()));
        let outcome = pruner.prune(&mut vec![r], 10_000);
        assert_eq!(outcome.schedules_touched.len(), 1);
        assert_eq!(outcome.schedules_touched[0].as_str(), "sched-1");
    }
}
