//! Policy module system.
//!
//! Rules are grouped into named **modules** (HIPAA, ITAR, OCAP, cost
//! control, admin override, ...). Modules can be enabled or disabled at
//! runtime without restarting the process, and the underlying TOML files
//! can be re-read on demand to pick up rule changes (hot reload).
//!
//! All enabled modules contribute rules to a single evaluation; the engine
//! in `super::engine` resolves them by priority and restrictiveness.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::engine::Rule;

/// One named group of rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyModule {
    /// Identifier (e.g. `"hipaa_baseline"`).
    pub name: String,
    /// Whether this module's rules contribute to evaluation.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// The rules in this module.
    #[serde(default)]
    pub rules: Vec<Rule>,
}

fn default_enabled() -> bool {
    true
}

impl PolicyModule {
    /// Build an empty enabled module with the given name.
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            enabled: true,
            rules: Vec::new(),
        }
    }
}

/// Errors from module loading / management.
#[derive(Debug, Error)]
pub enum ModuleError {
    /// Filesystem read failed.
    #[error("cannot read module file {path}: {source}")]
    Io {
        /// Path that failed to read.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// TOML parse failed.
    #[error("invalid module TOML at {path}: {source}")]
    Toml {
        /// Path that failed to parse.
        path: String,
        /// Underlying TOML error.
        source: toml::de::Error,
    },
    /// Module with this name does not exist in the registry.
    #[error("unknown module '{0}'")]
    UnknownModule(String),
}

/// Registry of compliance modules. Hot-reloadable on a per-module basis.
#[derive(Debug, Default)]
pub struct PolicyModuleRegistry {
    modules: RwLock<BTreeMap<String, PolicyModule>>,
}

impl PolicyModuleRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add (or replace) a module by name.
    pub fn install(&self, module: PolicyModule) {
        self.modules
            .write()
            .unwrap()
            .insert(module.name.clone(), module);
    }

    /// Load a module from a TOML file and install it. The TOML may be
    /// either a full `PolicyModule` object (with `name`, `enabled`,
    /// `rules`) or a bare `[[rules]]` array — in the latter case the
    /// supplied `name` is applied.
    pub fn load_from_path(&self, name: &str, path: &Path) -> Result<(), ModuleError> {
        let path_str = path.display().to_string();
        let raw = std::fs::read_to_string(path).map_err(|source| ModuleError::Io {
            path: path_str.clone(),
            source,
        })?;
        let module = parse_module(name, &raw).map_err(|source| ModuleError::Toml {
            path: path_str,
            source,
        })?;
        self.install(module);
        Ok(())
    }

    /// Enable or disable a previously-installed module.
    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<(), ModuleError> {
        let mut modules = self.modules.write().unwrap();
        let module = modules
            .get_mut(name)
            .ok_or_else(|| ModuleError::UnknownModule(name.to_string()))?;
        module.enabled = enabled;
        Ok(())
    }

    /// Names of installed modules in stable order.
    pub fn module_names(&self) -> Vec<String> {
        self.modules.read().unwrap().keys().cloned().collect()
    }

    /// Flatten every enabled module's rules into a single vector. The
    /// caller hands this to `engine::evaluate`.
    pub fn enabled_rules(&self) -> Vec<Rule> {
        let modules = self.modules.read().unwrap();
        let mut out = Vec::new();
        for module in modules.values() {
            if module.enabled {
                out.extend(module.rules.iter().cloned());
            }
        }
        out
    }

    /// Snapshot of all installed modules.
    pub fn snapshot(&self) -> Vec<PolicyModule> {
        self.modules.read().unwrap().values().cloned().collect()
    }
}

fn parse_module(name: &str, raw: &str) -> Result<PolicyModule, toml::de::Error> {
    // Try the full PolicyModule shape first.
    if let Ok(module) = toml::from_str::<PolicyModule>(raw) {
        return Ok(module);
    }
    // Fall back: rules-only shape, e.g. `[[rules]]\n...\n[[rules]]\n...`.
    #[derive(Deserialize)]
    struct RulesOnly {
        rules: Vec<Rule>,
    }
    let rules_only: RulesOnly = toml::from_str(raw)?;
    Ok(PolicyModule {
        name: name.to_string(),
        enabled: true,
        rules: rules_only.rules,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::engine::{Action, Condition, Operator, Value};

    fn sample_rule(name: &str) -> Rule {
        Rule {
            name: name.into(),
            priority: 100,
            condition: Condition::Match {
                field: "classification".into(),
                op: Operator::Equals,
                value: Value::Str("regulated".into()),
            },
            action: Action::Deny {
                reason: "phi".into(),
                code: "TEST".into(),
            },
            audit_level: super::super::engine::AuditLevel::Warn,
        }
    }

    #[test]
    fn test_install_and_enabled_rules() {
        let reg = PolicyModuleRegistry::new();
        let module = PolicyModule {
            name: "test".into(),
            enabled: true,
            rules: vec![sample_rule("r1")],
        };
        reg.install(module);
        let rules = reg.enabled_rules();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "r1");
    }

    #[test]
    fn test_disabled_module_contributes_no_rules() {
        let reg = PolicyModuleRegistry::new();
        reg.install(PolicyModule {
            name: "off".into(),
            enabled: false,
            rules: vec![sample_rule("hidden")],
        });
        assert!(reg.enabled_rules().is_empty());
    }

    #[test]
    fn test_set_enabled_toggles_visibility() {
        let reg = PolicyModuleRegistry::new();
        reg.install(PolicyModule {
            name: "toggle".into(),
            enabled: true,
            rules: vec![sample_rule("r")],
        });
        assert_eq!(reg.enabled_rules().len(), 1);
        reg.set_enabled("toggle", false).unwrap();
        assert_eq!(reg.enabled_rules().len(), 0);
        reg.set_enabled("toggle", true).unwrap();
        assert_eq!(reg.enabled_rules().len(), 1);
    }

    #[test]
    fn test_set_enabled_unknown_errors() {
        let reg = PolicyModuleRegistry::new();
        assert!(matches!(
            reg.set_enabled("ghost", true),
            Err(ModuleError::UnknownModule(_))
        ));
    }

    #[test]
    fn test_parse_module_full_shape() {
        let toml_text = r#"
name = "hipaa_baseline"
enabled = true

[[rules]]
name = "phi_deny_cloud"
priority = 100
audit_level = "warn"

[rules.condition]
type = "match"
field = "classification"
op = "equals"
value = "regulated"

[rules.action]
kind = "deny"
reason = "PHI cannot route to cloud"
code = "HIPAA-PHI-DENY"
"#;
        let m = parse_module("ignored", toml_text).unwrap();
        assert_eq!(m.name, "hipaa_baseline");
        assert!(m.enabled);
        assert_eq!(m.rules.len(), 1);
    }

    #[test]
    fn test_parse_module_rules_only_uses_provided_name() {
        let toml_text = r#"
[[rules]]
name = "rule1"
priority = 50
audit_level = "info"

[rules.condition]
type = "match"
field = "role"
op = "equals"
value = "guest"

[rules.action]
kind = "flag"
reason = "guest activity"
"#;
        let m = parse_module("supplied_name", toml_text).unwrap();
        assert_eq!(m.name, "supplied_name");
        assert_eq!(m.rules.len(), 1);
    }

    // Tiny stand-in for tempfile so we do not pull a new dep in.
    struct TestTmpDir {
        path: std::path::PathBuf,
    }
    impl TestTmpDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("mai_router_modules_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
        fn join(&self, name: &str) -> std::path::PathBuf {
            self.path.join(name)
        }
    }
    impl Drop for TestTmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn test_load_from_path_round_trip() {
        let tmp = TestTmpDir::new();
        let path = tmp.join("mod.toml");
        let toml_text = "[[rules]]\nname = \"x\"\npriority = 1\naudit_level = \"info\"\n\n[rules.condition]\ntype = \"match\"\nfield = \"role\"\nop = \"equals\"\nvalue = \"admin\"\n\n[rules.action]\nkind = \"allow\"\n";
        std::fs::write(&path, toml_text).unwrap();
        let reg = PolicyModuleRegistry::new();
        reg.load_from_path("loaded", &path).unwrap();
        assert_eq!(reg.module_names(), vec!["loaded"]);
        assert_eq!(reg.enabled_rules().len(), 1);
    }
}
