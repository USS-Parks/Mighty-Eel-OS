//! `aog-conformance` (Phase V) — Loom's executable conformance suite: the analog
//! of the Kubernetes conformance tests, scoped to the AOG workload domain and
//! extended with WSF trust. It runs the addendum A1.12 correctness bars against a
//! reference estate and emits a machine-checkable report a customer can run
//! themselves; passing it is the gate to *claim* "Kubernetes-grade" externally
//! (A1.12 bar 8 / A5).
//!
//! Bars 1 (idempotent reconciliation) and 2 (linearizable control-plane writes)
//! are asserted green in-process against the real `aog-store` Raft state machine
//! and `aog-controller` reconcile runtime. Every other bar is registered against
//! the Phase-V prompt that implements it on the live multi-node harness and is
//! reported `pending` — never as a pass it did not run (CANON §11: honest,
//! tracked). No bar is reported green without an executed check.

mod bars;

use serde::Serialize;

/// The addendum A1.12 correctness bars — "as good as Kubernetes", gated here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BarId {
    /// Bar 1 — level-triggered, idempotent reconciliation.
    IdempotentReconcile,
    /// Bar 2 — linearizable control-plane writes; no lost updates.
    LinearizableWrites,
    /// Bar 3 — split-brain safety: a minority partition serves no authoritative write.
    SplitBrainSafety,
    /// Bar 4 — self-healing: killed workloads reschedule within SLO.
    SelfHealing,
    /// Bar 5 — rollout/rollback determinism.
    RolloutDeterminism,
    /// Bar 6 — scale target: N nodes / M workloads reconcile within SLO.
    ScaleTarget,
    /// Bar 7 — kill-switch-under-scale: revocation halts the next call on every replica.
    KillSwitchUnderScale,
    /// Bar 8 — the conformance suite itself is executable and green.
    SuiteExecutable,
}

impl BarId {
    /// The A1.12 guarantee this bar proves, in one line.
    pub fn title(self) -> &'static str {
        match self {
            Self::IdempotentReconcile => "Level-triggered, idempotent reconciliation",
            Self::LinearizableWrites => "Linearizable control-plane writes; no lost updates",
            Self::SplitBrainSafety => {
                "Split-brain safety: a minority partition serves no authoritative write"
            }
            Self::SelfHealing => "Self-healing: killed workloads reschedule within SLO",
            Self::RolloutDeterminism => "Rollout/rollback determinism",
            Self::ScaleTarget => "Scale target: N nodes / M workloads reconcile within SLO",
            Self::KillSwitchUnderScale => {
                "Kill-switch-under-scale: revocation halts the next call on every replica"
            }
            Self::SuiteExecutable => "Conformance suite is executable and green",
        }
    }
}

/// Outcome of one bar. `Pending` = registered but implemented by a later Phase-V
/// prompt on the live harness; it is never counted as a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BarStatus {
    Pass,
    Fail,
    Pending,
}

/// One bar's line in the report.
#[derive(Debug, Clone, Serialize)]
pub struct BarReport {
    pub id: BarId,
    pub title: &'static str,
    pub status: BarStatus,
    pub detail: String,
}

/// The full conformance report. Serialize to JSON for a customer or CI lane.
#[derive(Debug, Clone, Serialize)]
pub struct ConformanceReport {
    pub bars: Vec<BarReport>,
    pub passed: usize,
    pub failed: usize,
    pub pending: usize,
}

impl ConformanceReport {
    fn new(bars: Vec<BarReport>) -> Self {
        let passed = bars.iter().filter(|b| b.status == BarStatus::Pass).count();
        let failed = bars.iter().filter(|b| b.status == BarStatus::Fail).count();
        let pending = bars
            .iter()
            .filter(|b| b.status == BarStatus::Pending)
            .count();
        Self {
            bars,
            passed,
            failed,
            pending,
        }
    }

    /// Green = no asserted bar failed. Pending bars (owned by later prompts) do
    /// not fail the run.
    pub fn is_green(&self) -> bool {
        self.failed == 0
    }

    /// Summit-ready = every bar asserted and passing, zero pending (A5).
    pub fn is_summit_ready(&self) -> bool {
        self.failed == 0 && self.pending == 0
    }
}

fn pending(id: BarId, prompt: &'static str) -> BarReport {
    BarReport {
        id,
        title: id.title(),
        status: BarStatus::Pending,
        detail: format!("owned by Phase-V prompt {prompt} (live multi-node harness)"),
    }
}

fn asserted(id: BarId, result: Result<String, String>) -> BarReport {
    let (status, detail) = match result {
        Ok(detail) => (BarStatus::Pass, detail),
        Err(detail) => (BarStatus::Fail, detail),
    };
    BarReport {
        id,
        title: id.title(),
        status,
        detail,
    }
}

/// Run the conformance suite against the in-process reference estate.
pub async fn run() -> ConformanceReport {
    let idempotent = bars::idempotent_reconcile(48, 0x0BAD_F00D).await;
    // Bar 2: the deterministic CAS proof, then linearizability under concurrent
    // clients + injected partitions (V3) at a modest in-suite scale.
    let linearizable = match bars::linearizable_writes().await {
        Ok(seq) => match bars::linearizable_under_faults(3, 15, 0x0A11_5EED).await {
            Ok(conc) => Ok(format!("{seq}; under concurrency + faults, {conc}")),
            Err(e) => Err(e),
        },
        Err(e) => Err(e),
    };
    // Bar 7 (V5): a published revocation halts the next call on every replica,
    // under estate scale — asserted here at a modest in-suite scale; the full
    // aggressive-profile gate (5 replicas × 100 objects) runs in `v5_*`.
    let kill_switch = bars::kill_switch_under_scale(3, 20).await;
    // Bar 6 (V8): N replicas reconcile M workloads within SLO — modest in-suite
    // scale here; the aggressive profile (5 × 100) runs in `v8_*`.
    let scale = bars::scale_target(3, 20).await;
    let mut reports = vec![
        asserted(BarId::IdempotentReconcile, idempotent),
        asserted(BarId::LinearizableWrites, linearizable),
        pending(BarId::SplitBrainSafety, "V4"),
        pending(BarId::SelfHealing, "V7"),
        pending(BarId::RolloutDeterminism, "V7"),
        asserted(BarId::ScaleTarget, scale),
        asserted(BarId::KillSwitchUnderScale, kill_switch),
    ];
    let any_fail = reports.iter().any(|b| b.status == BarStatus::Fail);
    reports.push(BarReport {
        id: BarId::SuiteExecutable,
        title: BarId::SuiteExecutable.title(),
        status: if any_fail {
            BarStatus::Fail
        } else {
            BarStatus::Pass
        },
        detail: "conformance suite executed to completion".to_owned(),
    });
    ConformanceReport::new(reports)
}

#[cfg(test)]
mod tests {
    use super::{BarId, BarStatus, run};

    #[tokio::test]
    async fn suite_runs_green_and_asserts_linearizability() {
        let report = run().await;
        assert!(
            report.is_green(),
            "conformance suite is green on the reference estate: {report:?}"
        );
        let lin = report
            .bars
            .iter()
            .find(|b| b.id == BarId::LinearizableWrites)
            .expect("bar 2 is registered");
        assert_eq!(
            lin.status,
            BarStatus::Pass,
            "bar 2 (linearizable writes) asserted green: {}",
            lin.detail
        );
        let idem = report
            .bars
            .iter()
            .find(|b| b.id == BarId::IdempotentReconcile)
            .expect("bar 1 is registered");
        assert_eq!(
            idem.status,
            BarStatus::Pass,
            "bar 1 (idempotent reconciliation) asserted green: {}",
            idem.detail
        );
        // The remaining bars are registered pending their Phase-V owner.
        assert!(
            report.pending >= 1,
            "later bars are registered against their Phase-V prompt"
        );
    }

    #[tokio::test]
    async fn v2_reconcile_idempotency_fuzz_converges() {
        // V2 gate (standard lane): 500 randomized delivery histories (reorder /
        // duplicate / overflow-drop) converge to one authoritative end state.
        let result = crate::bars::idempotent_reconcile(500, 0x00C0_FFEE).await;
        assert!(result.is_ok(), "V2 idempotency fuzz diverged: {result:?}");
    }

    /// The full V2 gate — 10^4 histories. Heavy (durable Raft writes per history,
    /// ~minutes), so it runs in the opt-in nightly/CI lane like the workspace's
    /// other heavy tests; the standard lane above runs 500. Not a silent cap
    /// (Doctrine D8): the full count is here and runnable with `-- --ignored`.
    #[tokio::test]
    #[ignore = "nightly: 10^4-history fuzz (~minutes); standard lane runs 500"]
    async fn v2_reconcile_idempotency_fuzz_full_10k() {
        let result = crate::bars::idempotent_reconcile(10_000, 0x00C0_FFEE).await;
        assert!(
            result.is_ok(),
            "V2 idempotency fuzz diverged at 10^4: {result:?}"
        );
    }

    #[tokio::test]
    async fn v3_linearizability_under_faults() {
        // V3 gate: concurrent CAS-increment clients under injected partitions and
        // real leader failovers; acknowledged increments must be <= the final
        // counter (no lost update, no stale allow).
        let result = crate::bars::linearizable_under_faults(4, 60, 0x0A11_5EED).await;
        assert!(result.is_ok(), "V3 linearizability violation: {result:?}");
    }

    #[tokio::test]
    async fn v5_kill_switch_under_scale() {
        // V5 gate (bar 7): under a 100-object estate, a signed revocation halts
        // the next call on every one of the 5 control-plane replicas, a live
        // token is still admitted, and a rogue-signed snapshot fails closed
        // (aggressive profile: 5 replicas, 100 workloads).
        let result = crate::bars::kill_switch_under_scale(5, 100).await;
        assert!(
            result.is_ok(),
            "V5 kill-switch-under-scale failed: {result:?}"
        );
    }

    #[tokio::test]
    async fn v8_scale_target() {
        // V8 gate (bar 6): 5 control-plane replicas ingest + reconcile 100
        // workloads to convergence within SLO, and every object replicates to
        // every node (aggressive profile: 5 nodes, 100 workloads).
        let result = crate::bars::scale_target(5, 100).await;
        assert!(result.is_ok(), "V8 scale target failed: {result:?}");
    }

    #[tokio::test]
    #[allow(clippy::print_stdout)] // an SLO gate surfaces its measured p50/p99
    async fn v10_revocation_to_denial_slo() {
        // V10 gate ("the kill number"): revocation-to-denial across all 5
        // replicas has p99 <= 3s over 100 revocations, and a replica past its
        // snapshot freshness window fails closed (doctrine I-9 / RC-KILL).
        let result = crate::bars::revocation_to_denial_slo(5, 100).await;
        if let Ok(detail) = &result {
            println!("V10: {detail}");
        }
        assert!(
            result.is_ok(),
            "V10 revocation-to-denial SLO failed: {result:?}"
        );
    }

    #[tokio::test]
    #[allow(clippy::print_stdout)] // a soak gate surfaces its result summary
    async fn v7_chaos_soak() {
        // V7 gate (control-plane leg of bars 4/5): a 5-node estate survives a
        // soak of 12 kill/heal cycles — a leader re-emerges within SLO each round,
        // every killed node rejoins + catches up, and all replicas converge to the
        // one deterministic rollout end state (no committed loss). The data-plane
        // workload-reschedule leg is the live estate's (v7-chaos-soak.sh).
        let result = crate::bars::chaos_soak(5, 12, 0x0C1A_05EE).await;
        if let Ok(detail) = &result {
            println!("V7: {detail}");
        }
        assert!(result.is_ok(), "V7 chaos+soak failed: {result:?}");
    }
}
