//! Policy decision + modes (G6).
//!
//! Reuses the **mai-compliance deny-wins composer**: PHI is detected
//! (`PhiDetector`), the HIPAA BAA rule decides whether cloud egress is permitted
//! (`BaaEnforcer::evaluate_for_cloud`), and the `PolicyComposer` folds that (and,
//! in M2, ITAR + OCAP) into one allow/deny where **any module's deny wins**. The
//! composed decision is combined with the G5 route to an `effective_route`.
//!
//! Three **modes** act on the same decision:
//! * **Shadow** — decide + log, **never block** (the M1 default; ships safe).
//! * **ReportOnly** — never block, but flag the violation for the audit surface.
//! * **Enforce** — block a request that would egress classified data to cloud.
//!
//! M1 wires the **HIPAA** module (the AWS + HIPAA beachhead). ITAR/OCAP modules
//! join in M2 — their evaluators need the full detector-input plumbing; the
//! composer already denies-wins over whatever module set it is handed.

use axum::Json;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use fabric_contracts::Route;
use mai_compliance::{BaaEnforcer, ComposerConfig, ModuleDecision, PhiDetector, PolicyComposer};
use serde_json::json;

use crate::app::{AppState, Target};
use crate::route::GatewayRoute;

/// Enforcement posture for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolicyMode {
    /// Decide + log, never block (M1 default).
    #[default]
    Shadow,
    /// Never block, but flag violations.
    ReportOnly,
    /// Block a classified-data cloud egress.
    Enforce,
}

impl PolicyMode {
    #[must_use]
    pub fn header(self) -> &'static str {
        match self {
            PolicyMode::Shadow => "shadow",
            PolicyMode::ReportOnly => "report_only",
            PolicyMode::Enforce => "enforce",
        }
    }

    /// Parse a mode name (for a settings value / override header).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "shadow" => Some(PolicyMode::Shadow),
            "report_only" | "report-only" => Some(PolicyMode::ReportOnly),
            "enforce" => Some(PolicyMode::Enforce),
            _ => None,
        }
    }
}

/// The deny-wins policy engine (mai-compliance).
pub struct PolicyEngine {
    phi: PhiDetector,
    baa: BaaEnforcer,
    composer: PolicyComposer,
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self::baseline()
    }
}

/// The composed decision for a request.
#[derive(Debug, Clone)]
pub struct PolicyDecision {
    /// Deny-wins composed result: is cloud egress permitted?
    pub allowed_cloud: bool,
    /// Any PHI was detected in the request text.
    pub phi_detected: bool,
    /// The route after combining the G5 decision with the policy decision.
    pub effective_route: Route,
    /// Human-readable reasons (populated on a deny).
    pub reasons: Vec<String>,
}

/// What a mode does with a decision for a specific dispatch target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeOutcome {
    pub block: bool,
    pub report: bool,
}

impl PolicyEngine {
    #[must_use]
    pub fn baseline() -> Self {
        Self {
            phi: PhiDetector::baseline(),
            baa: BaaEnforcer::standard(),
            composer: PolicyComposer::new(ComposerConfig::default()),
        }
    }

    /// Evaluate the deny-wins policy for request text, combined with the G5 route.
    #[must_use]
    pub fn evaluate(&self, text: &str, route: &GatewayRoute) -> PolicyDecision {
        let report = self.phi.scan(text);
        let phi_detected = report.has_any();
        let baa = self.baa.evaluate_for_cloud(&report);
        // M1 slice: the HIPAA module. The composer denies-wins over the set it is
        // given; ITAR/OCAP modules join in M2.
        let aggregate = self.composer.compose([ModuleDecision::from_hipaa(&baa)]);
        let allowed_cloud = aggregate.allowed;
        let policy_route = if allowed_cloud {
            Route::CloudAllowed
        } else {
            Route::LocalOnly
        };
        let effective_route = most_restrictive(route.route, policy_route);
        let reasons = if allowed_cloud {
            Vec::new()
        } else {
            vec![baa.reason.clone()]
        };
        PolicyDecision {
            allowed_cloud,
            phi_detected,
            effective_route,
            reasons,
        }
    }
}

/// The tighter of two routes (LocalOnly &lt; LocalPreferred &lt; CloudAllowed).
fn most_restrictive(a: Route, b: Route) -> Route {
    match (a, b) {
        (Route::LocalOnly, _) | (_, Route::LocalOnly) => Route::LocalOnly,
        (Route::LocalPreferred, _) | (_, Route::LocalPreferred) => Route::LocalPreferred,
        _ => Route::CloudAllowed,
    }
}

/// Apply a mode to a decision for a dispatch target. A **violation** is a request
/// whose effective route is local-only being dispatched to a cloud provider.
#[must_use]
pub fn apply_mode(
    decision: &PolicyDecision,
    mode: PolicyMode,
    target_is_cloud: bool,
) -> ModeOutcome {
    let violation = target_is_cloud && decision.effective_route == Route::LocalOnly;
    match mode {
        PolicyMode::Shadow => ModeOutcome {
            block: false,
            report: false,
        },
        PolicyMode::ReportOnly => ModeOutcome {
            block: false,
            report: violation,
        },
        PolicyMode::Enforce => ModeOutcome {
            block: violation,
            report: violation,
        },
    }
}

/// Is a provider target a cloud egress? The `"local"` provider stays on-prem;
/// everything else (openai/anthropic/…) is a cloud route.
#[must_use]
pub fn target_is_cloud(target: &Target) -> bool {
    target.provider != "local"
}

/// The policy gate a surface runs before dispatch. Returns the decision + outcome
/// when the request may proceed, or a ready `403` [`Response`] when enforce blocks.
///
/// The `Err` variant is intentionally a prepared HTTP `Response` (control flow,
/// not an error value) — boxing it would add indirection for no benefit.
#[allow(clippy::result_large_err)]
pub(crate) fn gate(
    state: &AppState,
    target_cloud: bool,
    query: &str,
    route: &GatewayRoute,
) -> Result<(PolicyDecision, ModeOutcome), Response> {
    let decision = state.policy.evaluate(query, route);
    let outcome = apply_mode(&decision, state.mode, target_cloud);
    if outcome.block {
        Err(blocked(&decision, state.mode))
    } else {
        Ok((decision, outcome))
    }
}

fn set_headers(
    resp: &mut Response,
    decision: &PolicyDecision,
    mode: PolicyMode,
    outcome: &ModeOutcome,
) {
    let h = resp.headers_mut();
    h.insert("x-aog-policy-mode", HeaderValue::from_static(mode.header()));
    h.insert(
        "x-aog-policy",
        HeaderValue::from_static(if decision.allowed_cloud {
            "allow"
        } else {
            "deny"
        }),
    );
    h.insert(
        "x-aog-policy-blocked",
        HeaderValue::from_static(if outcome.block { "true" } else { "false" }),
    );
    h.insert(
        "x-aog-policy-report",
        HeaderValue::from_static(if outcome.report { "true" } else { "false" }),
    );
}

/// Attach the policy decision headers to an allowed response.
pub(crate) fn tag_policy(
    mut resp: Response,
    decision: &PolicyDecision,
    mode: PolicyMode,
    outcome: &ModeOutcome,
) -> Response {
    set_headers(&mut resp, decision, mode, outcome);
    resp
}

/// A `403` for an enforce-blocked request (generic error shape both OpenAI and
/// Anthropic clients surface).
fn blocked(decision: &PolicyDecision, mode: PolicyMode) -> Response {
    let reason = decision
        .reasons
        .first()
        .cloned()
        .unwrap_or_else(|| "policy denies cloud egress for classified data".to_string());
    let body = json!({
        "error": {
            "message": format!("blocked by AOG policy (enforce): {reason}"),
            "type": "policy_denied",
            "code": "aog_enforce",
        }
    });
    let mut resp = (StatusCode::FORBIDDEN, Json(body)).into_response();
    set_headers(
        &mut resp,
        decision,
        mode,
        &ModeOutcome {
            block: true,
            report: true,
        },
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::RouteSource;
    use mai_compliance::BaaDecision;

    fn local_route() -> GatewayRoute {
        GatewayRoute {
            route: Route::LocalOnly,
            classification: "regulated".to_string(),
            reason: "phi".to_string(),
            source: RouteSource::Classified,
            denied: false,
        }
    }

    #[test]
    fn phi_denies_cloud_and_modes_differ() {
        let engine = PolicyEngine::baseline();
        let d = engine.evaluate(
            "Patient John Doe, SSN 123-45-6789, diagnosis",
            &local_route(),
        );
        assert!(d.phi_detected, "PHI detected");
        assert!(!d.allowed_cloud, "deny-wins: PHI denies cloud egress");
        assert_eq!(d.effective_route, Route::LocalOnly);

        // Same decision, cloud dispatch, three modes:
        assert!(
            !apply_mode(&d, PolicyMode::Shadow, true).block,
            "shadow NEVER blocks"
        );
        let ro = apply_mode(&d, PolicyMode::ReportOnly, true);
        assert!(!ro.block, "report-only never blocks");
        assert!(ro.report, "report-only reports the violation");
        assert!(
            apply_mode(&d, PolicyMode::Enforce, true).block,
            "enforce blocks PHI→cloud"
        );

        // A local dispatch of the same decision is never a violation.
        assert!(
            !apply_mode(&d, PolicyMode::Enforce, false).block,
            "local dispatch is not a violation"
        );
    }

    #[test]
    fn benign_allows_cloud() {
        let engine = PolicyEngine::baseline();
        let route = GatewayRoute {
            route: Route::CloudAllowed,
            classification: "public".to_string(),
            reason: String::new(),
            source: RouteSource::Classified,
            denied: false,
        };
        let d = engine.evaluate("What is the capital of France?", &route);
        assert!(d.allowed_cloud, "benign request allows cloud");
        assert!(
            !apply_mode(&d, PolicyMode::Enforce, true).block,
            "enforce does not block a benign cloud request"
        );
    }

    #[test]
    fn composer_honors_a_module_deny() {
        // The composed decision denies when its module denies (deny-wins). Multi-
        // module (ITAR/OCAP) deny-wins is the composer's own contract, reused here.
        let composer = PolicyComposer::new(ComposerConfig::default());
        let deny = BaaDecision {
            allowed: false,
            reason: "phi to cloud".into(),
            violations: vec![],
        };
        let allow = BaaDecision {
            allowed: true,
            reason: "clear".into(),
            violations: vec![],
        };
        assert!(
            !composer
                .compose([ModuleDecision::from_hipaa(&deny)])
                .allowed
        );
        assert!(
            composer
                .compose([ModuleDecision::from_hipaa(&allow)])
                .allowed
        );
    }
}
