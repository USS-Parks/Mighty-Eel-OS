//! Jurisdiction determination and country-based routing (+
//! trust integration).
//!
//! Combines the outputs of [`itar`](crate::itar) and [`ear`](crate::ear)
//! into a single [`UnifiedClassification`] and applies country / person
//! rules, plus the Trust Manifold [`TrustContext`](crate::trust::TrustContext)
//! checks (scope, allowed-route ceiling, revocation, offline mode), to
//! produce a [`JurisdictionDecision`]:
//!
//! - **Allow** — the query may proceed to its requested target.
//! - **RouteLocal** — controlled content or trust ceiling; must stay
//!   on the local appliance.
//! - **DenyExport** — actor is not eligible (non-US person, blocked
//!   country, revoked claim, or compliance scope missing).
//!
//! Ambiguity rule: when neither module produced a high-confidence
//! signal but ITAR's
//! [`default_to_itar_on_ambiguity`](crate::itar::ItarDetectorConfig::default_to_itar_on_ambiguity)
//! promoted the report to `Itar`, the unified classification keeps it
//! as `Itar`. This satisfies the most-restrictive default in the
//! acceptance criteria.
//!
//! Order of checks (highest-priority deny first):
//!
//! 1. Trust revoked → DenyExport.
//! 2. Country blocklist hit → DenyExport.
//! 3. ITAR scope missing on an ITAR query → DenyExport.
//! 4. Unknown revocation on an ITAR query → DenyExport (most-restrictive
//!    default; see `docs/compliance/SERVICE-IDENTITY.md` §4.5).
//! 5. ITAR country / person gate (existing).
//! 6. Allowed-route ceiling collapses Allow → RouteLocal.
//! 7. Offline mode collapses Allow → RouteLocal.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ear::{EarClassification, EarReport};
use crate::itar::{ExportClassification, ItarReport};
use crate::trust::{
    AllowedRoute, ComplianceScope, RevocationStatus, ServiceIdentity, SubjectHash, TenantId,
    TrustContext,
};

/// Two-letter ISO 3166-1 alpha-2 country code, uppercased and
/// validated at construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CountryCode(String);

impl CountryCode {
    /// Build a country code from a two-letter input.
    pub fn new(code: impl Into<String>) -> Result<Self, JurisdictionError> {
        let raw = code.into().trim().to_uppercase();
        if raw.len() != 2 || !raw.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(JurisdictionError::InvalidCountryCode(raw));
        }
        Ok(Self(raw))
    }

    /// Borrowed view of the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Constant for the United States.
    pub fn us() -> Self {
        Self("US".to_string())
    }
}

/// Person classification for ITAR purposes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonType {
    /// US citizen or lawful permanent resident.
    UsPerson,
    /// Anyone else (foreign national without permanent residency).
    NonUsPerson,
    /// Identity not asserted by the deployment.
    #[default]
    Unknown,
}

impl PersonType {
    /// True when the actor is permitted to access ITAR-controlled
    /// content (US person only).
    pub fn is_itar_eligible(self) -> bool {
        matches!(self, Self::UsPerson)
    }
}

/// Description of the requesting actor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActorContext {
    /// ISO country code, if asserted.
    pub country: Option<CountryCode>,
    /// Person classification.
    #[serde(default = "default_person_type")]
    pub person_type: PersonType,
    /// Free-form deployment profile id used to select rule overlays
    /// (e.g. `"defense"`, `"healthcare-defense"`).
    pub deployment_profile: Option<String>,
}

fn default_person_type() -> PersonType {
    PersonType::Unknown
}

/// Country-based routing rules. Operators tune via TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JurisdictionConfig {
    /// Allowed countries for ITAR content. Defaults to `{"US"}`.
    #[serde(default = "default_itar_allowed")]
    pub itar_allowed_countries: BTreeSet<CountryCode>,
    /// Countries explicitly blocked for ALL controlled content
    /// (e.g. embargoed destinations). Defaults to empty; deployments
    /// should populate from current OFAC / BIS lists.
    #[serde(default)]
    pub blocked_countries: BTreeSet<CountryCode>,
    /// When `true` (default), an `Unknown` person type is treated as a
    /// non-US person for ITAR purposes (fail-closed).
    #[serde(default = "default_unknown_fail_closed")]
    pub unknown_actor_fails_closed: bool,
    /// When `true`, missing actor country is treated as blocked for
    /// ITAR. Defaults to `true` (fail-closed).
    #[serde(default = "default_unknown_country_fail_closed")]
    pub missing_country_fails_closed: bool,
}

fn default_itar_allowed() -> BTreeSet<CountryCode> {
    let mut set = BTreeSet::new();
    set.insert(CountryCode::us());
    set
}

fn default_unknown_fail_closed() -> bool {
    true
}

fn default_unknown_country_fail_closed() -> bool {
    true
}

impl Default for JurisdictionConfig {
    fn default() -> Self {
        Self {
            itar_allowed_countries: default_itar_allowed(),
            blocked_countries: BTreeSet::new(),
            unknown_actor_fails_closed: default_unknown_fail_closed(),
            missing_country_fails_closed: default_unknown_country_fail_closed(),
        }
    }
}

/// Jurisdiction errors.
#[derive(Debug, Error)]
pub enum JurisdictionError {
    /// Country code did not pass two-letter ASCII validation.
    #[error("invalid ISO country code: '{0}'")]
    InvalidCountryCode(String),
}

/// Merged classification across ITAR + EAR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnifiedClassification {
    /// Classification surfaced by the ITAR module.
    pub from_itar: ExportClassification,
    /// Classification surfaced by the EAR module.
    pub from_ear: EarClassification,
    /// Effective level after `max(itar, ear_mapped_to_export)`.
    pub effective: ExportClassification,
    /// True when the effective level was driven by the
    /// default-to-ITAR-on-ambiguity rule.
    pub defaulted_to_itar_on_ambiguity: bool,
}

impl UnifiedClassification {
    /// Merge the two module reports. The rule is: ITAR beats CCL beats
    /// EAR99 beats Uncontrolled.
    pub fn merge(itar: &ItarReport, ear: &EarReport) -> Self {
        let from_ear_export = match ear.classification {
            EarClassification::Uncontrolled => ExportClassification::Uncontrolled,
            EarClassification::Ear99 => ExportClassification::Ear99,
            EarClassification::Ccl => ExportClassification::Ccl,
        };
        let effective = itar.classification.max(from_ear_export);
        Self {
            from_itar: itar.classification,
            from_ear: ear.classification,
            effective,
            defaulted_to_itar_on_ambiguity: itar.defaulted_to_itar_on_ambiguity,
        }
    }

    /// True when the effective classification is `Itar`.
    pub fn is_itar(&self) -> bool {
        self.effective == ExportClassification::Itar
    }

    /// True when the effective classification requires special handling
    /// (anything above `Uncontrolled`).
    pub fn is_controlled(&self) -> bool {
        self.effective > ExportClassification::Uncontrolled
    }
}

/// Final routing decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Query may proceed to the requested target.
    Allow,
    /// Controlled content; must stay local regardless of actor.
    RouteLocal,
    /// Actor is not eligible to access ITAR content. Block entirely.
    DenyExport,
}

/// Audit-grade snapshot of the trust context at decision time. The
/// raw `subject_id` is intentionally excluded — only the HMAC
/// [`SubjectHash`] is retained so the audit layer
/// can record the decision without leaking PII.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrustSnapshot {
    /// Claim id; primary audit correlation key.
    pub claim_id: String,
    /// Version of the policy bundle the claim was issued against.
    pub trust_bundle_version: String,
    /// Tenant the subject belongs to.
    pub tenant_id: TenantId,
    /// HMAC of the subject id.
    pub subject_hash: SubjectHash,
    /// Present for service-to-service claims.
    pub service_identity: Option<ServiceIdentity>,
    /// True when the appliance was operating offline at decision time.
    pub offline_mode: bool,
    /// Revocation status of the claim at decision time.
    pub revocation_status: RevocationStatus,
}

impl TrustSnapshot {
    fn from_context(ctx: &TrustContext) -> Self {
        Self {
            claim_id: ctx.claim_id.clone(),
            trust_bundle_version: ctx.trust_bundle_version.clone(),
            tenant_id: ctx.tenant_id.clone(),
            subject_hash: ctx.subject_hash.clone(),
            service_identity: ctx.service_identity,
            offline_mode: ctx.offline_mode(),
            revocation_status: ctx.revocation_status,
        }
    }
}

/// Result of evaluating a query.
#[derive(Debug, Clone, Serialize)]
pub struct JurisdictionDecision {
    /// Routing outcome.
    pub outcome: Outcome,
    /// Merged ITAR + EAR classification.
    pub classification: UnifiedClassification,
    /// Human-readable summary for the audit log.
    pub reason: String,
    /// Identifier of the rule that fired (e.g. `"itar.non_us_person"`).
    pub matched_rule: Option<String>,
    /// Audit-grade trust snapshot recorded at decision time.
    pub trust: TrustSnapshot,
}

/// Pure evaluator. Build once, reuse across requests.
#[derive(Debug, Clone, Default)]
pub struct JurisdictionEvaluator {
    config: JurisdictionConfig,
}

impl JurisdictionEvaluator {
    /// Build an evaluator with the given config.
    pub fn new(config: JurisdictionConfig) -> Self {
        Self { config }
    }

    /// Evaluate a request.
    ///
    /// The `trust` argument carries the Trust Manifold projection
    /// Until the verified-claim pipeline lands
    /// callers may construct it via [`TrustContext::for_local_dev`]
    /// for tests and bring-up.
    pub fn evaluate(
        &self,
        itar: &ItarReport,
        ear: &EarReport,
        actor: &ActorContext,
        trust: &TrustContext,
    ) -> JurisdictionDecision {
        let classification = UnifiedClassification::merge(itar, ear);
        let snapshot = TrustSnapshot::from_context(trust);

        // 1. Hard trust deny: revoked claim.
        if trust.is_revoked() {
            return JurisdictionDecision {
                outcome: Outcome::DenyExport,
                classification,
                reason: "Claim is revoked; request denied.".to_string(),
                matched_rule: Some("trust.revoked".to_string()),
                trust: snapshot,
            };
        }

        // 2. Country block list overrides everything else.
        if let Some(country) = &actor.country
            && self.config.blocked_countries.contains(country)
        {
            return JurisdictionDecision {
                outcome: Outcome::DenyExport,
                classification,
                reason: format!(
                    "Actor country '{}' is on the blocked-countries list.",
                    country.as_str()
                ),
                matched_rule: Some("jurisdiction.blocked_country".to_string()),
                trust: snapshot,
            };
        }

        match classification.effective {
            ExportClassification::Uncontrolled => self.finalise_allowed(
                classification,
                snapshot,
                trust,
                "No export-controlled content detected.",
            ),
            ExportClassification::Ear99 => JurisdictionDecision {
                outcome: Outcome::RouteLocal,
                classification,
                reason: "EAR99 content detected; routing local by default.".to_string(),
                matched_rule: Some("ear99.route_local".to_string()),
                trust: snapshot,
            },
            ExportClassification::Ccl => JurisdictionDecision {
                outcome: Outcome::RouteLocal,
                classification,
                reason: "CCL/ECCN content detected; routing local.".to_string(),
                matched_rule: Some("ccl.route_local".to_string()),
                trust: snapshot,
            },
            ExportClassification::Itar => {
                self.evaluate_itar(classification, actor, trust, snapshot)
            }
        }
    }

    /// Apply the allowed-route ceiling and offline-mode collapse to a
    /// nominally-allowed (Uncontrolled) decision.
    fn finalise_allowed(
        &self,
        classification: UnifiedClassification,
        snapshot: TrustSnapshot,
        trust: &TrustContext,
        base_reason: &str,
    ) -> JurisdictionDecision {
        // Offline mode collapses Allow → RouteLocal.
        if trust.offline_mode() {
            return JurisdictionDecision {
                outcome: Outcome::RouteLocal,
                classification,
                reason: format!("{base_reason} Appliance is in offline mode; routing local."),
                matched_rule: Some("trust.offline_mode".to_string()),
                trust: snapshot,
            };
        }

        // Allowed-route ceiling: if the claim does not authorise a
        // cloud route, collapse to RouteLocal.
        if !trust.allowed_routes.contains(&AllowedRoute::CloudAllowed) {
            return JurisdictionDecision {
                outcome: Outcome::RouteLocal,
                classification,
                reason: format!(
                    "{base_reason} Claim allowed_routes does not include cloud_allowed; routing local."
                ),
                matched_rule: Some("trust.allowed_routes".to_string()),
                trust: snapshot,
            };
        }

        JurisdictionDecision {
            outcome: Outcome::Allow,
            classification,
            reason: base_reason.to_string(),
            matched_rule: None,
            trust: snapshot,
        }
    }

    fn evaluate_itar(
        &self,
        classification: UnifiedClassification,
        actor: &ActorContext,
        trust: &TrustContext,
        snapshot: TrustSnapshot,
    ) -> JurisdictionDecision {
        // ITAR engine may only evaluate if the claim grants the scope.
        // Absence means the tenant did not license ITAR/EAR handling at
        // all — fail closed with a distinct rule tag.
        if !trust.allows_scope(ComplianceScope::ItarEar) {
            return JurisdictionDecision {
                outcome: Outcome::DenyExport,
                classification,
                reason:
                    "ITAR content detected but claim does not grant the itar_ear compliance scope."
                        .to_string(),
                matched_rule: Some("trust.scope_missing".to_string()),
                trust: snapshot,
            };
        }

        // Most-restrictive default: ITAR + unknown revocation collapses
        // to DenyExport. See SERVICE-IDENTITY.md §4.5.
        if trust.revocation_unknown() {
            return JurisdictionDecision {
                outcome: Outcome::DenyExport,
                classification,
                reason:
                    "ITAR content with unknown revocation status; treating as revoked under the most-restrictive rule."
                        .to_string(),
                matched_rule: Some("trust.revocation_unknown_for_itar".to_string()),
                trust: snapshot,
            };
        }

        // Country gate.
        let country_ok = match &actor.country {
            Some(c) => self.config.itar_allowed_countries.contains(c),
            None => !self.config.missing_country_fails_closed,
        };
        if !country_ok {
            let country_str = actor
                .country
                .as_ref()
                .map(|c| c.as_str().to_string())
                .unwrap_or_else(|| "<unspecified>".to_string());
            return JurisdictionDecision {
                outcome: Outcome::DenyExport,
                classification,
                reason: format!(
                    "ITAR content; actor country '{}' is not in the allowed set.",
                    country_str,
                ),
                matched_rule: Some("itar.country_gate".to_string()),
                trust: snapshot,
            };
        }

        // Person gate. Unknown → fail-closed by default.
        let person_ok = match actor.person_type {
            PersonType::UsPerson => true,
            PersonType::NonUsPerson => false,
            PersonType::Unknown => !self.config.unknown_actor_fails_closed,
        };
        if !person_ok {
            return JurisdictionDecision {
                outcome: Outcome::DenyExport,
                classification,
                reason: format!(
                    "ITAR content; actor person type is '{}' and not eligible.",
                    match actor.person_type {
                        PersonType::UsPerson => "us_person",
                        PersonType::NonUsPerson => "non_us_person",
                        PersonType::Unknown => "unknown",
                    },
                ),
                matched_rule: Some("itar.non_us_person".to_string()),
                trust: snapshot,
            };
        }

        // Allowed actor — but ITAR must still stay local.
        let reason = if classification.defaulted_to_itar_on_ambiguity {
            "Ambiguous content defaulted to ITAR (most restrictive); routing local.".to_string()
        } else {
            "ITAR content; eligible actor — routing local.".to_string()
        };
        JurisdictionDecision {
            outcome: Outcome::RouteLocal,
            classification,
            reason,
            matched_rule: Some("itar.route_local".to_string()),
            trust: snapshot,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ear::EarDetector;
    use crate::itar::ItarDetector;

    fn reports(text: &str) -> (ItarReport, EarReport) {
        (
            ItarDetector::baseline().scan(text),
            EarDetector::baseline().scan(text),
        )
    }

    fn actor_us() -> ActorContext {
        ActorContext {
            country: Some(CountryCode::us()),
            person_type: PersonType::UsPerson,
            deployment_profile: None,
        }
    }

    fn actor_non_us() -> ActorContext {
        ActorContext {
            country: Some(CountryCode::new("DE").unwrap()),
            person_type: PersonType::NonUsPerson,
            deployment_profile: None,
        }
    }

    fn evaluator() -> JurisdictionEvaluator {
        JurisdictionEvaluator::default()
    }

    /// Permissive trust context for tests that exercise the
    /// classification / actor paths without trust gating.
    fn trust_permissive() -> TrustContext {
        TrustContext::for_local_dev()
    }

    #[test]
    fn test_neutral_text_allowed() {
        let (i, e) = reports("Tell me about rainfall.");
        let d = evaluator().evaluate(&i, &e, &actor_non_us(), &trust_permissive());
        assert_eq!(d.outcome, Outcome::Allow);
        assert!(!d.classification.is_controlled());
    }

    #[test]
    fn test_itar_blocks_non_us_person() {
        let (i, e) = reports("Design notes for the F-35 stealth aircraft.");
        let d = evaluator().evaluate(&i, &e, &actor_non_us(), &trust_permissive());
        assert_eq!(d.outcome, Outcome::DenyExport);
        assert!(d.classification.is_itar());
        // Country gate fires first, before person gate.
        assert!(d.matched_rule.is_some());
    }

    #[test]
    fn test_itar_allows_us_person_but_routes_local() {
        let (i, e) = reports("Design notes for the F-22 air superiority fighter.");
        let d = evaluator().evaluate(&i, &e, &actor_us(), &trust_permissive());
        assert_eq!(d.outcome, Outcome::RouteLocal);
        assert_eq!(d.matched_rule.as_deref(), Some("itar.route_local"));
    }

    #[test]
    fn test_ambiguous_defaults_to_itar_then_blocks_non_us() {
        let (i, e) = reports("This appears to be a defense article reference.");
        assert!(i.defaulted_to_itar_on_ambiguity);
        let d = evaluator().evaluate(&i, &e, &actor_non_us(), &trust_permissive());
        assert_eq!(d.outcome, Outcome::DenyExport);
    }

    #[test]
    fn test_ambiguous_us_person_routes_local_with_ambiguity_reason() {
        let (i, e) = reports("Generic defense article reference here.");
        let d = evaluator().evaluate(&i, &e, &actor_us(), &trust_permissive());
        assert_eq!(d.outcome, Outcome::RouteLocal);
        assert!(d.reason.contains("Ambiguous"));
    }

    #[test]
    fn test_eccn_only_routes_local_for_any_actor() {
        let (i, e) = reports("ECCN 5A002 strong crypto module.");
        let d = evaluator().evaluate(&i, &e, &actor_non_us(), &trust_permissive());
        assert_eq!(d.outcome, Outcome::RouteLocal);
        assert_eq!(d.classification.effective, ExportClassification::Ccl);
    }

    #[test]
    fn test_blocked_country_overrides_uncontrolled() {
        let mut cfg = JurisdictionConfig::default();
        cfg.blocked_countries
            .insert(CountryCode::new("IR").unwrap());
        let eval = JurisdictionEvaluator::new(cfg);
        let actor = ActorContext {
            country: Some(CountryCode::new("IR").unwrap()),
            person_type: PersonType::UsPerson,
            deployment_profile: None,
        };
        let (i, e) = reports("Tell me about rainfall.");
        let d = eval.evaluate(&i, &e, &actor, &trust_permissive());
        assert_eq!(d.outcome, Outcome::DenyExport);
        assert_eq!(
            d.matched_rule.as_deref(),
            Some("jurisdiction.blocked_country")
        );
    }

    #[test]
    fn test_unknown_actor_fails_closed_for_itar() {
        let actor = ActorContext {
            country: Some(CountryCode::us()),
            person_type: PersonType::Unknown,
            deployment_profile: None,
        };
        let (i, e) = reports("Implosion lens geometry for a nuclear warhead.");
        let d = evaluator().evaluate(&i, &e, &actor, &trust_permissive());
        assert_eq!(d.outcome, Outcome::DenyExport);
    }

    #[test]
    fn test_unknown_actor_can_be_configured_open() {
        let cfg = JurisdictionConfig {
            unknown_actor_fails_closed: false,
            ..JurisdictionConfig::default()
        };
        let eval = JurisdictionEvaluator::new(cfg);
        let actor = ActorContext {
            country: Some(CountryCode::us()),
            person_type: PersonType::Unknown,
            deployment_profile: None,
        };
        let (i, e) = reports("Implosion lens geometry for a nuclear warhead.");
        let d = eval.evaluate(&i, &e, &actor, &trust_permissive());
        assert_eq!(d.outcome, Outcome::RouteLocal);
    }

    #[test]
    fn test_missing_country_fails_closed_for_itar() {
        let actor = ActorContext {
            country: None,
            person_type: PersonType::UsPerson,
            deployment_profile: None,
        };
        let (i, e) = reports("Production pathway for VX nerve agent.");
        let d = evaluator().evaluate(&i, &e, &actor, &trust_permissive());
        assert_eq!(d.outcome, Outcome::DenyExport);
    }

    #[test]
    fn test_country_code_validation() {
        assert!(CountryCode::new("US").is_ok());
        assert!(CountryCode::new("us").is_ok()); // lower-case normalized
        assert!(CountryCode::new("USA").is_err()); // 3 letters
        assert!(CountryCode::new("U1").is_err()); // digit
        assert!(CountryCode::new("").is_err());
    }

    #[test]
    fn test_unified_classification_takes_max() {
        // EAR-only CCL content; classification should be Ccl, not Uncontrolled.
        let (i, e) = reports("Software 5D002 controlled cryptographic toolkit.");
        let u = UnifiedClassification::merge(&i, &e);
        assert_eq!(u.effective, ExportClassification::Ccl);
        assert_eq!(u.from_itar, ExportClassification::Uncontrolled);
    }

    #[test]
    fn test_unified_classification_itar_dominates_ear() {
        let (i, e) = reports("ECCN 5A002 module describing an F-35 stealth aircraft.");
        let u = UnifiedClassification::merge(&i, &e);
        assert_eq!(u.effective, ExportClassification::Itar);
    }

    #[test]
    fn test_person_eligibility_helper() {
        assert!(PersonType::UsPerson.is_itar_eligible());
        assert!(!PersonType::NonUsPerson.is_itar_eligible());
        assert!(!PersonType::Unknown.is_itar_eligible());
    }

    #[test]
    fn test_trust_revoked_short_circuits_to_deny() {
        // Even neutral text is denied when the claim is revoked.
        let (i, e) = reports("Tell me about rainfall.");
        let mut trust = trust_permissive();
        trust.revocation_status = RevocationStatus::Revoked;
        let d = evaluator().evaluate(&i, &e, &actor_us(), &trust);
        assert_eq!(d.outcome, Outcome::DenyExport);
        assert_eq!(d.matched_rule.as_deref(), Some("trust.revoked"));
    }

    #[test]
    fn test_trust_scope_missing_denies_itar_query() {
        // ITAR content but the tenant did not license itar_ear scope.
        let (i, e) = reports("Design notes for the F-22 air superiority fighter.");
        let mut trust = trust_permissive();
        trust.compliance_scopes.remove(&ComplianceScope::ItarEar);
        let d = evaluator().evaluate(&i, &e, &actor_us(), &trust);
        assert_eq!(d.outcome, Outcome::DenyExport);
        assert_eq!(d.matched_rule.as_deref(), Some("trust.scope_missing"));
    }

    #[test]
    fn test_trust_scope_missing_does_not_block_uncontrolled() {
        // Uncontrolled content is fine even with no compliance scopes.
        let (i, e) = reports("Tell me about rainfall.");
        let mut trust = trust_permissive();
        trust.compliance_scopes.clear();
        let d = evaluator().evaluate(&i, &e, &actor_us(), &trust);
        assert_eq!(d.outcome, Outcome::Allow);
    }

    #[test]
    fn test_unknown_revocation_denies_itar_query() {
        let (i, e) = reports("Design notes for the F-22 air superiority fighter.");
        let mut trust = trust_permissive();
        trust.revocation_status = RevocationStatus::Unknown;
        let d = evaluator().evaluate(&i, &e, &actor_us(), &trust);
        assert_eq!(d.outcome, Outcome::DenyExport);
        assert_eq!(
            d.matched_rule.as_deref(),
            Some("trust.revocation_unknown_for_itar")
        );
    }

    #[test]
    fn test_unknown_revocation_allows_uncontrolled_query() {
        // For uncontrolled content, unknown revocation is treated as
        // "stale = continue but warn" (per SERVICE-IDENTITY.md §4.5),
        // NOT as denied. Outcome is Allow; the audit layer is
        // responsible for recording the unknown status (visible in
        // d.trust.revocation_status).
        let (i, e) = reports("Tell me about rainfall.");
        let mut trust = trust_permissive();
        trust.revocation_status = RevocationStatus::Unknown;
        let d = evaluator().evaluate(&i, &e, &actor_us(), &trust);
        assert_eq!(d.outcome, Outcome::Allow);
        assert_eq!(d.trust.revocation_status, RevocationStatus::Unknown);
    }

    #[test]
    fn test_allowed_routes_local_only_collapses_uncontrolled_to_route_local() {
        let (i, e) = reports("Tell me about rainfall.");
        let trust = TrustContext::strict_local_only();
        let d = evaluator().evaluate(&i, &e, &actor_us(), &trust);
        assert_eq!(d.outcome, Outcome::RouteLocal);
        assert_eq!(d.matched_rule.as_deref(), Some("trust.allowed_routes"));
    }

    #[test]
    fn test_offline_mode_collapses_uncontrolled_to_route_local() {
        use mai_core::airgap::ConnectivityState;
        let (i, e) = reports("Tell me about rainfall.");
        let mut trust = trust_permissive();
        trust.connectivity = ConnectivityState::StaleNotExpired;
        let d = evaluator().evaluate(&i, &e, &actor_us(), &trust);
        assert_eq!(d.outcome, Outcome::RouteLocal);
        assert_eq!(d.matched_rule.as_deref(), Some("trust.offline_mode"));
        assert!(d.trust.offline_mode);
    }

    #[test]
    fn test_trust_snapshot_recorded_on_every_decision() {
        let (i, e) = reports("Design notes for the F-22.");
        let d = evaluator().evaluate(&i, &e, &actor_us(), &trust_permissive());
        assert_eq!(d.trust.claim_id, "local-dev-claim");
        assert_eq!(d.trust.trust_bundle_version, "local-dev");
        assert_eq!(d.trust.tenant_id.as_str(), "local-dev");
        assert!(d.trust.subject_hash.as_str().starts_with("hmac:"));
    }

    #[test]
    fn test_service_identity_propagated_in_snapshot() {
        let (i, e) = reports("Tell me about rainfall.");
        let mut trust = trust_permissive();
        trust.service_identity = Some(ServiceIdentity::LampreyRouter);
        let d = evaluator().evaluate(&i, &e, &actor_us(), &trust);
        assert_eq!(
            d.trust.service_identity,
            Some(ServiceIdentity::LampreyRouter)
        );
    }

    #[test]
    fn test_revoked_check_runs_before_country_block() {
        // Both revoked AND blocked country present — revoked wins.
        let mut cfg = JurisdictionConfig::default();
        cfg.blocked_countries
            .insert(CountryCode::new("IR").unwrap());
        let eval = JurisdictionEvaluator::new(cfg);
        let actor = ActorContext {
            country: Some(CountryCode::new("IR").unwrap()),
            person_type: PersonType::UsPerson,
            deployment_profile: None,
        };
        let mut trust = trust_permissive();
        trust.revocation_status = RevocationStatus::Revoked;
        let (i, e) = reports("Tell me about rainfall.");
        let d = eval.evaluate(&i, &e, &actor, &trust);
        assert_eq!(d.matched_rule.as_deref(), Some("trust.revoked"));
    }
}
