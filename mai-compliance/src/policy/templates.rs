//! Pre-built compliance profiles.
//!
//! Templates are *starting points*: each one selects the modules that
//! make sense for a deployment vertical and gives the composer a
//! priority chain that reflects that vertical's restrictiveness order.
//! Operators are expected to customise the resulting [`ComposerConfig`]
//! before promoting it to production — the templates are not a
//! substitute for governance review.
//!
//! Versioning: every template carries a [`TemplateVersion`]. When the
//! shipped defaults change, the major version bumps; operators can
//! pin to a specific version to keep behaviour stable across upgrades.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::composer::{ComposerConfig, ModuleId};

/// Semver-ish version stamped onto every shipped template. The
/// `major` digit bumps when the *set* of enabled modules changes or
/// the priority chain reorders. Within a major, `minor` bumps are
/// safe (additive metadata, new optional fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TemplateVersion {
    /// Major version. Bumps on semantically observable changes.
    pub major: u16,
    /// Minor version. Safe additive changes.
    pub minor: u16,
}

impl TemplateVersion {
    /// Construct a version from its components.
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }
}

impl std::fmt::Display for TemplateVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}.{}", self.major, self.minor)
    }
}

/// The shipped compliance verticals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyTemplate {
    /// HIPAA baseline only. Suitable for non-tribal healthcare orgs
    /// without export-controlled or sovereign-data exposure.
    Standard,
    /// HIPAA + OCAP. For tribal health organisations whose data is
    /// both PHI and tribally owned.
    Healthcare,
    /// ITAR + EAR. For defence / aerospace deployments.
    Defense,
    /// OCAP + HIPAA. For tribal governments whose service area
    /// includes regulated health data.
    TribalGovernment,
}

impl PolicyTemplate {
    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Healthcare => "healthcare",
            Self::Defense => "defense",
            Self::TribalGovernment => "tribal_government",
        }
    }

    /// All shipped templates.
    pub fn all() -> [Self; 4] {
        [
            Self::Standard,
            Self::Healthcare,
            Self::Defense,
            Self::TribalGovernment,
        ]
    }

    /// Modules enabled by this template.
    pub fn enabled_modules(self) -> BTreeSet<ModuleId> {
        let mut set = BTreeSet::new();
        match self {
            Self::Standard => {
                set.insert(ModuleId::Hipaa);
            }
            Self::Healthcare => {
                set.insert(ModuleId::Hipaa);
                set.insert(ModuleId::Ocap);
            }
            Self::Defense => {
                set.insert(ModuleId::Itar);
            }
            Self::TribalGovernment => {
                set.insert(ModuleId::Ocap);
                set.insert(ModuleId::Hipaa);
            }
        }
        set
    }

    /// Priority chain emitted by this template. Always lists every
    /// enabled module of the template; never lists disabled modules.
    pub fn priority(self) -> Vec<ModuleId> {
        match self {
            Self::Standard => vec![ModuleId::Hipaa],
            Self::Defense => vec![ModuleId::Itar],
            // Healthcare and TribalGovernment share the same chain
            // today; if they diverge (e.g. weighting differs), split
            // them then.
            Self::Healthcare | Self::TribalGovernment => vec![ModuleId::Ocap, ModuleId::Hipaa],
        }
    }

    /// Current shipped version of the template. All baseline
    /// templates ship at v1.0; bumps land in the same release that
    /// changes the enabled set or priority chain.
    pub fn version(self) -> TemplateVersion {
        let _ = self;
        TemplateVersion::new(1, 0)
    }

    /// Produce a [`ComposerConfig`] suitable for handing to a fresh
    /// [`super::composer::PolicyComposer`].
    pub fn composer_config(self) -> ComposerConfig {
        ComposerConfig {
            priority: self.priority(),
            enabled: self.enabled_modules(),
        }
    }

    /// Look up a template by its wire-format identifier. Returns
    /// `None` for unknown names so callers can surface a meaningful
    /// error to operators editing config files.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|t| t.as_str() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_enables_hipaa_only() {
        let cfg = PolicyTemplate::Standard.composer_config();
        assert_eq!(cfg.enabled, BTreeSet::from([ModuleId::Hipaa]));
        assert_eq!(cfg.priority, vec![ModuleId::Hipaa]);
    }

    #[test]
    fn healthcare_enables_hipaa_and_ocap() {
        let cfg = PolicyTemplate::Healthcare.composer_config();
        assert_eq!(
            cfg.enabled,
            BTreeSet::from([ModuleId::Hipaa, ModuleId::Ocap])
        );
        // OCAP must rank above HIPAA per the default priority chain.
        assert_eq!(cfg.priority, vec![ModuleId::Ocap, ModuleId::Hipaa]);
    }

    #[test]
    fn defense_enables_itar_only() {
        let cfg = PolicyTemplate::Defense.composer_config();
        assert_eq!(cfg.enabled, BTreeSet::from([ModuleId::Itar]));
        assert_eq!(cfg.priority, vec![ModuleId::Itar]);
    }

    #[test]
    fn tribal_government_enables_ocap_and_hipaa() {
        let cfg = PolicyTemplate::TribalGovernment.composer_config();
        assert_eq!(
            cfg.enabled,
            BTreeSet::from([ModuleId::Ocap, ModuleId::Hipaa])
        );
        assert_eq!(cfg.priority, vec![ModuleId::Ocap, ModuleId::Hipaa]);
    }

    #[test]
    fn priority_only_lists_enabled_modules() {
        for t in PolicyTemplate::all() {
            let cfg = t.composer_config();
            for m in &cfg.priority {
                assert!(
                    cfg.enabled.contains(m),
                    "{t:?}: priority module {m:?} not in enabled set",
                );
            }
            for m in &cfg.enabled {
                assert!(
                    cfg.priority.contains(m),
                    "{t:?}: enabled module {m:?} missing from priority",
                );
            }
        }
    }

    #[test]
    fn from_name_roundtrips_for_every_template() {
        for t in PolicyTemplate::all() {
            assert_eq!(PolicyTemplate::from_name(t.as_str()), Some(t));
        }
    }

    #[test]
    fn from_name_returns_none_on_unknown() {
        assert_eq!(PolicyTemplate::from_name("not-a-template"), None);
    }

    #[test]
    fn templates_carry_a_version() {
        for t in PolicyTemplate::all() {
            let v = t.version();
            assert_eq!(v.major, 1);
            assert_eq!(format!("{v}"), "v1.0");
        }
    }
}
