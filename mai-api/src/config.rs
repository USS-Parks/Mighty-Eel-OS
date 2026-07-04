//! Server configuration with product tier defaults, TOML loading,
//! and hot-reload support.
//!
//! The configuration system enforces air-gap-safe defaults:
//! - Bind address is 127.0.0.1 only (no external exposure)
//! - Default port is 8420
//! - Tier presets match Scout/Ranger/Pack Leader hardware profiles

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tracing::{error, info, warn};

use crate::rate_limit::{BucketConfig, RateLimiter};

/// Complete server configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Server network configuration
    #[serde(default)]
    pub server: NetworkConfig,
    /// Product tier (determines resource limits)
    #[serde(default)]
    pub tier: ProductTier,
    /// Request limits
    #[serde(default)]
    pub limits: RequestLimits,
    /// Profile store configuration
    #[serde(default)]
    pub profiles: ProfileStoreConfig,
    /// Audit log configuration
    #[serde(default)]
    pub audit: AuditConfig,
    /// Air-gap verification configuration
    #[serde(default)]
    pub air_gap: AirGapConfig,
    /// TLS configuration (optional; plaintext on localhost is acceptable)
    #[serde(default)]
    pub tls: Option<TlsConfig>,
}

/// Network binding configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// REST API port (default: 8420)
    #[serde(default = "default_port")]
    pub port: u16,
    /// gRPC port (default: 8421)
    #[serde(default = "default_grpc_port")]
    pub grpc_port: u16,
    /// Bind address (default: 127.0.0.1, NEVER 0.0.0.0 in production)
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            grpc_port: default_grpc_port(),
            bind_address: default_bind_address(),
        }
    }
}

fn default_port() -> u16 {
    8420
}
fn default_grpc_port() -> u16 {
    8421
}
fn default_bind_address() -> String {
    "127.0.0.1".to_string()
}

/// Product tier determines default resource limits
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProductTier {
    /// Single GPU, conservative limits, Ollama primary
    #[default]
    Scout,
    /// Dual GPU, multi-adapter, higher limits
    Ranger,
    /// 4+ GPU, full adapter fleet, maximum limits
    PackLeader,
}

/// Request processing limits (tier-dependent defaults)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLimits {
    /// Maximum concurrent inference requests
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_requests: usize,
    /// Maximum request body size in bytes
    #[serde(default = "default_max_body_size")]
    pub max_body_size_bytes: usize,
    /// Default request timeout
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    /// Maximum streaming duration
    #[serde(default = "default_stream_timeout_secs")]
    pub stream_timeout_secs: u64,
    /// Queue depth before backpressure activates
    #[serde(default = "default_backpressure_threshold")]
    pub backpressure_threshold: usize,
    /// Optional per-route token-bucket rate limits. Empty means disabled.
    #[serde(default)]
    pub route_rate_limits: Vec<RouteRateLimit>,
}

impl Default for RequestLimits {
    fn default() -> Self {
        Self {
            max_concurrent_requests: default_max_concurrent(),
            max_body_size_bytes: default_max_body_size(),
            request_timeout_secs: default_request_timeout_secs(),
            stream_timeout_secs: default_stream_timeout_secs(),
            backpressure_threshold: default_backpressure_threshold(),
            route_rate_limits: Vec::new(),
        }
    }
}

fn default_max_concurrent() -> usize {
    10
}
fn default_max_body_size() -> usize {
    10 * 1024 * 1024
} // 10 MiB
fn default_request_timeout_secs() -> u64 {
    120
}
fn default_stream_timeout_secs() -> u64 {
    600
}
fn default_backpressure_threshold() -> usize {
    80
}

/// One per-prefix token-bucket config entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRateLimit {
    /// Route prefix to match (e.g. "/v1/chat", "/v1/health").
    pub prefix: String,
    /// Bucket capacity (max burst size).
    pub capacity: u32,
    /// Refill rate in tokens per second.
    pub refill_per_sec: f64,
}

/// Profile store configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileStoreConfig {
    /// Path to SQLite profile database
    #[serde(default = "default_profile_db_path")]
    pub db_path: PathBuf,
}

impl Default for ProfileStoreConfig {
    fn default() -> Self {
        Self {
            db_path: default_profile_db_path(),
        }
    }
}

fn default_profile_db_path() -> PathBuf {
    PathBuf::from("/var/lib/mai/profiles.db")
}

/// Audit logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// Path to audit log directory
    #[serde(default = "default_audit_path")]
    pub log_path: PathBuf,
    /// Retention period in days (default: 90)
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    /// Enable hash chain integrity verification
    #[serde(default = "default_true")]
    pub hash_chain_enabled: bool,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            log_path: default_audit_path(),
            retention_days: default_retention_days(),
            hash_chain_enabled: true,
        }
    }
}

fn default_audit_path() -> PathBuf {
    PathBuf::from("/var/lib/mai/audit")
}
fn default_retention_days() -> u32 {
    90
}
fn default_true() -> bool {
    true
}

/// Air-gap verification configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirGapConfig {
    /// Path to air-gap switch device (hardware-specific)
    #[serde(default = "default_switch_device")]
    pub switch_device_path: PathBuf,
    /// Re-verification interval in seconds (default: 60)
    #[serde(default = "default_check_interval_secs")]
    pub check_interval_secs: u64,
    /// Whether to block startup on air-gap violation
    #[serde(default = "default_true")]
    pub enforce_on_startup: bool,
}

impl Default for AirGapConfig {
    fn default() -> Self {
        Self {
            switch_device_path: default_switch_device(),
            check_interval_secs: default_check_interval_secs(),
            enforce_on_startup: true,
        }
    }
}

fn default_switch_device() -> PathBuf {
    PathBuf::from("/dev/im-airgap-switch")
}
fn default_check_interval_secs() -> u64 {
    60
}

/// TLS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    /// Path to TLS certificate (PEM)
    pub cert_path: PathBuf,
    /// Path to TLS private key (PEM)
    pub key_path: PathBuf,
}

impl ServerConfig {
    /// Apply product tier defaults, overriding limits that weren't
    /// explicitly set in the TOML file.
    pub fn apply_tier_defaults(&mut self) {
        match self.tier {
            ProductTier::Scout => {
                // Conservative: single adapter, low concurrency
                if self.limits.max_concurrent_requests == default_max_concurrent() {
                    self.limits.max_concurrent_requests = 8;
                }
                if self.limits.backpressure_threshold == default_backpressure_threshold() {
                    self.limits.backpressure_threshold = 40;
                }
            }
            ProductTier::Ranger => {
                // Moderate: multi-adapter, higher concurrency
                if self.limits.max_concurrent_requests == default_max_concurrent() {
                    self.limits.max_concurrent_requests = 32;
                }
                if self.limits.backpressure_threshold == default_backpressure_threshold() {
                    self.limits.backpressure_threshold = 100;
                }
            }
            ProductTier::PackLeader => {
                // Maximum: full fleet, high concurrency
                if self.limits.max_concurrent_requests == default_max_concurrent() {
                    self.limits.max_concurrent_requests = 128;
                }
                if self.limits.backpressure_threshold == default_backpressure_threshold() {
                    self.limits.backpressure_threshold = 300;
                }
            }
        }
    }

    /// Validate configuration for consistency and safety.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Bind address safety: refuse every wildcard "any" address. These
        // bind to every interface and have no legitimate use in this
        // codebase. strengthened this from "0.0.0.0 only" to
        // "any IPv4/IPv6 unspecified address".
        let bind = self.server.bind_address.trim();
        if bind == "0.0.0.0" || bind == "::" || bind == "[::]" {
            return Err(ConfigError::UnsafeBind(format!(
                "Binding to wildcard address {bind:?} is forbidden; use 127.0.0.1 or ::1"
            )));
        }

        // Port range validation
        if self.server.port == 0 {
            return Err(ConfigError::InvalidValue(
                "server.port must be > 0".to_string(),
            ));
        }

        // Timeout sanity
        if self.limits.request_timeout_secs == 0 {
            return Err(ConfigError::InvalidValue(
                "limits.request_timeout_secs must be > 0".to_string(),
            ));
        }

        if self.limits.stream_timeout_secs < self.limits.request_timeout_secs {
            return Err(ConfigError::InvalidValue(
                "stream_timeout must be >= request_timeout".to_string(),
            ));
        }

        for entry in &self.limits.route_rate_limits {
            let prefix = entry.prefix.trim();
            if prefix.is_empty() || !prefix.starts_with('/') {
                return Err(ConfigError::InvalidValue(format!(
                    "limits.route_rate_limits.prefix must start with '/', got {:?}",
                    entry.prefix
                )));
            }
            if entry.capacity == 0 {
                return Err(ConfigError::InvalidValue(format!(
                    "limits.route_rate_limits.capacity must be > 0 for prefix {prefix:?}"
                )));
            }
            if !(entry.refill_per_sec.is_finite() && entry.refill_per_sec > 0.0) {
                return Err(ConfigError::InvalidValue(format!(
                    "limits.route_rate_limits.refill_per_sec must be finite and > 0 for prefix {prefix:?}"
                )));
            }
        }

        Ok(())
    }

    /// Build the optional per-route rate limiter configured under
    /// [`RequestLimits::route_rate_limits`].
    ///
    /// Returns `Ok(None)` when the config disables rate limiting.
    pub fn build_route_rate_limiter(&self) -> Result<Option<RateLimiter>, ConfigError> {
        self.validate()?;
        if self.limits.route_rate_limits.is_empty() {
            return Ok(None);
        }
        let routes: Vec<(String, BucketConfig)> = self
            .limits
            .route_rate_limits
            .iter()
            .map(|e| {
                (
                    e.prefix.clone(),
                    BucketConfig {
                        capacity: e.capacity,
                        refill_per_sec: e.refill_per_sec,
                    },
                )
            })
            .collect();
        Ok(Some(RateLimiter::new(&routes)))
    }

    /// Like [`Self::validate`] but additionally enforces that, under any
    /// state that disallows network traffic ([`ConnectivityState::AirGapped`]
    /// or [`ConnectivityState::Expired`]), the bind address is a loopback
    /// literal. Used by the API server at startup and after every
    /// connectivity-state transition.
    pub fn validate_with_connectivity(
        &self,
        state: mai_core::airgap::ConnectivityState,
    ) -> Result<(), ConfigError> {
        self.validate()?;
        if state.requires_local_only()
            && !mai_adapters::validation::is_loopback(self.server.bind_address.trim())
        {
            return Err(ConfigError::UnsafeBind(format!(
                "Bind address {:?} is non-loopback but connectivity state is {state}; \
                 loopback (127.0.0.1 or ::1) is required",
                self.server.bind_address
            )));
        }
        Ok(())
    }
}

/// Configuration loading errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    IoError(String),
    #[error("Failed to parse TOML: {0}")]
    ParseError(String),
    #[error("Invalid configuration value: {0}")]
    InvalidValue(String),
    #[error("Unsafe bind address: {0}")]
    UnsafeBind(String),
}

/// Load configuration from a TOML file path, apply tier defaults,
/// and validate.
pub fn load_config(path: &Path) -> Result<ServerConfig, ConfigError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ConfigError::IoError(format!("{}: {e}", path.display())))?;

    let mut config: ServerConfig =
        toml::from_str(&content).map_err(|e| ConfigError::ParseError(e.to_string()))?;

    config.apply_tier_defaults();
    config.validate()?;

    info!(
        tier = ?config.tier,
        port = config.server.port,
        max_concurrent = config.limits.max_concurrent_requests,
        "Configuration loaded"
    );

    Ok(config)
}

/// Load configuration or fall back to defaults for the given tier.
pub fn load_or_default(path: Option<&Path>, tier: ProductTier) -> ServerConfig {
    if let Some(p) = path {
        match load_config(p) {
            Ok(config) => return config,
            Err(e) => {
                warn!(error = %e, path = %p.display(), "Failed to load config, using defaults");
            }
        }
    }

    let mut config = ServerConfig {
        tier,
        ..ServerConfig::default()
    };
    config.apply_tier_defaults();
    config
}

/// Configuration watcher for hot-reload support.
///
/// Watches a TOML config file for changes and sends updated
/// configurations through a channel. Only non-breaking changes
/// are applied (port changes require restart).
pub struct ConfigWatcher {
    config_path: PathBuf,
    sender: watch::Sender<Arc<ServerConfig>>,
    receiver: watch::Receiver<Arc<ServerConfig>>,
}

impl ConfigWatcher {
    /// Create a new config watcher for the given path.
    pub fn new(config_path: PathBuf, initial: ServerConfig) -> Self {
        let (sender, receiver) = watch::channel(Arc::new(initial));
        Self {
            config_path,
            sender,
            receiver,
        }
    }

    /// Get a receiver that yields updated configs when the file changes.
    pub fn subscribe(&self) -> watch::Receiver<Arc<ServerConfig>> {
        self.receiver.clone()
    }

    /// Attempt to reload the configuration file. Returns Ok(true) if
    /// the config changed, Ok(false) if unchanged, Err on failure.
    pub fn try_reload(&self) -> Result<bool, ConfigError> {
        let new_config = load_config(&self.config_path)?;
        let current = self.receiver.borrow();

        // Check if port changed (requires restart, not hot-reloadable)
        if new_config.server.port != current.server.port {
            warn!(
                old_port = current.server.port,
                new_port = new_config.server.port,
                "Port change detected; restart required for this to take effect"
            );
        }

        // Send the new config regardless (consumers decide what to apply)
        if self.sender.send(Arc::new(new_config)).is_err() {
            error!("All config subscribers dropped");
        }

        Ok(true)
    }

    /// Start watching the config file for changes. Runs until cancelled.
    /// Uses polling (not inotify) for maximum portability.
    pub async fn watch_loop(&self, poll_interval: Duration) {
        let mut last_modified = std::fs::metadata(&self.config_path)
            .and_then(|m| m.modified())
            .ok();

        loop {
            tokio::time::sleep(poll_interval).await;

            let current_modified = std::fs::metadata(&self.config_path)
                .and_then(|m| m.modified())
                .ok();

            if current_modified != last_modified {
                info!(path = %self.config_path.display(), "Config file changed, reloading");
                match self.try_reload() {
                    Ok(true) => {
                        info!("Configuration reloaded successfully");
                        last_modified = current_modified;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        error!(error = %e, "Failed to reload configuration");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_valid() {
        let config = ServerConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_scout_tier_defaults() {
        let mut config = ServerConfig {
            tier: ProductTier::Scout,
            ..ServerConfig::default()
        };
        config.apply_tier_defaults();
        assert_eq!(config.limits.max_concurrent_requests, 8);
        assert_eq!(config.limits.backpressure_threshold, 40);
    }

    #[test]
    fn test_ranger_tier_defaults() {
        let mut config = ServerConfig {
            tier: ProductTier::Ranger,
            ..ServerConfig::default()
        };
        config.apply_tier_defaults();
        assert_eq!(config.limits.max_concurrent_requests, 32);
    }

    #[test]
    fn test_pack_leader_tier_defaults() {
        let mut config = ServerConfig {
            tier: ProductTier::PackLeader,
            ..ServerConfig::default()
        };
        config.apply_tier_defaults();
        assert_eq!(config.limits.max_concurrent_requests, 128);
    }

    #[test]
    fn test_reject_wildcard_bind() {
        let mut config = ServerConfig::default();
        config.server.bind_address = "0.0.0.0".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_reject_ipv6_wildcard_bind() {
        // strengthened: IPv6 unspecified must also be rejected.
        let mut config = ServerConfig::default();
        config.server.bind_address = "::".to_string();
        assert!(matches!(config.validate(), Err(ConfigError::UnsafeBind(_))));

        config.server.bind_address = "[::]".to_string();
        assert!(matches!(config.validate(), Err(ConfigError::UnsafeBind(_))));
    }

    #[test]
    fn test_validate_with_connectivity_rejects_non_loopback_under_airgap() {
        use mai_core::airgap::ConnectivityState;
        let mut config = ServerConfig::default();
        config.server.bind_address = "10.0.0.5".to_string();

        // Connected: passes (just basic safety, no allowlist enforced at API
        // layer — that's a separate operator policy).
        assert!(
            config
                .validate_with_connectivity(ConnectivityState::Connected)
                .is_ok()
        );

        // AirGapped: must reject non-loopback.
        assert!(matches!(
            config.validate_with_connectivity(ConnectivityState::AirGapped),
            Err(ConfigError::UnsafeBind(_))
        ));

        // Expired (local-only): same as AirGapped.
        assert!(matches!(
            config.validate_with_connectivity(ConnectivityState::Expired),
            Err(ConfigError::UnsafeBind(_))
        ));

        // Loopback always passes regardless of state.
        config.server.bind_address = "127.0.0.1".to_string();
        for state in [
            ConnectivityState::Connected,
            ConnectivityState::AirGapped,
            ConnectivityState::Expired,
        ] {
            assert!(config.validate_with_connectivity(state).is_ok());
        }
    }

    #[test]
    fn test_reject_zero_timeout() {
        let mut config = ServerConfig::default();
        config.limits.request_timeout_secs = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_stream_timeout_must_exceed_request() {
        let mut config = ServerConfig::default();
        config.limits.request_timeout_secs = 300;
        config.limits.stream_timeout_secs = 100;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_toml_roundtrip() {
        let config = ServerConfig::default();
        let serialized = toml::to_string(&config).unwrap();
        let deserialized: ServerConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.server.port, 8420);
        assert_eq!(deserialized.tier, ProductTier::Scout);
    }

    #[test]
    fn test_load_or_default_missing_file() {
        let config = load_or_default(Some(Path::new("/nonexistent")), ProductTier::Ranger);
        assert_eq!(config.tier, ProductTier::Ranger);
        assert_eq!(config.limits.max_concurrent_requests, 32);
    }
}
