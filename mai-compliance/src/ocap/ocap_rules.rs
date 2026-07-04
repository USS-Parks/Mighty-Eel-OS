//! OCAP policy engine — Ownership, Control, Access, Possession.
//!
//! Combines the outputs of [`super::tribal_data`],
//! [`super::treaty`], and [`super::cultural`] with the tenant's
//! governance metadata and [`crate::trust::TrustContext`] to produce
//! an [`OcapDecision`].
//!
//! The four OCAP principles map onto rules as follows:
//!
//! - **Ownership**: data tagged or detected as tribal stays under the
//!   originating community's authority. The decision records the
//!   tenant id from the trust context so audit can correlate
//!   ownership back to a tribal nation.
//! - **Control**: processing tribal data requires the caller's
//!   profile to be on the authorised list. The list is part of the
//!   tenant's governance metadata. Unauthorised callers are denied
//!   even when the data classification would otherwise allow them.
//! - **Access**: per-role access (elder / council / member / public).
//!   Sacred or elder-attributed material requires a sufficient role.
//! - **Possession**: tribal data must be processed on-premises. Cloud
//!   routes are refused regardless of other factors. This is the
//!   air-gap-equivalent of OCAP — and the most differentiating part
//!   of the engine.
//!
//! ### TrustContext contract
//!
//! The evaluator refuses to evaluate when:
//!
//! - `compliance_scopes` does not contain `ComplianceScope::Ocap`
//!   (the tenant did not authorise OCAP evaluation).
//! - `revocation_status` is `Revoked` (the bridge revoked this claim).
//!
//! When the trust layer asserts `allowed_routes = { LocalOnly }`, the
//! decision is `RouteLocal` even for content with no tribal-data
//! signal — the trust ceiling beats the classification floor.
//!
//! Every [`OcapDecision`] carries the audit-grade trust fields
//! (`tenant_id`, `subject_hash`, `claim_id`, `trust_bundle_version`,
//! `service_identity`, `offline_mode`, `revocation_status`) so the
//! audit log can record the decision without re-deriving
//! anything from the TrustContext.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::trust::{
    AllowedRoute, ComplianceScope, RevocationStatus, ServiceIdentity, TrustContext,
};

use super::cultural::{CulturalReport, CulturalSignal};
use super::treaty::TreatyReport;
use super::tribal_data::{TribalDataReport, TribalIdentifierKind};

/// Per-role access tier. Mapping a caller to a role is the operator's
/// responsibility; OCAP only enforces what each role may see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessRole {
    /// Public — no privileged access.
    Public,
    /// Tribal member with verified lineage.
    Member,
    /// Tribal council member; broader administrative access.
    Council,
    /// Elder; access to elder-attributed material and ceremonial
    /// content under cultural authority.
    Elder,
    /// Cultural authority that may approve or refuse cross-community
    /// release of restricted material. Operates above Elder for
    /// release decisions; not above Elder for personal knowledge.
    CulturalAuthority,
}

impl AccessRole {
    /// Numeric tier for `>=` gating. Higher = more access.
    pub fn tier(self) -> u8 {
        match self {
            Self::Public => 0,
            Self::Member => 1,
            Self::Council => 2,
            Self::Elder => 3,
            Self::CulturalAuthority => 4,
        }
    }

    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Member => "member",
            Self::Council => "council",
            Self::Elder => "elder",
            Self::CulturalAuthority => "cultural_authority",
        }
    }
}

/// Possession status for the data being processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PossessionStatus {
    /// Stored on-premises under tribal control.
    OnPremises,
    /// Stored at an approved sovereign-cloud location (rare — must be
    /// individually approved by the tribal authority).
    SovereignCloud,
    /// Stored in a third-party cloud. Tribal data here is an OCAP
    /// violation by default.
    ThirdPartyCloud,
    /// Possession is unknown / unverified — fail-closed.
    Unknown,
}

impl PossessionStatus {
    /// True iff possession is acceptable for tribal data without
    /// further consent review.
    pub fn is_on_premises(self) -> bool {
        matches!(self, Self::OnPremises)
    }

    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OnPremises => "on_premises",
            Self::SovereignCloud => "sovereign_cloud",
            Self::ThirdPartyCloud => "third_party_cloud",
            Self::Unknown => "unknown",
        }
    }
}

/// Consent status for the requested processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentStatus {
    /// Explicit consent on file (signed protocol, tribal-authority
    /// approval, or live consent capture).
    Granted,
    /// Consent has been requested but not yet recorded.
    Pending,
    /// Consent was explicitly refused.
    Refused,
    /// No consent record exists. Fail-closed.
    NotProvided,
}

impl ConsentStatus {
    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Pending => "pending",
            Self::Refused => "refused",
            Self::NotProvided => "not_provided",
        }
    }

    /// True iff processing may proceed without quarantining.
    pub fn permits_processing(self) -> bool {
        matches!(self, Self::Granted)
    }
}

/// Tenant governance metadata supplied by the operator on every
/// request. This is the *non-trust* policy input that complements
/// [`TrustContext`] — `TrustContext` says who the caller is and what
/// the trust layer allows, while `GovernanceMetadata` describes the
/// **data**'s status under the tenant's tribal governance rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceMetadata {
    /// Caller's access role under tribal governance.
    pub access_role: AccessRole,
    /// Where the underlying data is physically stored / processed.
    pub possession: PossessionStatus,
    /// State of the consent record for the requested processing.
    pub consent: ConsentStatus,
    /// True when the tenant has explicitly tagged this data as
    /// tribally owned (overrides detection; the engine treats this
    /// as authoritative).
    #[serde(default)]
    pub tribal_owned: bool,
    /// True when this request has been tagged as needing
    /// human-in-the-loop review regardless of automated decision.
    #[serde(default)]
    pub force_review: bool,
}

impl Default for GovernanceMetadata {
    fn default() -> Self {
        Self {
            access_role: AccessRole::Public,
            possession: PossessionStatus::Unknown,
            consent: ConsentStatus::NotProvided,
            tribal_owned: false,
            force_review: false,
        }
    }
}

/// Evaluator configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcapConfig {
    /// When true, the evaluator promotes a tribal-data report with
    /// only Possible-confidence hits to actionable. Defaults to
    /// `false` — false positives are a respect violation.
    #[serde(default)]
    pub possible_implies_tribal: bool,
    /// When true (default), the evaluator refuses to evaluate when
    /// `TrustContext::compliance_scopes` does not contain `Ocap`.
    /// Set to false only for bring-up; production deployments must
    /// require scope alignment.
    #[serde(default = "default_require_scope")]
    pub require_ocap_scope: bool,
    /// Minimum access role required for elder-attributed material.
    /// Defaults to [`AccessRole::Elder`].
    #[serde(default = "default_elder_role")]
    pub min_role_for_elder_material: AccessRole,
    /// Minimum access role required for sacred / ceremonial material.
    /// Defaults to [`AccessRole::Council`].
    #[serde(default = "default_sacred_role")]
    pub min_role_for_sacred_material: AccessRole,
    /// Set of [`AccessRole`]s authorised to process tribal data at
    /// all (the "control" principle). Empty set = any role with the
    /// per-signal minimum may proceed (this is the most permissive
    /// default; tenants should lock this down).
    #[serde(default)]
    pub authorised_profiles: BTreeSet<AccessRole>,
}

fn default_require_scope() -> bool {
    true
}

fn default_elder_role() -> AccessRole {
    AccessRole::Elder
}

fn default_sacred_role() -> AccessRole {
    AccessRole::Council
}

impl Default for OcapConfig {
    fn default() -> Self {
        Self {
            possible_implies_tribal: false,
            require_ocap_scope: default_require_scope(),
            min_role_for_elder_material: default_elder_role(),
            min_role_for_sacred_material: default_sacred_role(),
            authorised_profiles: BTreeSet::new(),
        }
    }
}

/// Evaluator errors.
#[derive(Debug, Error)]
pub enum OcapError {
    /// Trust context did not authorise OCAP evaluation.
    #[error("OCAP scope missing from trust context (tenant '{tenant}')")]
    ScopeMissing {
        /// Tenant id from the trust context, for the audit record.
        tenant: String,
    },
}

/// OCAP decision outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcapOutcome {
    /// Request may proceed normally.
    Allow,
    /// Request must stay on the local appliance.
    RouteLocal,
    /// Request held pending human review.
    Quarantine,
    /// Request refused (access control or revocation).
    DenyAccess,
}

impl OcapOutcome {
    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::RouteLocal => "route_local",
            Self::Quarantine => "quarantine",
            Self::DenyAccess => "deny_access",
        }
    }
}

/// Single reason code emitted by the evaluator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OcapReason {
    /// Stable rule identifier (`"ocap.possession.cloud_blocked"`, etc.).
    pub rule: String,
    /// Human-readable summary for the audit record.
    pub summary: String,
}

impl OcapReason {
    fn new(rule: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            rule: rule.into(),
            summary: summary.into(),
        }
    }
}

/// Result of an OCAP evaluation. Includes the trust-correlation
/// fields the audit log needs.
#[derive(Debug, Clone, Serialize)]
pub struct OcapDecision {
    /// Routing outcome.
    pub outcome: OcapOutcome,
    /// True when tribal data was detected or asserted.
    pub tribal_data_detected: bool,
    /// True when at least one cultural-sensitivity signal fired.
    pub cultural_review_required: bool,
    /// True when a treaty reference required local processing.
    pub treaty_local_only: bool,
    /// Reasons that drove the outcome (one or more).
    pub reasons: Vec<OcapReason>,
    /// Primary matched rule (the one that fixed the outcome).
    pub matched_rule: Option<String>,

    // Trust correlation fields. --
    /// Tenant id from the trust context.
    pub tenant_id: String,
    /// Audit-safe subject hash.
    pub subject_hash: String,
    /// Claim id (audit correlation key).
    pub claim_id: String,
    /// Trust bundle version the claim was issued against.
    pub trust_bundle_version: String,
    /// Optional service identity (set when the caller is a service).
    pub service_identity: Option<ServiceIdentity>,
    /// Local appliance state at decision time.
    pub offline_mode: bool,
    /// Revocation status at decision time.
    pub revocation_status: RevocationStatus,
}

/// Internal bundle of the three "did we see X?" booleans that flow
/// through every decision branch. Kept private to the module; existed
/// only to keep [`OcapEvaluator::build_decision`] under the
/// `clippy::too_many_arguments` ceiling and to give the call sites a
/// single point of mutation if a new fact is added later.
#[derive(Debug, Clone, Copy)]
struct DecisionFacts {
    tribal_data_detected: bool,
    cultural_review_required: bool,
    treaty_local_only: bool,
}

/// OCAP policy evaluator.
#[derive(Debug, Clone, Default)]
pub struct OcapEvaluator {
    config: OcapConfig,
}

impl OcapEvaluator {
    /// Build an evaluator with the given config.
    pub fn new(config: OcapConfig) -> Self {
        Self { config }
    }

    /// Evaluate a request. Returns `Err(OcapError::ScopeMissing)` only
    /// when the trust context refuses OCAP evaluation entirely.
    pub fn evaluate(
        &self,
        tribal: &TribalDataReport,
        treaty: &TreatyReport,
        cultural: &CulturalReport,
        gov: &GovernanceMetadata,
        trust: &TrustContext,
    ) -> Result<OcapDecision, OcapError> {
        if self.config.require_ocap_scope && !trust.allows_scope(ComplianceScope::Ocap) {
            return Err(OcapError::ScopeMissing {
                tenant: trust.tenant_id.as_str().to_string(),
            });
        }

        let mut reasons: Vec<OcapReason> = Vec::new();
        let tribal_data_detected = gov.tribal_owned
            || tribal.has_actionable()
            || (self.config.possible_implies_tribal && tribal.has_any());
        let cultural_review_required = cultural.requires_review() || gov.force_review;
        let treaty_local_only = treaty.requires_local_processing;
        let facts = DecisionFacts {
            tribal_data_detected,
            cultural_review_required,
            treaty_local_only,
        };

        // 1. Revocation gate.
        if trust.is_revoked() {
            reasons.push(OcapReason::new(
                "ocap.revoked",
                "Trust claim has been revoked.",
            ));
            return Ok(self.build_decision(
                OcapOutcome::DenyAccess,
                Some("ocap.revoked".to_string()),
                facts,
                reasons,
                trust,
            ));
        }

        // 2. Hard local-only trust ceiling. If the trust layer says
        //    "local only", the outcome is at worst RouteLocal.
        let trust_local_ceiling = trust.is_local_only_ceiling();
        if trust_local_ceiling {
            reasons.push(OcapReason::new(
                "ocap.trust.local_only_ceiling",
                "Trust context requires local-only routing.",
            ));
        }

        // 3. Possession gate. Tribal data anywhere but on-premises
        //    is a possession violation.
        let possession_violation = (tribal_data_detected || treaty.has_any())
            && !matches!(gov.possession, PossessionStatus::OnPremises);
        if possession_violation {
            let summary = format!(
                "Tribal data possession status is '{}'; OCAP requires on-premises.",
                gov.possession.as_str(),
            );
            reasons.push(OcapReason::new("ocap.possession.not_on_premises", summary));
            // Possession violation is recoverable by routing local
            // when the data IS on a sovereign appliance. If the data
            // is in third-party cloud or unknown, the only safe
            // answer is to quarantine for human review.
            let outcome = match gov.possession {
                PossessionStatus::SovereignCloud => OcapOutcome::RouteLocal,
                _ => OcapOutcome::Quarantine,
            };
            return Ok(self.build_decision(
                outcome,
                Some("ocap.possession.not_on_premises".to_string()),
                facts,
                reasons,
                trust,
            ));
        }

        // 4. Control gate. If the tenant has an authorised-profiles
        //    set and the caller's role is not in it, deny.
        if (tribal_data_detected || treaty.has_any())
            && !self.config.authorised_profiles.is_empty()
            && !self.config.authorised_profiles.contains(&gov.access_role)
        {
            let summary = format!(
                "Caller role '{}' is not in the tenant's authorised-profiles set.",
                gov.access_role.as_str(),
            );
            reasons.push(OcapReason::new("ocap.control.unauthorised", summary));
            return Ok(self.build_decision(
                OcapOutcome::DenyAccess,
                Some("ocap.control.unauthorised".to_string()),
                facts,
                reasons,
                trust,
            ));
        }

        // 5. Access gate. Sacred / elder-attributed content requires
        //    a sufficient role.
        let sacred_or_ceremonial = cultural.has_signal(CulturalSignal::SacredKnowledge)
            || cultural.has_signal(CulturalSignal::Ceremonial)
            || cultural.has_signal(CulturalSignal::Funerary);
        let elder_attributed = cultural.has_signal(CulturalSignal::ElderTeaching)
            || tribal.has_kind(TribalIdentifierKind::ElderAttribution);

        if sacred_or_ceremonial
            && gov.access_role.tier() < self.config.min_role_for_sacred_material.tier()
        {
            let summary = format!(
                "Sacred/ceremonial content requires role '{}'; caller is '{}'.",
                self.config.min_role_for_sacred_material.as_str(),
                gov.access_role.as_str(),
            );
            reasons.push(OcapReason::new("ocap.access.sacred_role", summary));
            return Ok(self.build_decision(
                OcapOutcome::DenyAccess,
                Some("ocap.access.sacred_role".to_string()),
                facts,
                reasons,
                trust,
            ));
        }

        if elder_attributed
            && gov.access_role.tier() < self.config.min_role_for_elder_material.tier()
        {
            let summary = format!(
                "Elder-attributed content requires role '{}'; caller is '{}'.",
                self.config.min_role_for_elder_material.as_str(),
                gov.access_role.as_str(),
            );
            reasons.push(OcapReason::new("ocap.access.elder_role", summary));
            return Ok(self.build_decision(
                OcapOutcome::DenyAccess,
                Some("ocap.access.elder_role".to_string()),
                facts,
                reasons,
                trust,
            ));
        }

        // 6. Cultural review gate. Probable+ cultural signals → quarantine
        //    unless consent is on file.
        if cultural_review_required && !gov.consent.permits_processing() {
            let summary = format!(
                "Cultural-sensitivity signal fired and consent status is '{}'; quarantining.",
                gov.consent.as_str(),
            );
            reasons.push(OcapReason::new("ocap.cultural.review_required", summary));
            return Ok(self.build_decision(
                OcapOutcome::Quarantine,
                Some("ocap.cultural.review_required".to_string()),
                facts,
                reasons,
                trust,
            ));
        }

        // 7. Treaty consent gate. Treaties requiring consent review
        //    quarantine until consent is recorded, just like cultural.
        if treaty.requires_consent_review && !gov.consent.permits_processing() {
            let summary = format!(
                "Treaty obligations require consent review; consent status is '{}'.",
                gov.consent.as_str(),
            );
            reasons.push(OcapReason::new("ocap.treaty.consent_required", summary));
            return Ok(self.build_decision(
                OcapOutcome::Quarantine,
                Some("ocap.treaty.consent_required".to_string()),
                facts,
                reasons,
                trust,
            ));
        }

        // 8. Possession (positive case). Tribal data on-premises with
        //    consent on file routes local.
        if tribal_data_detected || treaty_local_only || trust_local_ceiling {
            let rule = if treaty_local_only {
                "ocap.treaty.route_local"
            } else if tribal_data_detected {
                "ocap.ownership.route_local"
            } else {
                "ocap.trust.local_only_ceiling"
            };
            reasons.push(OcapReason::new(
                rule,
                if treaty_local_only {
                    "Treaty obligation requires local processing; routing local."
                } else if tribal_data_detected {
                    "Tribal data detected; ownership rule routes local."
                } else {
                    "Trust ceiling requires local routing."
                },
            ));
            return Ok(self.build_decision(
                OcapOutcome::RouteLocal,
                Some(rule.to_string()),
                facts,
                reasons,
                trust,
            ));
        }

        // 9. No OCAP signal at all → allow. The trust ceiling still
        //    applies above us in the policy runtime.
        reasons.push(OcapReason::new(
            "ocap.no_signal",
            "No tribal-data, treaty, or cultural-sensitivity signal detected.",
        ));
        Ok(self.build_decision(OcapOutcome::Allow, None, facts, reasons, trust))
    }

    fn build_decision(
        &self,
        outcome: OcapOutcome,
        matched_rule: Option<String>,
        facts: DecisionFacts,
        reasons: Vec<OcapReason>,
        trust: &TrustContext,
    ) -> OcapDecision {
        OcapDecision {
            outcome,
            tribal_data_detected: facts.tribal_data_detected,
            cultural_review_required: facts.cultural_review_required,
            treaty_local_only: facts.treaty_local_only,
            reasons,
            matched_rule,
            tenant_id: trust.tenant_id.as_str().to_string(),
            subject_hash: trust.subject_hash.as_str().to_string(),
            claim_id: trust.claim_id.clone(),
            trust_bundle_version: trust.trust_bundle_version.clone(),
            service_identity: trust.service_identity,
            offline_mode: trust.offline_mode(),
            revocation_status: trust.revocation_status,
        }
    }
}

/// Extension trait that exposes the local-only ceiling check on a
/// `TrustContext`. We define it locally rather than adding it to
/// [`crate::trust::TrustContext`] because the semantics here are
/// specific to OCAP: any route set that does NOT contain
/// [`AllowedRoute::CloudAllowed`] is treated as a local ceiling, so
/// `{ LocalOnly }`, `{ LocalPreferred }`, and `{}` all gate as
/// local-only. The HIPAA / ITAR engines may interpret these
/// differently.
trait TrustContextOcapExt {
    fn is_local_only_ceiling(&self) -> bool;
}

impl TrustContextOcapExt for TrustContext {
    fn is_local_only_ceiling(&self) -> bool {
        !self.allowed_routes.contains(&AllowedRoute::CloudAllowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocap::cultural::CulturalFilter;
    use crate::ocap::treaty::{TreatyDetector, TreatyDetectorConfig};
    use crate::ocap::tribal_data::TribalDataDetector;
    use crate::trust::TrustContext;

    fn evaluator() -> OcapEvaluator {
        OcapEvaluator::default()
    }

    fn trust_with_ocap_scope() -> TrustContext {
        TrustContext::for_local_dev()
    }

    fn trust_without_ocap_scope() -> TrustContext {
        let mut ctx = TrustContext::for_local_dev();
        ctx.compliance_scopes.remove(&ComplianceScope::Ocap);
        ctx
    }

    fn neutral_gov() -> GovernanceMetadata {
        GovernanceMetadata {
            access_role: AccessRole::Public,
            possession: PossessionStatus::OnPremises,
            consent: ConsentStatus::NotProvided,
            tribal_owned: false,
            force_review: false,
        }
    }

    fn tribal_council_gov() -> GovernanceMetadata {
        GovernanceMetadata {
            access_role: AccessRole::Council,
            possession: PossessionStatus::OnPremises,
            consent: ConsentStatus::Granted,
            tribal_owned: true,
            force_review: false,
        }
    }

    fn scan_all(text: &str) -> (TribalDataReport, TreatyReport, CulturalReport) {
        (
            TribalDataDetector::baseline().scan(text),
            TreatyDetector::new(TreatyDetectorConfig::default()).scan(text),
            CulturalFilter::baseline().scan(text),
        )
    }

    #[test]
    fn neutral_text_allows() {
        let (t, tr, c) = scan_all("Tell me about the weather.");
        let d = evaluator()
            .evaluate(&t, &tr, &c, &neutral_gov(), &trust_with_ocap_scope())
            .expect("evaluation");
        assert_eq!(d.outcome, OcapOutcome::Allow);
        assert!(!d.tribal_data_detected);
        assert!(!d.cultural_review_required);
    }

    #[test]
    fn tribal_data_routes_local_when_on_premises_and_consented() {
        let (t, tr, c) = scan_all("This data falls under traditional ecological knowledge.");
        let d = evaluator()
            .evaluate(&t, &tr, &c, &tribal_council_gov(), &trust_with_ocap_scope())
            .expect("evaluation");
        assert_eq!(d.outcome, OcapOutcome::RouteLocal);
        assert!(d.tribal_data_detected);
    }

    #[test]
    fn tribal_data_on_third_party_cloud_quarantines() {
        let (t, tr, c) = scan_all("Notes on traditional ecological knowledge.");
        let mut gov = tribal_council_gov();
        gov.possession = PossessionStatus::ThirdPartyCloud;
        let d = evaluator()
            .evaluate(&t, &tr, &c, &gov, &trust_with_ocap_scope())
            .expect("evaluation");
        assert_eq!(d.outcome, OcapOutcome::Quarantine);
        assert_eq!(
            d.matched_rule.as_deref(),
            Some("ocap.possession.not_on_premises")
        );
    }

    #[test]
    fn tribal_data_on_sovereign_cloud_routes_local() {
        let (t, tr, c) = scan_all("Notes on traditional ecological knowledge.");
        let mut gov = tribal_council_gov();
        gov.possession = PossessionStatus::SovereignCloud;
        let d = evaluator()
            .evaluate(&t, &tr, &c, &gov, &trust_with_ocap_scope())
            .expect("evaluation");
        assert_eq!(d.outcome, OcapOutcome::RouteLocal);
    }

    #[test]
    fn unauthorised_profile_denies() {
        let cfg = OcapConfig {
            authorised_profiles: {
                let mut s = BTreeSet::new();
                s.insert(AccessRole::Council);
                s.insert(AccessRole::Elder);
                s
            },
            ..OcapConfig::default()
        };
        let eval = OcapEvaluator::new(cfg);
        let (t, tr, c) = scan_all("Notes on traditional ecological knowledge.");
        let mut gov = tribal_council_gov();
        gov.access_role = AccessRole::Public;
        let d = eval
            .evaluate(&t, &tr, &c, &gov, &trust_with_ocap_scope())
            .expect("evaluation");
        assert_eq!(d.outcome, OcapOutcome::DenyAccess);
        assert_eq!(d.matched_rule.as_deref(), Some("ocap.control.unauthorised"));
    }

    #[test]
    fn sacred_material_requires_council_role() {
        let (t, tr, c) = scan_all("Restricted teaching shared in confidence — closed ceremony.");
        let mut gov = tribal_council_gov();
        gov.access_role = AccessRole::Member;
        let d = evaluator()
            .evaluate(&t, &tr, &c, &gov, &trust_with_ocap_scope())
            .expect("evaluation");
        assert_eq!(d.outcome, OcapOutcome::DenyAccess);
        assert_eq!(d.matched_rule.as_deref(), Some("ocap.access.sacred_role"));
    }

    #[test]
    fn elder_attributed_material_requires_elder_role() {
        let (t, tr, c) =
            scan_all("As shared by our elder, this teaching has guided us for generations.");
        let mut gov = tribal_council_gov();
        gov.access_role = AccessRole::Member;
        let d = evaluator()
            .evaluate(&t, &tr, &c, &gov, &trust_with_ocap_scope())
            .expect("evaluation");
        assert_eq!(d.outcome, OcapOutcome::DenyAccess);
        assert_eq!(d.matched_rule.as_deref(), Some("ocap.access.elder_role"));
    }

    #[test]
    fn cultural_review_quarantines_without_consent() {
        let (t, tr, c) = scan_all("Notes from the Pipe Ceremony last summer.");
        let mut gov = tribal_council_gov();
        // Consent is NOT granted; cultural signal fires.
        gov.consent = ConsentStatus::NotProvided;
        let d = evaluator()
            .evaluate(&t, &tr, &c, &gov, &trust_with_ocap_scope())
            .expect("evaluation");
        assert_eq!(d.outcome, OcapOutcome::Quarantine);
        assert_eq!(
            d.matched_rule.as_deref(),
            Some("ocap.cultural.review_required")
        );
    }

    #[test]
    fn cultural_review_proceeds_with_consent() {
        let (t, tr, c) = scan_all("Notes from the Pipe Ceremony last summer.");
        let gov = tribal_council_gov(); // consent = Granted
        let d = evaluator()
            .evaluate(&t, &tr, &c, &gov, &trust_with_ocap_scope())
            .expect("evaluation");
        assert!(
            matches!(d.outcome, OcapOutcome::RouteLocal | OcapOutcome::Allow),
            "expected proceed; got {:?}",
            d.outcome,
        );
    }

    #[test]
    fn treaty_reference_forces_local() {
        let (t, tr, c) = scan_all("This claim falls under Treaty 7 provisions.");
        let d = evaluator()
            .evaluate(&t, &tr, &c, &tribal_council_gov(), &trust_with_ocap_scope())
            .expect("evaluation");
        assert!(d.treaty_local_only);
        // Unknown treaty → consent required → quarantine (because
        // tribal_council_gov has consent granted, so this proceeds
        // to RouteLocal).
        // Actually: treaty.requires_consent_review = true for unknown
        // treaties, but tribal_council_gov has Granted consent, so
        // the consent gate is satisfied. Outcome should be RouteLocal.
        assert_eq!(d.outcome, OcapOutcome::RouteLocal);
    }

    #[test]
    fn revoked_claim_denies_access() {
        let mut trust = trust_with_ocap_scope();
        trust.revocation_status = RevocationStatus::Revoked;
        let (t, tr, c) = scan_all("Notes on traditional ecological knowledge.");
        let d = evaluator()
            .evaluate(&t, &tr, &c, &tribal_council_gov(), &trust)
            .expect("evaluation");
        assert_eq!(d.outcome, OcapOutcome::DenyAccess);
        assert_eq!(d.matched_rule.as_deref(), Some("ocap.revoked"));
    }

    #[test]
    fn scope_missing_returns_error() {
        let (t, tr, c) = scan_all("Notes on traditional ecological knowledge.");
        let r = evaluator().evaluate(
            &t,
            &tr,
            &c,
            &tribal_council_gov(),
            &trust_without_ocap_scope(),
        );
        assert!(matches!(r, Err(OcapError::ScopeMissing { .. })));
    }

    #[test]
    fn decision_records_trust_correlation_fields() {
        let (t, tr, c) = scan_all("Tell me about the weather.");
        let trust = trust_with_ocap_scope();
        let d = evaluator()
            .evaluate(&t, &tr, &c, &neutral_gov(), &trust)
            .expect("evaluation");
        assert_eq!(d.tenant_id, trust.tenant_id.as_str());
        assert_eq!(d.subject_hash, trust.subject_hash.as_str());
        assert_eq!(d.claim_id, trust.claim_id);
        assert_eq!(d.trust_bundle_version, trust.trust_bundle_version);
        assert_eq!(d.revocation_status, trust.revocation_status);
    }

    #[test]
    fn tribal_owned_flag_overrides_detection() {
        // No tribal content in text; metadata asserts tribal ownership.
        let (t, tr, c) = scan_all("This is opaque internal data.");
        let mut gov = tribal_council_gov();
        gov.tribal_owned = true;
        gov.possession = PossessionStatus::ThirdPartyCloud;
        let d = evaluator()
            .evaluate(&t, &tr, &c, &gov, &trust_with_ocap_scope())
            .expect("evaluation");
        assert!(d.tribal_data_detected);
        assert_eq!(d.outcome, OcapOutcome::Quarantine);
    }

    #[test]
    fn force_review_quarantines_even_without_cultural_signal() {
        let (t, tr, c) = scan_all("Standard request body, nothing tribal in content.");
        let mut gov = tribal_council_gov();
        gov.tribal_owned = true;
        gov.consent = ConsentStatus::NotProvided;
        gov.force_review = true;
        let d = evaluator()
            .evaluate(&t, &tr, &c, &gov, &trust_with_ocap_scope())
            .expect("evaluation");
        assert!(d.cultural_review_required);
        assert_eq!(d.outcome, OcapOutcome::Quarantine);
    }

    #[test]
    fn local_only_trust_ceiling_routes_local_even_without_tribal_signal() {
        let mut trust = trust_with_ocap_scope();
        trust.allowed_routes.clear();
        trust.allowed_routes.insert(AllowedRoute::LocalOnly);
        let (t, tr, c) = scan_all("Tell me about the weather.");
        let d = evaluator()
            .evaluate(&t, &tr, &c, &neutral_gov(), &trust)
            .expect("evaluation");
        assert_eq!(d.outcome, OcapOutcome::RouteLocal);
        assert_eq!(
            d.matched_rule.as_deref(),
            Some("ocap.trust.local_only_ceiling"),
        );
    }

    #[test]
    fn access_role_tier_ordering() {
        assert!(AccessRole::Public.tier() < AccessRole::Member.tier());
        assert!(AccessRole::Member.tier() < AccessRole::Council.tier());
        assert!(AccessRole::Council.tier() < AccessRole::Elder.tier());
        assert!(AccessRole::Elder.tier() < AccessRole::CulturalAuthority.tier());
    }

    #[test]
    fn possession_helpers_consistent() {
        assert!(PossessionStatus::OnPremises.is_on_premises());
        assert!(!PossessionStatus::SovereignCloud.is_on_premises());
        assert!(!PossessionStatus::ThirdPartyCloud.is_on_premises());
        assert!(!PossessionStatus::Unknown.is_on_premises());
    }

    #[test]
    fn consent_permits_processing_only_when_granted() {
        assert!(ConsentStatus::Granted.permits_processing());
        assert!(!ConsentStatus::Pending.permits_processing());
        assert!(!ConsentStatus::Refused.permits_processing());
        assert!(!ConsentStatus::NotProvided.permits_processing());
    }

    #[test]
    fn outcome_wire_format_round_trips() {
        // Sanity-check serde tags.
        let outcomes = [
            OcapOutcome::Allow,
            OcapOutcome::RouteLocal,
            OcapOutcome::Quarantine,
            OcapOutcome::DenyAccess,
        ];
        for o in outcomes {
            let s = serde_json::to_string(&o).unwrap();
            let back: OcapOutcome = serde_json::from_str(&s).unwrap();
            assert_eq!(o, back);
        }
    }
}
