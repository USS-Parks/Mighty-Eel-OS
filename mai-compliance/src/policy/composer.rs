//! Policy composition engine.
//!
//! The composer coordinates the per-domain compliance modules (HIPAA
//! via [`crate::baa`], ITAR/EAR via [`crate::jurisdiction`], OCAP via
//! [`crate::ocap`]) and folds their independent decisions into a single
//! [`AggregateDecision`] using three conflict-resolution rules:
//!
//! 1. **Any Deny wins.** If any enabled module disallows the request,
//!    the aggregate `allowed` is false.
//! 2. **Most restrictive route wins.** Ordering is
//!    `Cloud < Local < Quarantine`; the aggregate route is the highest
//!    of all module routes.
//! 3. **Flag accumulation.** Flags from every enabled module are
//!    concatenated in priority order, never deduplicated — the audit
//!    layer needs to see every signal that fired.
//!
//! The default priority chain is `OCAP > ITAR > HIPAA`, configurable
//! from `config/compliance/policy.toml`. Priority only affects the
//! ordering of `reasons` / `flags` and the `modules_applied` list; the
//! allow/deny/route axis is order-independent by construction.
//!
//! The composer does *not* run the underlying modules. Call sites build
//! a [`ModuleDecision`] from each module's native output via
//! [`ModuleDecision::from_hipaa`], [`ModuleDecision::from_itar`], and
//! [`ModuleDecision::from_ocap`], then hand the list to
//! [`PolicyComposer::compose`]. This split keeps the composer pure and
//! makes the module results easy to mock in tests.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::baa::BaaDecision;
use crate::jurisdiction::{JurisdictionDecision, Outcome as JurisdictionOutcome};
use crate::ocap::{OcapDecision, OcapOutcome};

/// Compliance modules recognised by the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleId {
    /// HIPAA Business Associate Agreement enforcement.
    Hipaa,
    /// ITAR + EAR jurisdiction evaluation (export control).
    Itar,
    /// OCAP — tribal data sovereignty.
    Ocap,
}

impl ModuleId {
    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hipaa => "hipaa",
            Self::Itar => "itar",
            Self::Ocap => "ocap",
        }
    }

    /// All known modules, in the default priority order
    /// (most-restrictive domain first).
    pub fn all() -> [Self; 3] {
        [Self::Ocap, Self::Itar, Self::Hipaa]
    }
}

/// Routing destination expressed in the composer's wire language.
///
/// Ordered most-permissive → most-restrictive so the
/// `most_restrictive_of` fold is just a `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Destination {
    /// Request may proceed to the cloud route.
    Cloud,
    /// Request must stay on the local appliance.
    Local,
    /// Request is held pending human review.
    Quarantine,
}

impl Destination {
    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cloud => "cloud",
            Self::Local => "local",
            Self::Quarantine => "quarantine",
        }
    }

    /// Return the more restrictive of `self` and `other`.
    pub fn most_restrictive(self, other: Self) -> Self {
        self.max(other)
    }
}

/// One compliance flag emitted by a module.
///
/// Flags are *informational* — they don't change the outcome but are
/// recorded for the audit feed and downstream dashboards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComplianceFlag {
    /// Module that emitted the flag.
    pub module: ModuleId,
    /// Stable flag code (`"phi.detected"`, `"itar.usml_category_iv"`, …).
    pub code: String,
    /// Human-readable detail.
    pub detail: String,
}

impl ComplianceFlag {
    /// Construct a flag with the given fields.
    pub fn new(module: ModuleId, code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            module,
            code: code.into(),
            detail: detail.into(),
        }
    }
}

/// One reason the composer attached to the aggregate decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComplianceReason {
    /// Module that emitted the reason.
    pub module: ModuleId,
    /// Stable rule identifier (e.g. `"ocap.cultural.elder"`). `None`
    /// means the underlying module did not surface one.
    pub rule: Option<String>,
    /// Human-readable summary for the audit log.
    pub summary: String,
}

impl ComplianceReason {
    /// Construct a reason with the given fields.
    pub fn new(module: ModuleId, rule: Option<String>, summary: impl Into<String>) -> Self {
        Self {
            module,
            rule,
            summary: summary.into(),
        }
    }
}

/// Per-module decision in the composer's normalised shape.
///
/// Call sites build one of these from each module's native decision
/// type via the `from_*` constructors below, then hand the list to
/// [`PolicyComposer::compose`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleDecision {
    /// Which module produced this decision.
    pub module: ModuleId,
    /// Per-module allow / deny verdict.
    pub allowed: bool,
    /// Per-module route preference.
    pub route: Destination,
    /// Informational flags emitted by the module.
    #[serde(default)]
    pub flags: Vec<ComplianceFlag>,
    /// One or more reasons. Conventionally non-empty.
    #[serde(default)]
    pub reasons: Vec<ComplianceReason>,
}

impl ModuleDecision {
    /// Build the normalised view from a [`BaaDecision`].
    ///
    /// HIPAA semantics: when the BAA denies the request, the request
    /// must stay local (BAA Standard / Strict modes). When the BAA
    /// allows it, the request may proceed to the cloud route.
    pub fn from_hipaa(decision: &BaaDecision) -> Self {
        let (allowed, route) = if decision.allowed {
            (true, Destination::Cloud)
        } else {
            (false, Destination::Local)
        };
        let flags = decision
            .violations
            .iter()
            .map(|v| {
                ComplianceFlag::new(
                    ModuleId::Hipaa,
                    format!("hipaa.phi.{}", v.identifier.as_str()),
                    v.reason.clone(),
                )
            })
            .collect();
        let reasons = vec![ComplianceReason::new(
            ModuleId::Hipaa,
            None,
            decision.reason.clone(),
        )];
        Self {
            module: ModuleId::Hipaa,
            allowed,
            route,
            flags,
            reasons,
        }
    }

    /// Build the normalised view from a [`JurisdictionDecision`].
    ///
    /// Mapping:
    /// - [`JurisdictionOutcome::Allow`] → `Cloud`, allowed
    /// - [`JurisdictionOutcome::RouteLocal`] → `Local`, allowed
    /// - [`JurisdictionOutcome::DenyExport`] → `Local`, not allowed
    pub fn from_itar(decision: &JurisdictionDecision) -> Self {
        let (allowed, route) = match decision.outcome {
            JurisdictionOutcome::Allow => (true, Destination::Cloud),
            JurisdictionOutcome::RouteLocal => (true, Destination::Local),
            JurisdictionOutcome::DenyExport => (false, Destination::Local),
        };
        let flags = if decision.classification.is_controlled() {
            vec![ComplianceFlag::new(
                ModuleId::Itar,
                "itar.controlled_content",
                format!(
                    "Effective export classification: {:?}",
                    decision.classification.effective
                ),
            )]
        } else {
            Vec::new()
        };
        let reasons = vec![ComplianceReason::new(
            ModuleId::Itar,
            decision.matched_rule.clone(),
            decision.reason.clone(),
        )];
        Self {
            module: ModuleId::Itar,
            allowed,
            route,
            flags,
            reasons,
        }
    }

    /// Build the normalised view from an [`OcapDecision`].
    ///
    /// Mapping:
    /// - [`OcapOutcome::Allow`] → `Cloud`, allowed
    /// - [`OcapOutcome::RouteLocal`] → `Local`, allowed
    /// - [`OcapOutcome::Quarantine`] → `Quarantine`, not allowed
    /// - [`OcapOutcome::DenyAccess`] → `Local`, not allowed
    pub fn from_ocap(decision: &OcapDecision) -> Self {
        let (allowed, route) = match decision.outcome {
            OcapOutcome::Allow => (true, Destination::Cloud),
            OcapOutcome::RouteLocal => (true, Destination::Local),
            OcapOutcome::Quarantine => (false, Destination::Quarantine),
            OcapOutcome::DenyAccess => (false, Destination::Local),
        };
        let mut flags = Vec::new();
        if decision.tribal_data_detected {
            flags.push(ComplianceFlag::new(
                ModuleId::Ocap,
                "ocap.tribal_data_detected",
                "Tribal data identifiers present in payload",
            ));
        }
        if decision.cultural_review_required {
            flags.push(ComplianceFlag::new(
                ModuleId::Ocap,
                "ocap.cultural_review_required",
                "Cultural-sensitivity signal fired; human review required",
            ));
        }
        if decision.treaty_local_only {
            flags.push(ComplianceFlag::new(
                ModuleId::Ocap,
                "ocap.treaty_local_only",
                "Treaty reference forces local-only processing",
            ));
        }
        let reasons: Vec<ComplianceReason> = decision
            .reasons
            .iter()
            .map(|r| ComplianceReason::new(ModuleId::Ocap, Some(r.rule.clone()), r.summary.clone()))
            .collect();
        Self {
            module: ModuleId::Ocap,
            allowed,
            route,
            flags,
            reasons,
        }
    }
}

/// Aggregate decision returned by the composer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregateDecision {
    /// True only if every enabled module allowed the request.
    pub allowed: bool,
    /// Most-restrictive route across all enabled modules. `None` only
    /// when there are no enabled modules with input.
    pub route: Option<Destination>,
    /// Accumulated flags, in priority order then input order.
    pub flags: Vec<ComplianceFlag>,
    /// Accumulated reasons, in priority order then input order.
    pub reasons: Vec<ComplianceReason>,
    /// Modules whose decisions were folded into this aggregate, in
    /// priority order. Disabled modules and modules with no input are
    /// omitted.
    pub modules_applied: Vec<ModuleId>,
}

/// Configuration for the policy composer.
///
/// Loaded from `config/compliance/policy.toml`. Both fields default to
/// the OCAP > ITAR > HIPAA chain with all modules enabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerConfig {
    /// Priority chain. Determines the order in which module reasons
    /// and flags appear in [`AggregateDecision`]. Modules not listed
    /// here sort after the listed ones, preserving input order.
    #[serde(default = "ComposerConfig::default_priority")]
    pub priority: Vec<ModuleId>,
    /// Modules currently enabled. Disabled modules are dropped from
    /// the input before composition; their decisions never influence
    /// the aggregate.
    #[serde(default = "ComposerConfig::default_enabled")]
    pub enabled: BTreeSet<ModuleId>,
}

impl Default for ComposerConfig {
    fn default() -> Self {
        Self {
            priority: Self::default_priority(),
            enabled: Self::default_enabled(),
        }
    }
}

impl ComposerConfig {
    /// OCAP > ITAR > HIPAA, per the policy acceptance criteria.
    pub fn default_priority() -> Vec<ModuleId> {
        vec![ModuleId::Ocap, ModuleId::Itar, ModuleId::Hipaa]
    }

    /// All modules enabled by default.
    pub fn default_enabled() -> BTreeSet<ModuleId> {
        ModuleId::all().into_iter().collect()
    }
}

/// Pure composition engine. Build once, reuse across requests.
#[derive(Debug, Clone, Default)]
pub struct PolicyComposer {
    config: ComposerConfig,
}

impl PolicyComposer {
    /// Build a composer with the given configuration.
    pub fn new(config: ComposerConfig) -> Self {
        Self { config }
    }

    /// Read-only view of the active configuration.
    pub fn config(&self) -> &ComposerConfig {
        &self.config
    }

    /// Replace the active configuration. Returns the previous one so
    /// callers (e.g. the policy API) can stash it for rollback.
    pub fn set_config(&mut self, config: ComposerConfig) -> ComposerConfig {
        std::mem::replace(&mut self.config, config)
    }

    /// Compose a set of per-module decisions into one aggregate.
    pub fn compose<I>(&self, inputs: I) -> AggregateDecision
    where
        I: IntoIterator<Item = ModuleDecision>,
    {
        // 1. Drop disabled modules.
        let mut decisions: Vec<ModuleDecision> = inputs
            .into_iter()
            .filter(|d| self.config.enabled.contains(&d.module))
            .collect();

        // 2. Sort by configured priority. `usize::MAX` parks modules
        //    that are not in the priority list after the listed ones,
        //    preserving their relative input order via `sort_by_key`'s
        //    stable sort.
        decisions.sort_by_key(|d| {
            self.config
                .priority
                .iter()
                .position(|m| *m == d.module)
                .unwrap_or(usize::MAX)
        });

        // 3. Fold.
        let mut allowed = true;
        let mut route: Option<Destination> = None;
        let mut flags = Vec::new();
        let mut reasons = Vec::new();
        let mut modules_applied = Vec::new();
        for d in decisions {
            modules_applied.push(d.module);
            if !d.allowed {
                allowed = false;
            }
            route = Some(match route {
                None => d.route,
                Some(existing) => existing.most_restrictive(d.route),
            });
            flags.extend(d.flags);
            reasons.extend(d.reasons);
        }

        AggregateDecision {
            allowed,
            route,
            flags,
            reasons,
            modules_applied,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(module: ModuleId) -> ModuleDecision {
        ModuleDecision {
            module,
            allowed: true,
            route: Destination::Cloud,
            flags: Vec::new(),
            reasons: vec![ComplianceReason::new(module, None, "allow")],
        }
    }

    fn route_local(module: ModuleId) -> ModuleDecision {
        ModuleDecision {
            module,
            allowed: true,
            route: Destination::Local,
            flags: vec![ComplianceFlag::new(module, "test.local", "local")],
            reasons: vec![ComplianceReason::new(module, None, "route local")],
        }
    }

    fn deny(module: ModuleId) -> ModuleDecision {
        ModuleDecision {
            module,
            allowed: false,
            route: Destination::Local,
            flags: vec![ComplianceFlag::new(module, "test.deny", "deny")],
            reasons: vec![ComplianceReason::new(module, Some("rule.x".into()), "deny")],
        }
    }

    #[test]
    fn destination_ordering_matches_restrictiveness() {
        assert!(Destination::Cloud < Destination::Local);
        assert!(Destination::Local < Destination::Quarantine);
        assert_eq!(
            Destination::Cloud.most_restrictive(Destination::Local),
            Destination::Local
        );
        assert_eq!(
            Destination::Local.most_restrictive(Destination::Quarantine),
            Destination::Quarantine
        );
        assert_eq!(
            Destination::Quarantine.most_restrictive(Destination::Cloud),
            Destination::Quarantine
        );
    }

    #[test]
    fn default_priority_is_ocap_itar_hipaa() {
        let cfg = ComposerConfig::default();
        assert_eq!(
            cfg.priority,
            vec![ModuleId::Ocap, ModuleId::Itar, ModuleId::Hipaa]
        );
    }

    #[test]
    fn compose_all_allow_keeps_cloud_route() {
        let composer = PolicyComposer::default();
        let out = composer.compose([
            allow(ModuleId::Hipaa),
            allow(ModuleId::Itar),
            allow(ModuleId::Ocap),
        ]);
        assert!(out.allowed);
        assert_eq!(out.route, Some(Destination::Cloud));
        assert_eq!(out.modules_applied.len(), 3);
    }

    #[test]
    fn any_deny_wins() {
        let composer = PolicyComposer::default();
        let out = composer.compose([
            allow(ModuleId::Hipaa),
            deny(ModuleId::Itar),
            allow(ModuleId::Ocap),
        ]);
        assert!(!out.allowed);
    }

    #[test]
    fn most_restrictive_route_wins() {
        let composer = PolicyComposer::default();
        let out = composer.compose([
            allow(ModuleId::Hipaa),
            route_local(ModuleId::Itar),
            allow(ModuleId::Ocap),
        ]);
        assert!(out.allowed);
        assert_eq!(out.route, Some(Destination::Local));
    }

    #[test]
    fn quarantine_beats_local_in_route_fold() {
        let composer = PolicyComposer::default();
        let mut quarantine = deny(ModuleId::Ocap);
        quarantine.route = Destination::Quarantine;
        let out = composer.compose([route_local(ModuleId::Hipaa), quarantine]);
        assert_eq!(out.route, Some(Destination::Quarantine));
        assert!(!out.allowed);
    }

    #[test]
    fn flags_accumulate_in_priority_order() {
        let composer = PolicyComposer::default();
        let out = composer.compose([
            route_local(ModuleId::Hipaa),
            route_local(ModuleId::Itar),
            route_local(ModuleId::Ocap),
        ]);
        // OCAP first, then ITAR, then HIPAA.
        assert_eq!(out.flags.len(), 3);
        assert_eq!(out.flags[0].module, ModuleId::Ocap);
        assert_eq!(out.flags[1].module, ModuleId::Itar);
        assert_eq!(out.flags[2].module, ModuleId::Hipaa);
    }

    #[test]
    fn disabled_module_is_dropped() {
        let mut cfg = ComposerConfig::default();
        cfg.enabled.remove(&ModuleId::Hipaa);
        let composer = PolicyComposer::new(cfg);
        let out = composer.compose([deny(ModuleId::Hipaa), allow(ModuleId::Itar)]);
        assert!(out.allowed, "HIPAA deny should be dropped");
        assert_eq!(out.modules_applied, vec![ModuleId::Itar]);
    }

    #[test]
    fn no_inputs_yields_allow_with_no_route() {
        let composer = PolicyComposer::default();
        let out = composer.compose([]);
        assert!(out.allowed);
        assert_eq!(out.route, None);
        assert!(out.flags.is_empty());
        assert!(out.reasons.is_empty());
        assert!(out.modules_applied.is_empty());
    }

    #[test]
    fn set_config_returns_previous() {
        let mut composer = PolicyComposer::default();
        let new_cfg = ComposerConfig {
            priority: vec![ModuleId::Hipaa],
            ..ComposerConfig::default()
        };
        let old = composer.set_config(new_cfg.clone());
        assert_eq!(old.priority, ComposerConfig::default().priority);
        assert_eq!(composer.config().priority, new_cfg.priority);
    }
}
