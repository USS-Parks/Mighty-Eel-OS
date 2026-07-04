//! Compliance triggers and escalation.
//!
//! [`TriggerManager`] watches the audit stream and fires escalation
//! events when configured thresholds are crossed:
//!
//! - Sliding-window **violation threshold** — N violations in M
//!   minutes promotes the situation to an escalation event the
//!   dashboard surfaces in red.
//! - **Policy-change trigger** — every policy change emits a
//!   `PolicyChanged` escalation so dashboards always know when the
//!   active configuration drifted.
//! - **Chain-break alert** — when the chain verifier returns an
//!   error, the manager fires a `ChainBreak` escalation; this is
//!   *critical* severity and meant to page operators.
//! - **Storage quota** — at 80% of the configured retention budget
//!   the manager warns; at 90% it requests archival.
//!
//! The manager is intentionally pure / synchronous: it consumes an
//! event, mutates its sliding-window counter, and returns a list of
//! escalations the caller can publish (typically back to the compliance
//! audit feed and / or to a paging channel).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::entry::RoutingDecision;
use crate::policy::composer::ModuleId;

/// Severity of an escalation event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational only.
    Info,
    /// Operator should investigate (e.g. quota warning).
    Warn,
    /// Operator must investigate immediately (e.g. quota at 90%).
    Critical,
}

/// An escalation event surfaced by the trigger manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Escalation {
    /// Violation threshold crossed in the sliding window.
    ViolationThreshold {
        /// Severity of the escalation.
        severity: Severity,
        /// Count of violations observed in the window.
        count: u32,
        /// Configured threshold.
        threshold: u32,
        /// Window duration in seconds.
        window_secs: u64,
    },
    /// A policy change occurred.
    PolicyChanged {
        /// Severity (typically `Info`).
        severity: Severity,
        /// Short summary the dashboard renders.
        summary: String,
    },
    /// The hash-chain verifier returned an error. Always
    /// [`Severity::Critical`].
    ChainBreak {
        /// Stable description of the break.
        reason: String,
    },
    /// Storage usage crossed a watermark.
    StorageQuota {
        /// `Warn` (80%) or `Critical` (90%+).
        severity: Severity,
        /// Used entries.
        used: u64,
        /// Capacity entries.
        capacity: u64,
        /// Percentage used (0–100).
        percent: u8,
    },
}

impl Escalation {
    /// Wire-format kind identifier (matches the serde tag).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ViolationThreshold { .. } => "violation_threshold",
            Self::PolicyChanged { .. } => "policy_changed",
            Self::ChainBreak { .. } => "chain_break",
            Self::StorageQuota { .. } => "storage_quota",
        }
    }

    /// Severity of this escalation.
    pub fn severity(&self) -> Severity {
        match self {
            Self::ViolationThreshold { severity, .. }
            | Self::PolicyChanged { severity, .. }
            | Self::StorageQuota { severity, .. } => *severity,
            Self::ChainBreak { .. } => Severity::Critical,
        }
    }
}

/// Configuration for the trigger manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggersConfig {
    /// Sliding-window length in seconds (default 300 = 5 minutes).
    #[serde(default = "TriggersConfig::default_window_secs")]
    pub window_secs: u64,
    /// Violation count within the window that triggers escalation
    /// (default 5).
    #[serde(default = "TriggersConfig::default_violation_threshold")]
    pub violation_threshold: u32,
    /// Storage warn watermark, percent (default 80).
    #[serde(default = "TriggersConfig::default_warn_percent")]
    pub warn_percent: u8,
    /// Storage critical watermark, percent (default 90).
    #[serde(default = "TriggersConfig::default_critical_percent")]
    pub critical_percent: u8,
}

impl Default for TriggersConfig {
    fn default() -> Self {
        Self {
            window_secs: Self::default_window_secs(),
            violation_threshold: Self::default_violation_threshold(),
            warn_percent: Self::default_warn_percent(),
            critical_percent: Self::default_critical_percent(),
        }
    }
}

impl TriggersConfig {
    /// Default sliding-window length (300 s).
    pub fn default_window_secs() -> u64 {
        300
    }
    /// Default violation threshold (5).
    pub fn default_violation_threshold() -> u32 {
        5
    }
    /// Default warn watermark (80%).
    pub fn default_warn_percent() -> u8 {
        80
    }
    /// Default critical watermark (90%).
    pub fn default_critical_percent() -> u8 {
        90
    }
}

/// Trigger manager. Not thread-safe by itself — callers wrap it in a
/// `Mutex` when shared. Kept lock-free so the per-event hot path is
/// trivially benchmarkable.
#[derive(Debug, Clone)]
pub struct TriggerManager {
    config: TriggersConfig,
    violations: VecDeque<Instant>,
    last_quota_severity: Option<Severity>,
}

impl Default for TriggerManager {
    fn default() -> Self {
        Self::new(TriggersConfig::default())
    }
}

impl TriggerManager {
    /// Build a manager with the given config.
    pub fn new(config: TriggersConfig) -> Self {
        Self {
            config,
            violations: VecDeque::new(),
            last_quota_severity: None,
        }
    }

    /// Active configuration.
    pub fn config(&self) -> &TriggersConfig {
        &self.config
    }

    /// Replace the active configuration. Sliding-window state is
    /// preserved.
    pub fn set_config(&mut self, config: TriggersConfig) {
        self.config = config;
    }

    /// Record a decision. Returns any escalation triggered by this
    /// observation.
    pub fn record_decision(
        &mut self,
        decision: RoutingDecision,
        _module: Option<ModuleId>,
        now: Instant,
    ) -> Vec<Escalation> {
        let violation = matches!(
            decision,
            RoutingDecision::Deny | RoutingDecision::Quarantine
        );
        if !violation {
            self.prune(now);
            return Vec::new();
        }
        self.violations.push_back(now);
        self.prune(now);
        let count = u32::try_from(self.violations.len()).unwrap_or(u32::MAX);
        if count >= self.config.violation_threshold {
            let severity = if count >= self.config.violation_threshold.saturating_mul(2) {
                Severity::Critical
            } else {
                Severity::Warn
            };
            vec![Escalation::ViolationThreshold {
                severity,
                count,
                threshold: self.config.violation_threshold,
                window_secs: self.config.window_secs,
            }]
        } else {
            Vec::new()
        }
    }

    /// Record a policy change. Always returns one escalation event.
    pub fn record_policy_change(&self, summary: impl Into<String>) -> Vec<Escalation> {
        vec![Escalation::PolicyChanged {
            severity: Severity::Info,
            summary: summary.into(),
        }]
    }

    /// Record a chain verification failure. Always returns one
    /// critical escalation.
    pub fn record_chain_break(&self, reason: impl Into<String>) -> Vec<Escalation> {
        vec![Escalation::ChainBreak {
            reason: reason.into(),
        }]
    }

    /// Record current storage usage. Returns an escalation only on
    /// transitions across the warn / critical watermarks (so the
    /// dashboard doesn't get spammed with the same severity
    /// repeatedly).
    pub fn record_storage_usage(&mut self, used: u64, capacity: u64) -> Vec<Escalation> {
        if capacity == 0 {
            return Vec::new();
        }
        let percent_u128 = (u128::from(used) * 100) / u128::from(capacity);
        let percent = u8::try_from(percent_u128.min(100)).unwrap_or(100);
        let new_severity = if percent >= self.config.critical_percent {
            Some(Severity::Critical)
        } else if percent >= self.config.warn_percent {
            Some(Severity::Warn)
        } else {
            None
        };

        if new_severity == self.last_quota_severity {
            return Vec::new();
        }
        self.last_quota_severity = new_severity;
        match new_severity {
            None => Vec::new(),
            Some(severity) => vec![Escalation::StorageQuota {
                severity,
                used,
                capacity,
                percent,
            }],
        }
    }

    /// Current sliding-window violation count (testing / dashboards).
    pub fn current_violation_count(&self) -> u32 {
        u32::try_from(self.violations.len()).unwrap_or(u32::MAX)
    }

    fn prune(&mut self, now: Instant) {
        let cutoff = now.checked_sub(Duration::from_secs(self.config.window_secs));
        while let Some(front) = self.violations.front() {
            let should_drop = match cutoff {
                Some(c) => *front < c,
                None => false,
            };
            if should_drop {
                self.violations.pop_front();
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_violation_decisions_do_not_escalate() {
        let mut t = TriggerManager::default();
        let now = Instant::now();
        let out = t.record_decision(RoutingDecision::Allow, None, now);
        assert!(out.is_empty());
        let out = t.record_decision(RoutingDecision::LocalOnly, None, now);
        assert!(out.is_empty());
        assert_eq!(t.current_violation_count(), 0);
    }

    #[test]
    fn violations_accumulate_and_escalate_at_threshold() {
        let mut t = TriggerManager::new(TriggersConfig {
            violation_threshold: 3,
            ..TriggersConfig::default()
        });
        let now = Instant::now();
        // 1, 2: no escalation.
        assert!(
            t.record_decision(RoutingDecision::Deny, None, now)
                .is_empty()
        );
        assert!(
            t.record_decision(RoutingDecision::Quarantine, None, now)
                .is_empty()
        );
        // 3: warn-level escalation.
        let out = t.record_decision(RoutingDecision::Deny, None, now);
        assert_eq!(out.len(), 1);
        let Escalation::ViolationThreshold {
            severity, count, ..
        } = &out[0]
        else {
            panic!("expected violation threshold escalation, got {:?}", out[0]);
        };
        assert_eq!(*severity, Severity::Warn);
        assert_eq!(*count, 3);
    }

    #[test]
    fn many_violations_promote_to_critical() {
        let mut t = TriggerManager::new(TriggersConfig {
            violation_threshold: 2,
            ..TriggersConfig::default()
        });
        let now = Instant::now();
        for _ in 0..4 {
            t.record_decision(RoutingDecision::Deny, None, now);
        }
        let out = t.record_decision(RoutingDecision::Deny, None, now);
        let Escalation::ViolationThreshold { severity, .. } = &out[0] else {
            panic!("expected violation threshold escalation");
        };
        assert_eq!(*severity, Severity::Critical);
    }

    #[test]
    fn old_violations_drop_out_of_window() {
        let mut t = TriggerManager::new(TriggersConfig {
            violation_threshold: 100, // ensure no escalation noise
            window_secs: 1,
            ..TriggersConfig::default()
        });
        let now = Instant::now();
        t.record_decision(RoutingDecision::Deny, None, now);
        assert_eq!(t.current_violation_count(), 1);
        // 2 seconds later — outside the 1-second window.
        let later = now + Duration::from_secs(2);
        t.record_decision(RoutingDecision::Allow, None, later);
        assert_eq!(t.current_violation_count(), 0);
    }

    #[test]
    fn policy_change_always_emits_info() {
        let t = TriggerManager::default();
        let out = t.record_policy_change("template:healthcare");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].severity(), Severity::Info);
        assert_eq!(out[0].kind(), "policy_changed");
    }

    #[test]
    fn chain_break_is_critical() {
        let t = TriggerManager::default();
        let out = t.record_chain_break("link broken at id=42");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].severity(), Severity::Critical);
    }

    #[test]
    fn storage_quota_emits_on_severity_transitions_only() {
        let mut t = TriggerManager::new(TriggersConfig {
            warn_percent: 80,
            critical_percent: 90,
            ..TriggersConfig::default()
        });
        // 50% — nothing.
        assert!(t.record_storage_usage(50, 100).is_empty());
        // 80% — warn fires.
        let warn = t.record_storage_usage(80, 100);
        assert_eq!(warn.len(), 1);
        assert_eq!(warn[0].severity(), Severity::Warn);
        // 85% — still warn, no new escalation.
        assert!(t.record_storage_usage(85, 100).is_empty());
        // 92% — promote to critical.
        let crit = t.record_storage_usage(92, 100);
        assert_eq!(crit.len(), 1);
        assert_eq!(crit[0].severity(), Severity::Critical);
        // 70% — drops back below warn; emits empty (recovery
        // surfacing is the dashboard's job).
        assert!(t.record_storage_usage(70, 100).is_empty());
    }

    #[test]
    fn zero_capacity_storage_is_a_no_op() {
        let mut t = TriggerManager::default();
        let out = t.record_storage_usage(0, 0);
        assert!(out.is_empty());
    }
}
