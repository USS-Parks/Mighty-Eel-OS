//! Per-profile cost / token-budget tracker.
//!
//! Cloud routing costs money. This module enforces per-profile monthly token
//! budgets with soft and hard caps:
//!
//! - **Soft cap** (default 80% of budget): request is allowed but flagged
//!   for audit so operators know a profile is approaching its limit.
//! - **Hard cap** (100%): request is forced to local routing regardless of
//!   the router's preferred decision.
//!
//! Budgets are configured per role. The tracker stores running per-profile
//! usage and resets at the configured period boundary.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Budget configuration, loaded from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Monthly token budget per role. Missing roles use `default_budget`.
    #[serde(default)]
    pub per_role: HashMap<String, u64>,
    /// Fallback budget when the role is unknown.
    #[serde(default = "default_budget")]
    pub default_budget: u64,
    /// Fraction of budget at which to flag (soft cap), in `[0.0, 1.0]`.
    #[serde(default = "default_soft_cap")]
    pub soft_cap_fraction: f64,
}

fn default_budget() -> u64 {
    100_000
}

fn default_soft_cap() -> f64 {
    0.8
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            per_role: HashMap::from([
                ("admin".to_string(), 1_000_000),
                ("adult".to_string(), 100_000),
                ("teen".to_string(), 50_000),
                ("child".to_string(), 10_000),
                ("guest".to_string(), 5_000),
            ]),
            default_budget: default_budget(),
            soft_cap_fraction: default_soft_cap(),
        }
    }
}

/// Result of a budget check for an incoming request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetCheck {
    /// Below the soft cap — proceed quietly.
    Ok {
        /// Remaining tokens this period.
        remaining: u64,
    },
    /// At or above the soft cap — proceed but flag for audit.
    SoftCapReached {
        /// Remaining tokens this period.
        remaining: u64,
        /// Configured soft-cap threshold.
        threshold: u64,
    },
    /// Hard cap would be exceeded — must force local routing.
    HardCapExceeded {
        /// Tokens already used this period.
        used: u64,
        /// Configured ceiling.
        budget: u64,
        /// Tokens requested by this call.
        requested: u64,
    },
}

/// Errors that prevent the tracker from operating.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BudgetError {
    /// Caller asked for zero or a negative-equivalent token count.
    #[error("requested token count must be > 0")]
    InvalidRequest,
}

/// Per-profile usage tracker.
#[derive(Debug)]
pub struct BudgetTracker {
    config: BudgetConfig,
    usage: Mutex<HashMap<String, u64>>,
}

impl BudgetTracker {
    /// Build a tracker.
    pub fn new(config: BudgetConfig) -> Self {
        Self {
            config,
            usage: Mutex::new(HashMap::new()),
        }
    }

    /// Default tracker with the per-role baseline budgets.
    pub fn with_defaults() -> Self {
        Self::new(BudgetConfig::default())
    }

    /// Resolve the budget for a role (with fallback).
    pub fn budget_for(&self, role: &str) -> u64 {
        self.config
            .per_role
            .get(role)
            .copied()
            .unwrap_or(self.config.default_budget)
    }

    /// Check whether `requested` tokens can be spent by `profile_id` under
    /// `role`. Does not mutate usage — call `record` after the request
    /// completes (so failed cloud calls don't burn budget).
    pub fn check(
        &self,
        profile_id: &str,
        role: &str,
        requested: u64,
    ) -> Result<BudgetCheck, BudgetError> {
        if requested == 0 {
            return Err(BudgetError::InvalidRequest);
        }
        let budget = self.budget_for(role);
        let used = *self.usage.lock().unwrap().get(profile_id).unwrap_or(&0);
        let projected = used.saturating_add(requested);

        if projected > budget {
            return Ok(BudgetCheck::HardCapExceeded {
                used,
                budget,
                requested,
            });
        }
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let threshold = (budget as f64 * self.config.soft_cap_fraction) as u64;
        if projected >= threshold {
            let remaining = budget.saturating_sub(projected);
            return Ok(BudgetCheck::SoftCapReached {
                remaining,
                threshold,
            });
        }
        Ok(BudgetCheck::Ok {
            remaining: budget.saturating_sub(projected),
        })
    }

    /// Commit token consumption to a profile's running usage.
    pub fn record(&self, profile_id: &str, tokens: u64) {
        let mut usage = self.usage.lock().unwrap();
        *usage.entry(profile_id.to_string()).or_insert(0) = usage
            .get(profile_id)
            .copied()
            .unwrap_or(0)
            .saturating_add(tokens);
    }

    /// Reset every profile's running usage to zero. Call at the start of a
    /// new billing period.
    pub fn reset(&self) {
        self.usage.lock().unwrap().clear();
    }

    /// Look up a profile's current usage.
    pub fn usage_for(&self, profile_id: &str) -> u64 {
        *self.usage.lock().unwrap().get(profile_id).unwrap_or(&0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> BudgetTracker {
        BudgetTracker::with_defaults()
    }

    #[test]
    fn test_check_with_no_usage_is_ok() {
        let t = tracker();
        let check = t.check("alice", "adult", 100).unwrap();
        assert!(matches!(check, BudgetCheck::Ok { .. }));
    }

    #[test]
    fn test_zero_request_errors() {
        let t = tracker();
        assert_eq!(
            t.check("alice", "adult", 0),
            Err(BudgetError::InvalidRequest)
        );
    }

    #[test]
    fn test_soft_cap_triggers_at_80_percent() {
        let t = tracker();
        // Adult budget is 100_000; soft cap at 80_000.
        t.record("alice", 79_999);
        let check = t.check("alice", "adult", 2).unwrap();
        assert!(matches!(check, BudgetCheck::SoftCapReached { .. }));
    }

    #[test]
    fn test_hard_cap_blocks_request() {
        let t = tracker();
        t.record("alice", 99_999);
        let check = t.check("alice", "adult", 100).unwrap();
        assert!(matches!(check, BudgetCheck::HardCapExceeded { .. }));
    }

    #[test]
    fn test_unknown_role_uses_default_budget() {
        let t = tracker();
        assert_eq!(t.budget_for("phantom"), default_budget());
    }

    #[test]
    fn test_record_accumulates_and_reset_clears() {
        let t = tracker();
        t.record("alice", 100);
        t.record("alice", 250);
        assert_eq!(t.usage_for("alice"), 350);
        t.reset();
        assert_eq!(t.usage_for("alice"), 0);
    }

    #[test]
    fn test_per_role_budgets_differ() {
        let t = tracker();
        assert!(t.budget_for("admin") > t.budget_for("guest"));
        assert!(t.budget_for("adult") > t.budget_for("child"));
    }

    #[test]
    fn test_failed_cloud_call_does_not_burn_budget() {
        // The check/record split lets callers gate on check() and only call
        // record() after a successful cloud call. Simulate a failed call.
        let t = tracker();
        let _ = t.check("alice", "adult", 500).unwrap();
        // Failure: do not record.
        assert_eq!(t.usage_for("alice"), 0);
    }
}
