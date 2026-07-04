//! ROI recommender (G10).
//!
//! Turns metered spend (aog-meter's per-task aggregates, G7) into a plain capital
//! decision: what the cloud costs per month, when an on-prem Summit pays for
//! itself at the current run rate, and whether to **move workloads on-prem**,
//! **upgrade** the local tier, or **stay** on cloud. Pure + deterministic — the
//! same telemetry always yields the same recommendation (the G10 gate).

use serde::Serialize;

use crate::meter::TaskUsage;

/// If a Summit pays for itself within this many months at the current cloud run
/// rate, moving on-prem is the recommendation.
const BREAK_EVEN_ATTRACTIVE_MONTHS: f64 = 18.0;
/// At or above this share of calls already served on-prem, the local tier is
/// treated as saturated (recommend more capacity rather than more migration).
const SATURATION_LOCAL_SHARE: f64 = 0.85;

/// The knobs a recommendation is computed against.
#[derive(Debug, Clone, Copy)]
pub struct RoiInputs {
    /// Amortized cost of the on-prem Summit appliance, in cents.
    pub summit_cost_cents: u64,
    /// The window the aggregates cover, in days (to derive a monthly run rate).
    pub window_days: u32,
}

impl Default for RoiInputs {
    fn default() -> Self {
        // $40k appliance, a 30-day window.
        Self {
            summit_cost_cents: 4_000_000,
            window_days: 30,
        }
    }
}

/// A break-even + utilization recommendation over the metered spend.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RoiReport {
    pub window_days: u32,
    /// Cloud spend (cents) observed in the window (local is free).
    pub cloud_spend_cents: u64,
    pub local_calls: u64,
    pub cloud_calls: u64,
    /// Cloud spend projected to a 30-day month at the window's run rate.
    pub monthly_cloud_cents: u64,
    pub summit_cost_cents: u64,
    /// Months for the Summit to pay for itself at the current cloud run rate; `None`
    /// when there is no cloud spend (nothing to recover).
    pub break_even_months: Option<f64>,
    /// Share of calls already served on-prem, in `[0, 1]`.
    pub local_share: f64,
    /// The headline call: `move_on_prem` | `upgrade_capacity` | `stay_cloud`.
    pub recommendation: String,
    /// Plain-language justification lines.
    pub reasons: Vec<String>,
}

/// Compute the ROI recommendation from metered aggregates + the Summit cost inputs.
#[must_use]
pub fn recommend(aggregates: &[TaskUsage], inputs: RoiInputs) -> RoiReport {
    let mut cloud_spend_cents = 0u64;
    let mut local_calls = 0u64;
    let mut cloud_calls = 0u64;
    for a in aggregates {
        if a.provider == "local" {
            local_calls = local_calls.saturating_add(a.calls);
        } else {
            cloud_calls = cloud_calls.saturating_add(a.calls);
            cloud_spend_cents = cloud_spend_cents.saturating_add(a.spend_cents);
        }
    }

    let window_days = inputs.window_days.max(1);
    let monthly_cloud_cents = cloud_spend_cents.saturating_mul(30) / u64::from(window_days);
    let break_even_months =
        (monthly_cloud_cents > 0).then(|| ratio(inputs.summit_cost_cents, monthly_cloud_cents));

    let total_calls = local_calls.saturating_add(cloud_calls);
    let local_share = if total_calls > 0 {
        ratio(local_calls, total_calls)
    } else {
        0.0
    };

    let (recommendation, reasons) = decide(break_even_months, local_share);
    RoiReport {
        window_days,
        cloud_spend_cents,
        local_calls,
        cloud_calls,
        monthly_cloud_cents,
        summit_cost_cents: inputs.summit_cost_cents,
        break_even_months,
        local_share,
        recommendation,
        reasons,
    }
}

/// `a / b` as an `f64` (both small enough that the precision loss is immaterial).
#[allow(clippy::cast_precision_loss)]
fn ratio(a: u64, b: u64) -> f64 {
    a as f64 / b as f64
}

/// The deterministic decision. Saturation of the local tier is checked first (more
/// migration cannot help a full tier); then an attractive break-even recommends
/// migration; otherwise stay on cloud.
fn decide(break_even_months: Option<f64>, local_share: f64) -> (String, Vec<String>) {
    let mut reasons = Vec::new();
    if local_share >= SATURATION_LOCAL_SHARE {
        reasons.push(format!(
            "{:.0}% of calls already run on-prem — the local tier is saturated; add capacity before migrating more",
            local_share * 100.0
        ));
        return ("upgrade_capacity".to_string(), reasons);
    }
    match break_even_months {
        Some(m) if m <= BREAK_EVEN_ATTRACTIVE_MONTHS => {
            reasons.push(format!(
                "at the current cloud run rate the Summit pays for itself in {m:.1} months"
            ));
            reasons.push("move the cloud-served, on-prem-capable workloads to Summit".to_string());
            ("move_on_prem".to_string(), reasons)
        }
        Some(m) => {
            reasons.push(format!(
                "break-even is {m:.1} months, beyond the {BREAK_EVEN_ATTRACTIVE_MONTHS:.0}-month bar — cloud stays cheaper for now"
            ));
            ("stay_cloud".to_string(), reasons)
        }
        None => {
            reasons.push("no cloud spend recorded — nothing to move on-prem yet".to_string());
            ("stay_cloud".to_string(), reasons)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(provider: &str, calls: u64, spend_cents: u64) -> TaskUsage {
        TaskUsage {
            tenant_id: "t".to_string(),
            provider: provider.to_string(),
            model: "m".to_string(),
            workflow_id: None,
            calls,
            input_tokens: 0,
            output_tokens: 0,
            spend_cents,
        }
    }

    #[test]
    fn attractive_break_even_recommends_moving_on_prem() {
        // $40k Summit, $5k/month cloud over the window → 8-month break-even.
        let agg = vec![usage("openai", 1000, 500_000), usage("local", 100, 0)];
        let r = recommend(
            &agg,
            RoiInputs {
                summit_cost_cents: 4_000_000,
                window_days: 30,
            },
        );
        assert_eq!(r.monthly_cloud_cents, 500_000);
        assert_eq!(r.break_even_months, Some(8.0));
        assert_eq!(r.recommendation, "move_on_prem");
        assert!(r.reasons[0].contains("8.0 months"));
    }

    #[test]
    fn slow_break_even_stays_on_cloud() {
        // $40k Summit, only $1k/month cloud → 40-month break-even (beyond 18).
        let agg = vec![usage("openai", 50, 100_000), usage("local", 10, 0)];
        let r = recommend(
            &agg,
            RoiInputs {
                summit_cost_cents: 4_000_000,
                window_days: 30,
            },
        );
        assert_eq!(r.break_even_months, Some(40.0));
        assert_eq!(r.recommendation, "stay_cloud");
    }

    #[test]
    fn saturated_local_tier_recommends_upgrade() {
        // 90% of calls already local → saturation wins even with cloud spend.
        let agg = vec![usage("local", 900, 0), usage("openai", 100, 500_000)];
        let r = recommend(&agg, RoiInputs::default());
        assert!(r.local_share >= 0.85);
        assert_eq!(r.recommendation, "upgrade_capacity");
    }

    #[test]
    fn all_local_estate_has_no_break_even_and_reads_saturated() {
        let agg = vec![usage("local", 500, 0)];
        let r = recommend(&agg, RoiInputs::default());
        assert_eq!(r.break_even_months, None);
        assert_eq!(r.cloud_spend_cents, 0);
        // 100% local (share = 1.0) is past the saturation bar → upgrade, not migrate.
        assert_eq!(r.recommendation, "upgrade_capacity");
    }

    #[test]
    fn empty_telemetry_is_stable() {
        let r = recommend(&[], RoiInputs::default());
        assert_eq!(r.local_share, 0.0);
        assert_eq!(r.break_even_months, None);
        assert_eq!(r.recommendation, "stay_cloud");
        assert!(r.reasons[0].contains("no cloud spend"));
    }
}
