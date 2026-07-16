//! Policy decision + modes (G6).
//!
//! Reuses the **mai-compliance deny-wins composer**: PHI is detected
//! (`PhiDetector`), the HIPAA BAA rule decides whether cloud egress is permitted
//! (`BaaEnforcer::evaluate_for_cloud`), and the `PolicyComposer` folds that (and,
//! in M2, ITAR + OCAP) into one allow/deny where **any module's deny wins**. The
//! composed decision is combined with the G5 route to an `effective_route`.
//!
//! Three **modes** act on the same decision:
//! * **Shadow** — decide + log, **never block** (development-only; explicit opt-in).
//! * **ReportOnly** — never block, but flag the violation (development-only; explicit).
//! * **Enforce** — block a classified-data cloud egress (the fail-closed default).
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

use crate::app::Target;
use crate::route::GatewayRoute;

/// Enforcement posture for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolicyMode {
    /// Decide + log, never block. Development-only; must be selected explicitly.
    Shadow,
    /// Never block, but flag violations. Development-only; must be selected explicitly.
    ReportOnly,
    /// Block a classified-data cloud egress. The fail-closed default.
    #[default]
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

/// Deployment profile. Production is strict/fail-closed; development may opt into
/// the non-blocking modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    /// Strict posture: the non-blocking modes are unavailable. The fail-safe default.
    #[default]
    Production,
    /// Development posture: `shadow` / `report_only` may be selected explicitly.
    Development,
}

impl Profile {
    /// Parse the `AOG_PROFILE` value. Absent or blank resolves to `Production`
    /// (fail-safe: development must be opted into explicitly).
    pub fn parse(value: Option<&str>) -> Result<Self, ModeError> {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("production") | Some("prod") => Ok(Profile::Production),
            Some("development") | Some("dev") => Ok(Profile::Development),
            Some(other) => Err(ModeError::UnknownProfile(other.to_string())),
        }
    }

    #[must_use]
    pub fn header(self) -> &'static str {
        match self {
            Profile::Production => "production",
            Profile::Development => "development",
        }
    }
}

/// Why policy-mode resolution failed. Every variant fails startup before bind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeError {
    /// `AOG_MODE` was an unrecognized value.
    UnrecognizedMode(String),
    /// A non-blocking mode (`shadow` / `report_only`) was requested under the
    /// production profile.
    NonBlockingInProduction(String),
    /// `AOG_PROFILE` was an unrecognized value.
    UnknownProfile(String),
}

impl std::fmt::Display for ModeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModeError::UnrecognizedMode(v) => {
                write!(
                    f,
                    "unrecognized AOG_MODE '{v}' (expected: shadow | report_only | enforce)"
                )
            }
            ModeError::NonBlockingInProduction(v) => write!(
                f,
                "AOG_MODE '{v}' is a development-only non-blocking mode; the production profile \
                 requires 'enforce' (set AOG_PROFILE=development to use it)"
            ),
            ModeError::UnknownProfile(v) => {
                write!(
                    f,
                    "unrecognized AOG_PROFILE '{v}' (expected: production | development)"
                )
            }
        }
    }
}

impl std::error::Error for ModeError {}

/// Resolve the effective policy mode from a profile and the raw `AOG_MODE` value.
///
/// Fail-closed:
/// - An absent or blank `AOG_MODE` resolves to `Enforce` in every profile.
/// - `shadow` / `report_only` are development-only and must be selected
///   explicitly; requesting either under `Production` is an error, so startup
///   fails before the listener binds.
/// - An unrecognized value is an error.
///
/// # Errors
/// Returns [`ModeError`] for an unrecognized mode or a non-blocking mode under
/// the production profile.
pub fn resolve_mode(profile: Profile, aog_mode: Option<&str>) -> Result<PolicyMode, ModeError> {
    let raw = aog_mode.map(str::trim).unwrap_or_default();
    if raw.is_empty() {
        return Ok(PolicyMode::Enforce); // fail-closed default — never silent shadow
    }
    let mode = PolicyMode::parse(&raw.to_ascii_lowercase())
        .ok_or_else(|| ModeError::UnrecognizedMode(raw.to_string()))?;
    match (profile, mode) {
        (Profile::Production, PolicyMode::Shadow | PolicyMode::ReportOnly) => {
            Err(ModeError::NonBlockingInProduction(raw.to_string()))
        }
        _ => Ok(mode),
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
        // Fail closed when no compliance module actually vetted the request — e.g.
        // the HIPAA module is disabled, so the composer dropped its decision and
        // `allowed` is vacuously true over the empty set. An unvetted request is
        // not cloud-eligible; route it local rather than egress potential PHI
        // (audit G1).
        let allowed_cloud = aggregate.allowed && !aggregate.modules_applied.is_empty();
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

fn set_headers(
    resp: &mut Response,
    decision: &PolicyDecision,
    mode: PolicyMode,
    outcome: ModeOutcome,
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
    outcome: ModeOutcome,
) -> Response {
    set_headers(&mut resp, decision, mode, outcome);
    resp
}

/// A `403` for an enforce-blocked request (generic error shape both OpenAI and
/// Anthropic clients surface).
pub(crate) fn blocked(decision: &PolicyDecision, mode: PolicyMode) -> Response {
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
        ModeOutcome {
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

    #[test]
    fn default_policy_mode_is_enforce() {
        assert_eq!(PolicyMode::default(), PolicyMode::Enforce);
        assert_eq!(Profile::default(), Profile::Production);
    }

    #[test]
    fn resolve_mode_absent_or_blank_defaults_to_enforce() {
        for raw in [None, Some(""), Some("   "), Some("\t")] {
            assert_eq!(
                resolve_mode(Profile::Production, raw).unwrap(),
                PolicyMode::Enforce
            );
            assert_eq!(
                resolve_mode(Profile::Development, raw).unwrap(),
                PolicyMode::Enforce
            );
        }
    }

    #[test]
    fn resolve_mode_enforce_allowed_in_both_profiles() {
        assert_eq!(
            resolve_mode(Profile::Production, Some("enforce")).unwrap(),
            PolicyMode::Enforce
        );
        assert_eq!(
            resolve_mode(Profile::Development, Some("enforce")).unwrap(),
            PolicyMode::Enforce
        );
    }

    #[test]
    fn resolve_mode_shadow_and_report_are_development_only() {
        for m in ["shadow", "report_only", "report-only"] {
            assert!(
                matches!(
                    resolve_mode(Profile::Production, Some(m)),
                    Err(ModeError::NonBlockingInProduction(_))
                ),
                "production must reject non-blocking mode {m:?}"
            );
            assert!(
                resolve_mode(Profile::Development, Some(m)).is_ok(),
                "development must accept explicit {m:?}"
            );
        }
    }

    #[test]
    fn resolve_mode_is_case_insensitive_but_never_coerces_to_shadow() {
        assert_eq!(
            resolve_mode(Profile::Production, Some("ENFORCE")).unwrap(),
            PolicyMode::Enforce
        );
        assert_eq!(
            resolve_mode(Profile::Development, Some("Shadow")).unwrap(),
            PolicyMode::Shadow
        );
        // A mixed-case shadow is still rejected in production — never silently coerced.
        assert!(resolve_mode(Profile::Production, Some("ShAdOw")).is_err());
    }

    #[test]
    fn resolve_mode_rejects_unrecognized_values() {
        for bad in ["audit", "block-everything", "on", "1"] {
            assert!(
                matches!(
                    resolve_mode(Profile::Development, Some(bad)),
                    Err(ModeError::UnrecognizedMode(_))
                ),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn profile_parse_defaults_and_variants() {
        assert_eq!(Profile::parse(None).unwrap(), Profile::Production);
        assert_eq!(Profile::parse(Some("")).unwrap(), Profile::Production);
        assert_eq!(
            Profile::parse(Some("production")).unwrap(),
            Profile::Production
        );
        assert_eq!(Profile::parse(Some("PROD")).unwrap(), Profile::Production);
        assert_eq!(
            Profile::parse(Some("development")).unwrap(),
            Profile::Development
        );
        assert_eq!(Profile::parse(Some("dev")).unwrap(), Profile::Development);
        assert!(matches!(
            Profile::parse(Some("staging")),
            Err(ModeError::UnknownProfile(_))
        ));
    }

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

    #[test]
    fn disabled_module_is_unvetted_not_cloud_allowed() {
        // Audit G1: with the only compliance module disabled, the composer drops its
        // decision, leaving an empty set whose `allowed` is vacuously true. The
        // enforce guard must treat that as NOT cloud-allowed — an unvetted request
        // never egresses.
        let mut cfg = ComposerConfig::default();
        cfg.enabled.remove(&mai_compliance::ModuleId::Hipaa);
        let composer = PolicyComposer::new(cfg);
        let clear = BaaDecision {
            allowed: true,
            reason: "clear".into(),
            violations: vec![],
        };
        let aggregate = composer.compose([ModuleDecision::from_hipaa(&clear)]);
        // Precondition: HIPAA disabled -> nothing vetted, vacuous allow.
        assert!(aggregate.modules_applied.is_empty());
        assert!(aggregate.allowed);
        // The G1 enforce guard (mirrors `evaluate`): a vacuous allow over an empty
        // set is not cloud-eligible.
        let allowed_cloud = aggregate.allowed && !aggregate.modules_applied.is_empty();
        assert!(
            !allowed_cloud,
            "an unvetted request must not be cloud-allowed"
        );
    }
}
