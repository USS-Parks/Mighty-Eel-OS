//! Adapter discovery and configuration loading.
//!
//! Discovers available adapters by scanning the `adapters/` directory for
//! Python modules containing the `@mai_adapter` decorator. Loads per-adapter
//! configuration from the product tier TOML config files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use mai_core::airgap::ConnectivityState;
use mai_hil::traits::AdapterConfig;

use crate::errors::FrameworkError;
use crate::validation::validate_adapter_host;

/// Discovery result for a single adapter module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredAdapter {
    /// Adapter name (from @mai_adapter decorator).
    pub name: String,
    /// Adapter version string.
    pub version: String,
    /// Path to the adapter's Python module directory.
    pub module_path: PathBuf,
    /// Entry point module (e.g., "adapter" for adapter.py).
    pub entry_module: String,
}

/// Framework-level configuration loaded from product tier TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameworkConfig {
    /// Base directory containing adapter Python modules.
    pub adapters_dir: PathBuf,
    /// Path to Python interpreter.
    pub python_path: PathBuf,
    /// Path to the adapter runner script.
    pub runner_script: PathBuf,
    /// Heartbeat interval in milliseconds.
    pub heartbeat_interval_ms: u64,
    /// Number of missed heartbeats before declaring dead.
    pub missed_heartbeat_threshold: u32,
    /// Maximum restart attempts before giving up.
    pub max_restart_attempts: u32,
    /// Base backoff duration in milliseconds for restart.
    pub base_backoff_ms: u64,
    /// Maximum backoff duration in milliseconds.
    pub max_backoff_ms: u64,
    /// Request timeout in milliseconds.
    pub request_timeout_ms: u64,
    /// Per-adapter configurations.
    pub adapters: HashMap<String, AdapterConfig>,
    /// Cgroups memory limit per adapter (bytes). 0 = no limit.
    pub cgroup_memory_limit: u64,
    /// Cgroups CPU quota per adapter (microseconds per period). 0 = no limit.
    pub cgroup_cpu_quota: u64,
}

impl Default for FrameworkConfig {
    fn default() -> Self {
        Self {
            adapters_dir: PathBuf::from("adapters"),
            python_path: PathBuf::from("python3"),
            runner_script: PathBuf::from("adapters/runner.py"),
            heartbeat_interval_ms: 5000,
            missed_heartbeat_threshold: 3,
            max_restart_attempts: 10,
            base_backoff_ms: 1000,
            max_backoff_ms: 60_000,
            request_timeout_ms: 30_000,
            adapters: HashMap::new(),
            cgroup_memory_limit: 0,
            cgroup_cpu_quota: 0,
        }
    }
}

impl FrameworkConfig {
    /// Load framework configuration from a product tier TOML file.
    pub fn from_toml(path: &Path) -> Result<Self, FrameworkError> {
        let content = std::fs::read_to_string(path).map_err(|e| FrameworkError::ConfigError {
            name: "framework".to_string(),
            reason: format!("Failed to read config file '{}': {e}", path.display()),
        })?;

        let table: toml::Table =
            toml::from_str(&content).map_err(|e| FrameworkError::ConfigError {
                name: "framework".to_string(),
                reason: format!("Invalid TOML in '{}': {e}", path.display()),
            })?;

        // Extract adapter framework section
        let fw_section = table
            .get("adapter_framework")
            .and_then(toml::Value::as_table)
            .cloned()
            .unwrap_or_else(toml::Table::new);

        let mut config = Self::default();

        if let Some(v) = fw_section.get("adapters_dir").and_then(toml::Value::as_str) {
            config.adapters_dir = PathBuf::from(v);
        }
        if let Some(v) = fw_section.get("python_path").and_then(toml::Value::as_str) {
            config.python_path = PathBuf::from(v);
        }
        if let Some(v) = fw_section
            .get("runner_script")
            .and_then(toml::Value::as_str)
        {
            config.runner_script = PathBuf::from(v);
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            if let Some(v) = fw_section
                .get("heartbeat_interval_ms")
                .and_then(toml::Value::as_integer)
            {
                config.heartbeat_interval_ms = v as u64;
            }
            if let Some(v) = fw_section
                .get("missed_heartbeat_threshold")
                .and_then(toml::Value::as_integer)
            {
                config.missed_heartbeat_threshold = v as u32;
            }
            if let Some(v) = fw_section
                .get("max_restart_attempts")
                .and_then(toml::Value::as_integer)
            {
                config.max_restart_attempts = v as u32;
            }
            if let Some(v) = fw_section
                .get("request_timeout_ms")
                .and_then(toml::Value::as_integer)
            {
                config.request_timeout_ms = v as u64;
            }
        }

        info!(
            adapters_dir = %config.adapters_dir.display(),
            heartbeat_ms = config.heartbeat_interval_ms,
            "Loaded adapter framework configuration"
        );

        Ok(config)
    }

    /// Validate every adapter host in this configuration against the
    /// supplied connectivity state and operator-approved allowlist.
    ///
    /// Returns the first failure encountered, keyed by adapter name.
    /// Used by [`crate::manager::AdapterManager`] at config-load time and
    /// after every hot-reload to enforce the air-gap host policy from
    pub fn validate_hosts(
        &self,
        state: ConnectivityState,
        allow_list: &[String],
    ) -> Result<(), FrameworkError> {
        for (name, cfg) in &self.adapters {
            if let Err(err) = validate_adapter_host(&cfg.host, state, allow_list) {
                let fw_err: FrameworkError = err.into();
                tracing::warn!(
                    adapter = %name,
                    host = %cfg.host,
                    state = %state,
                    "adapter host validation failed"
                );
                // Propagate the framework-level error with the adapter
                // name in the message.
                return match fw_err {
                    FrameworkError::ConfigError { reason, .. } => {
                        Err(FrameworkError::ConfigError {
                            name: format!("adapter '{name}'"),
                            reason,
                        })
                    }
                    other => Err(other),
                };
            }
        }
        Ok(())
    }

    /// Return a snapshot of every (adapter_name, host) pair currently
    /// configured. Helpful for diagnostic endpoints that want to display
    /// the host policy state without exposing the full `AdapterConfig`.
    #[must_use]
    pub fn adapter_hosts(&self) -> Vec<(String, String)> {
        self.adapters
            .iter()
            .map(|(name, cfg)| (name.clone(), cfg.host.clone()))
            .collect()
    }

    /// Discover adapters by scanning the adapters directory for Python modules
    /// that contain the `@mai_adapter` decorator.
    pub fn discover_adapters(&self) -> Result<Vec<DiscoveredAdapter>, FrameworkError> {
        let mut discovered = Vec::new();

        let adapters_dir = &self.adapters_dir;
        if !adapters_dir.exists() {
            warn!(dir = %adapters_dir.display(), "Adapters directory not found");
            return Ok(discovered);
        }

        let entries = std::fs::read_dir(adapters_dir).map_err(|e| FrameworkError::ConfigError {
            name: "discovery".to_string(),
            reason: format!("Cannot read adapters dir '{}': {e}", adapters_dir.display()),
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let dir_name = path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("")
                .to_string();

            // Skip __pycache__, tests, hidden dirs
            if dir_name.starts_with('_') || dir_name.starts_with('.') || dir_name == "tests" {
                continue;
            }

            // Check for adapter.py or __init__.py with @mai_adapter
            let adapter_py = path.join("adapter.py");
            let init_py = path.join("__init__.py");

            let entry_module = if adapter_py.exists() {
                "adapter".to_string()
            } else if init_py.exists() {
                "__init__".to_string()
            } else {
                continue;
            };

            // Simple AST-free check: look for @mai_adapter in the file
            let check_file = if entry_module == "adapter" {
                &adapter_py
            } else {
                &init_py
            };

            let Ok(content) = std::fs::read_to_string(check_file) else {
                debug!(path = %check_file.display(), "Cannot read adapter entry file, skipping");
                continue;
            };
            if !content.contains("@mai_adapter") && !content.contains("mai_adapter(") {
                debug!(dir = %dir_name, "No @mai_adapter decorator found, skipping");
                continue;
            }

            // Extract name and version from decorator (simple regex-free parse)
            let (name, version) = parse_adapter_decorator(&content)
                .unwrap_or_else(|| (dir_name.clone(), "0.0.0".to_string()));

            info!(name = %name, version = %version, path = %path.display(), "Discovered adapter");

            discovered.push(DiscoveredAdapter {
                name,
                version,
                module_path: path,
                entry_module,
            });
        }

        Ok(discovered)
    }
}

/// Parse `@mai_adapter(name="...", version="...")` from source text.
/// Returns (name, version) or None if not parseable.
fn parse_adapter_decorator(source: &str) -> Option<(String, String)> {
    // Find the decorator line
    let decorator_start = source.find("@mai_adapter(")?;
    let after_start = &source[decorator_start..];
    let paren_end = after_start.find(')')?;
    let decorator_content = &after_start[..=paren_end];

    // Extract name="..."
    let name = extract_kwarg(decorator_content, "name")?;
    let version =
        extract_kwarg(decorator_content, "version").unwrap_or_else(|| "1.0.0".to_string());

    Some((name, version))
}

/// Extract a keyword argument value from a decorator string.
fn extract_kwarg(content: &str, key: &str) -> Option<String> {
    let pattern = format!("{key}=");
    let start = content.find(&pattern)?;
    let after_eq = &content[start + pattern.len()..];
    let after_eq = after_eq.trim_start();

    // Find the quote character
    let quote = after_eq.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }

    let value_start = 1; // skip opening quote
    let value_end = after_eq[value_start..].find(quote)?;
    Some(after_eq[value_start..value_start + value_end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_adapter_decorator() {
        let source = r#"
@mai_adapter(name="ollama", version="1.0.0")
class OllamaAdapter(AdapterBase):
    pass
"#;
        let (name, version) = parse_adapter_decorator(source).unwrap();
        assert_eq!(name, "ollama");
        assert_eq!(version, "1.0.0");
    }

    #[test]
    fn test_parse_decorator_single_quotes() {
        let source = "@mai_adapter(name='vllm', version='2.0')";
        let (name, version) = parse_adapter_decorator(source).unwrap();
        assert_eq!(name, "vllm");
        assert_eq!(version, "2.0");
    }

    #[test]
    fn test_extract_kwarg() {
        assert_eq!(
            extract_kwarg(r#"(name="test", version="1.0")"#, "name"),
            Some("test".to_string())
        );
        assert_eq!(
            extract_kwarg(r#"(name="test", version="1.0")"#, "version"),
            Some("1.0".to_string())
        );
    }

    #[test]
    fn test_default_config() {
        let config = FrameworkConfig::default();
        assert_eq!(config.heartbeat_interval_ms, 5000);
        assert_eq!(config.missed_heartbeat_threshold, 3);
        assert_eq!(config.max_restart_attempts, 10);
        assert_eq!(config.base_backoff_ms, 1000);
        assert_eq!(config.max_backoff_ms, 60_000);
    }
}
