//! Ship profile: typed representation + parse-time validation for the
//! `deployment/ship/profile.toml` schema documented in
//! `mai/docs/SHIP-HARDENING-PLAN.md` §3 (Workstream 1).
//!
//! Scope (SHIP-01):
//! - Define the schema as Rust types.
//! - Parse the TOML.
//! - Reject obviously-unsafe shapes at parse time (no demo defaults,
//!   no stub vault, no memory audit writer, no accept-all verifier,
//!   no wildcard bind, required persistent paths, required trust
//!   anchor / bundle-on-boot, required non-empty key store).
//!
//! Out of scope (SHIP-01):
//! - Wiring into `ServerConfig` or `MaiServer` startup. The runtime
//!   guard with check IDs lands in SHIP-02 (`production_guard.rs`).
//! - Filesystem existence checks for the configured paths. Those run
//!   inside the runtime guard once it owns the boot path.
//! - The validator CLI. That ships in SHIP-07.
//!
//! Profile modes:
//! - `production`: the customer-facing posture. Every rule below is
//!   enforced at parse time.
//! - `local-dev`: developer convenience. Schema is parsed and the
//!   meta is validated, but the production-only invariants are
//!   skipped so the same struct can be reused under a relaxed
//!   profile during bring-up.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Top-level ship-profile document. Mirrors the section layout in
/// `deployment/ship/profile.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShipProfile {
    pub profile: ProfileMeta,
    pub paths: PathsConfig,
    pub vault: VaultConfig,
    pub audit: AuditConfig,
    pub trust: TrustConfig,
    pub auth: AuthConfig,
    pub dashboard: DashboardConfig,
    pub network: NetworkConfig,
    pub observability: ObservabilityConfig,
    /// Optional `[openbao]` bridge configuration. When present, the
    /// server wires the OpenBao bridge client from this section
    /// instead of falling back to `OpenBaoBridgeConfig::staging()`.
    /// Required when the exchange mode is `OpenBaoBridge`.
    #[serde(default)]
    pub openbao: Option<OpenbaoConfig>,
}

/// `[profile]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileMeta {
    pub name: String,
    pub mode: ProfileMode,
    #[serde(default)]
    pub allow_demo_defaults: bool,
    #[serde(default)]
    pub fail_closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileMode {
    Production,
    LocalDev,
}

/// `[paths]` section. All directories are required in production mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsConfig {
    pub state_dir: PathBuf,
    pub config_dir: PathBuf,
    pub log_dir: PathBuf,
    pub run_dir: PathBuf,
    pub backup_dir: PathBuf,
}

/// `[vault]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    pub backend: VaultBackend,
    pub root: PathBuf,
    #[serde(default)]
    pub require_sealed_master_key: bool,
    #[serde(default)]
    pub require_pqc: bool,
    #[serde(default)]
    pub allow_stub: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VaultBackend {
    Stub,
    Zfs,
    FileDev,
}

/// `[audit]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    pub api_writer: AuditWriter,
    pub compliance_writer: AuditWriter,
    pub wal_dir: PathBuf,
    #[serde(default)]
    pub require_hash_chain: bool,
    #[serde(default)]
    pub require_pqc_checkpoints: bool,
    #[serde(default)]
    pub require_encryption_at_rest: bool,
    #[serde(default)]
    pub allow_memory_writer: bool,
    #[serde(default)]
    pub allow_null_sealer: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuditWriter {
    Memory,
    Wal,
}

/// `[trust]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustConfig {
    pub anchors_dir: PathBuf,
    pub bundle_cache_dir: PathBuf,
    pub verifier: TrustVerifier,
    #[serde(default)]
    pub allow_accept_all_verifier: bool,
    #[serde(default)]
    pub allow_local_dev_exchange: bool,
    #[serde(default)]
    pub require_trust_anchor: bool,
    #[serde(default)]
    pub require_bundle_on_boot: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrustVerifier {
    AcceptAll,
    MlDsa,
}

/// `[auth]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub auth_keys_path: PathBuf,
    #[serde(default)]
    pub allow_internal_profile_header: bool,
    #[serde(default)]
    pub require_nonempty_key_store: bool,
}

/// `[dashboard]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allow_default_admin_token: bool,
}

/// `[network]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub bind_address: String,
    pub tls_mode: TlsMode,
    #[serde(default)]
    pub require_forwarded_proto_header: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TlsMode {
    ReverseProxyRequired,
    Direct,
}

/// `[observability]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityConfig {
    pub log_format: LogFormat,
    #[serde(default)]
    pub log_rotation: bool,
    pub metrics_exporter: MetricsExporter,
    #[serde(default)]
    pub alerts_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LogFormat {
    Json,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetricsExporter {
    None,
    Prometheus,
}

/// `[openbao]` section — bridge configuration for the Ring-1 ↔ Ring-3
/// trust bridge. Secrets (secret_id, wrapped_secret_id) are NEVER stored
/// here; they come from environment variables at runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenbaoConfig {
    pub address: String,
    pub role_id: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub transit: TransitKeysConfig,
    #[serde(default)]
    pub kv: KvPathConfig,
    #[serde(default)]
    pub pki: PkiRoleConfig,
    #[serde(default)]
    pub trust_refresh: TrustRefreshConfig,
}

fn default_timeout_secs() -> u64 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitKeysConfig {
    #[serde(default = "default_claim_signer_key")]
    pub claim_signer_key: String,
    #[serde(default = "default_bundle_signer_key")]
    pub bundle_signer_key: String,
    #[serde(default = "default_revocation_signer_key")]
    pub revocation_signer_key: String,
}

impl Default for TransitKeysConfig {
    fn default() -> Self {
        Self {
            claim_signer_key: default_claim_signer_key(),
            bundle_signer_key: default_bundle_signer_key(),
            revocation_signer_key: default_revocation_signer_key(),
        }
    }
}

fn default_claim_signer_key() -> String {
    "lamprey-claim-signer".into()
}
fn default_bundle_signer_key() -> String {
    "lamprey-bundle-signer".into()
}
fn default_revocation_signer_key() -> String {
    "lamprey-revocation-signer".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KvPathConfig {
    #[serde(default = "default_tenant_path")]
    pub tenant_path: String,
    #[serde(default = "default_revocation_path")]
    pub revocation_path: String,
}

impl Default for KvPathConfig {
    fn default() -> Self {
        Self {
            tenant_path: default_tenant_path(),
            revocation_path: default_revocation_path(),
        }
    }
}

fn default_tenant_path() -> String {
    "kv/tenants".into()
}
fn default_revocation_path() -> String {
    "kv/revocations".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PkiRoleConfig {
    #[serde(default = "default_pki_role")]
    pub role: String,
}

impl Default for PkiRoleConfig {
    fn default() -> Self {
        Self {
            role: default_pki_role(),
        }
    }
}

fn default_pki_role() -> String {
    "mai-appliance".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustRefreshConfig {
    #[serde(default = "default_refresh_enabled")]
    pub enabled: bool,
    #[serde(default = "default_refresh_interval_secs")]
    pub interval_secs: u64,
}

impl Default for TrustRefreshConfig {
    fn default() -> Self {
        Self {
            enabled: default_refresh_enabled(),
            interval_secs: default_refresh_interval_secs(),
        }
    }
}

fn default_refresh_enabled() -> bool {
    true
}
fn default_refresh_interval_secs() -> u64 {
    300
}

/// Errors produced by [`load_ship_profile`] and [`ShipProfile::validate`].
#[derive(Debug, thiserror::Error)]
pub enum ShipProfileError {
    #[error("Failed to read ship profile {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Failed to parse ship profile TOML: {0}")]
    Parse(String),
    #[error("Ship profile validation failed: {0}")]
    Validation(String),
}

impl ShipProfile {
    /// True when the profile is the customer-facing production posture.
    pub fn is_production(&self) -> bool {
        self.profile.mode == ProfileMode::Production
    }

    /// Run parse-time validation. In production mode this enforces
    /// every rule called out in SHIP-HARDENING-PLAN.md §1.1 that can be
    /// checked from the TOML alone. In local-dev mode only the
    /// profile-meta sanity checks run; demo defaults are allowed.
    pub fn validate(&self) -> Result<(), ShipProfileError> {
        if self.profile.name.trim().is_empty() {
            return Err(ShipProfileError::Validation(
                "profile.name must not be empty".to_string(),
            ));
        }
        if self.is_production() {
            self.validate_production()?;
        }
        Ok(())
    }

    fn validate_production(&self) -> Result<(), ShipProfileError> {
        // -- [profile] meta --------------------------------------------------
        if self.profile.allow_demo_defaults {
            return Err(reject(
                "profile.allow_demo_defaults must be false in production mode",
            ));
        }
        if !self.profile.fail_closed {
            return Err(reject(
                "profile.fail_closed must be true in production mode",
            ));
        }

        // -- [paths] ---------------------------------------------------------
        check_path(&self.paths.state_dir, "paths.state_dir")?;
        check_path(&self.paths.config_dir, "paths.config_dir")?;
        check_path(&self.paths.log_dir, "paths.log_dir")?;
        check_path(&self.paths.run_dir, "paths.run_dir")?;
        check_path(&self.paths.backup_dir, "paths.backup_dir")?;

        // -- [vault] ---------------------------------------------------------
        if self.vault.allow_stub {
            return Err(reject("vault.allow_stub must be false in production mode"));
        }
        if matches!(self.vault.backend, VaultBackend::Stub) {
            return Err(reject(
                "vault.backend must not be \"stub\" in production mode",
            ));
        }
        check_path(&self.vault.root, "vault.root")?;

        // -- [audit] ---------------------------------------------------------
        if self.audit.allow_memory_writer {
            return Err(reject(
                "audit.allow_memory_writer must be false in production mode",
            ));
        }
        if matches!(self.audit.api_writer, AuditWriter::Memory) {
            return Err(reject(
                "audit.api_writer must not be \"memory\" in production mode",
            ));
        }
        if matches!(self.audit.compliance_writer, AuditWriter::Memory) {
            return Err(reject(
                "audit.compliance_writer must not be \"memory\" in production mode",
            ));
        }
        check_path(&self.audit.wal_dir, "audit.wal_dir")?;
        if self.audit.allow_null_sealer {
            return Err(reject(
                "audit.allow_null_sealer must be false in production mode",
            ));
        }

        // -- [trust] ---------------------------------------------------------
        if self.trust.allow_accept_all_verifier {
            return Err(reject(
                "trust.allow_accept_all_verifier must be false in production mode",
            ));
        }
        if matches!(self.trust.verifier, TrustVerifier::AcceptAll) {
            return Err(reject(
                "trust.verifier must not be \"accept-all\" in production mode",
            ));
        }
        if self.trust.allow_local_dev_exchange {
            return Err(reject(
                "trust.allow_local_dev_exchange must be false in production mode",
            ));
        }
        if !self.trust.require_trust_anchor {
            return Err(reject(
                "trust.require_trust_anchor must be true in production mode",
            ));
        }
        check_path(&self.trust.anchors_dir, "trust.anchors_dir")?;
        check_path(&self.trust.bundle_cache_dir, "trust.bundle_cache_dir")?;
        if !self.trust.require_bundle_on_boot {
            return Err(reject(
                "trust.require_bundle_on_boot must be true in production mode",
            ));
        }

        // -- [auth] ----------------------------------------------------------
        if self.auth.allow_internal_profile_header {
            return Err(reject(
                "auth.allow_internal_profile_header must be false in production mode",
            ));
        }
        if !self.auth.require_nonempty_key_store {
            return Err(reject(
                "auth.require_nonempty_key_store must be true in production mode",
            ));
        }
        check_path(&self.auth.auth_keys_path, "auth.auth_keys_path")?;

        // -- [dashboard] -----------------------------------------------------
        if self.dashboard.allow_default_admin_token {
            return Err(reject(
                "dashboard.allow_default_admin_token must be false in production mode",
            ));
        }

        // -- [network] -------------------------------------------------------
        let bind = self.network.bind_address.trim();
        if bind.is_empty() {
            return Err(reject("network.bind_address must not be empty"));
        }
        if bind == "0.0.0.0" || bind == "::" || bind == "[::]" {
            return Err(reject(&format!(
                "network.bind_address {bind:?} is a wildcard; production must bind loopback"
            )));
        }

        // -- [observability] -------------------------------------------------
        // No production-only invariants on observability at SHIP-01.
        // Alert wiring lands in SHIP-11; until then the fields parse
        // and are stored for downstream sessions to consume.

        Ok(())
    }
}

fn check_path(p: &Path, name: &str) -> Result<(), ShipProfileError> {
    if p.as_os_str().is_empty() {
        return Err(reject(&format!("{name} must not be empty")));
    }
    Ok(())
}

fn reject(msg: &str) -> ShipProfileError {
    ShipProfileError::Validation(msg.to_string())
}

/// Load a ship profile from disk and validate it.
pub fn load_ship_profile(path: &Path) -> Result<ShipProfile, ShipProfileError> {
    let content = std::fs::read_to_string(path).map_err(|source| ShipProfileError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_ship_profile(&content)
}

/// Parse a ship profile from an in-memory TOML string and validate it.
/// Exposed so unit tests and the SHIP-07 validator CLI can reuse the
/// same parse path without touching disk.
pub fn parse_ship_profile(content: &str) -> Result<ShipProfile, ShipProfileError> {
    let profile: ShipProfile =
        toml::from_str(content).map_err(|e| ShipProfileError::Parse(e.to_string()))?;
    profile.validate()?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical happy-path ship profile used as a baseline by the
    /// negative-case tests. Each test mutates one field and asserts
    /// the validator catches the violation.
    fn baseline() -> &'static str {
        r#"
[profile]
name = "ship"
mode = "production"
allow_demo_defaults = false
fail_closed = true

[paths]
state_dir = "/var/lib/mai"
config_dir = "/etc/mai"
log_dir = "/var/log/mai"
run_dir = "/run/mai"
backup_dir = "/var/backups/mai"

[vault]
backend = "zfs"
root = "/var/lib/mai/vault"
require_sealed_master_key = true
require_pqc = true
allow_stub = false

[audit]
api_writer = "wal"
compliance_writer = "wal"
wal_dir = "/var/lib/mai/audit"
require_hash_chain = true
require_pqc_checkpoints = true
require_encryption_at_rest = true
allow_memory_writer = false
allow_null_sealer = false

[trust]
anchors_dir = "/etc/mai/trust-anchors"
bundle_cache_dir = "/var/lib/mai/trust"
verifier = "ml-dsa"
allow_accept_all_verifier = false
allow_local_dev_exchange = false
require_trust_anchor = true
require_bundle_on_boot = true

[auth]
auth_keys_path = "/etc/mai/auth_keys.toml"
allow_internal_profile_header = false
require_nonempty_key_store = true

[dashboard]
enabled = true
allow_default_admin_token = false

[network]
bind_address = "127.0.0.1"
tls_mode = "reverse-proxy-required"
require_forwarded_proto_header = false

[observability]
log_format = "json"
log_rotation = true
metrics_exporter = "prometheus"
alerts_enabled = true
"#
    }

    #[test]
    fn baseline_parses_and_validates() {
        let p = parse_ship_profile(baseline()).expect("baseline must validate");
        assert!(p.is_production());
        assert!(p.profile.fail_closed);
        assert!(!p.profile.allow_demo_defaults);
        assert_eq!(p.vault.backend, VaultBackend::Zfs);
        assert_eq!(p.audit.api_writer, AuditWriter::Wal);
        assert_eq!(p.trust.verifier, TrustVerifier::MlDsa);
        assert_eq!(p.network.tls_mode, TlsMode::ReverseProxyRequired);
    }

    #[test]
    fn rejects_allow_demo_defaults_true() {
        let toml = baseline().replace("allow_demo_defaults = false", "allow_demo_defaults = true");
        let err = parse_ship_profile(&toml).expect_err("must reject demo defaults");
        match err {
            ShipProfileError::Validation(msg) => {
                assert!(msg.contains("allow_demo_defaults"), "msg: {msg}")
            }
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_fail_closed_false() {
        let toml = baseline().replace("fail_closed = true", "fail_closed = false");
        let err = parse_ship_profile(&toml).expect_err("must reject fail_closed=false");
        match err {
            ShipProfileError::Validation(msg) => assert!(msg.contains("fail_closed"), "msg: {msg}"),
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_missing_audit_wal_dir() {
        // Replace the configured WAL dir with the empty string. We
        // intentionally do NOT delete the field — that exercises a
        // separate code path (TOML parse failure). The acceptance
        // contract here is "missing audit WAL path", which we model
        // as an empty-string PathBuf.
        let toml = baseline().replace("wal_dir = \"/var/lib/mai/audit\"", "wal_dir = \"\"");
        let err = parse_ship_profile(&toml).expect_err("must reject empty wal_dir");
        match err {
            ShipProfileError::Validation(msg) => {
                assert!(msg.contains("audit.wal_dir"), "msg: {msg}")
            }
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_missing_trust_anchor_dir() {
        let toml = baseline().replace(
            "anchors_dir = \"/etc/mai/trust-anchors\"",
            "anchors_dir = \"\"",
        );
        let err = parse_ship_profile(&toml).expect_err("must reject empty anchors_dir");
        match err {
            ShipProfileError::Validation(msg) => {
                assert!(msg.contains("trust.anchors_dir"), "msg: {msg}")
            }
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_require_trust_anchor_false() {
        let toml = baseline().replace(
            "require_trust_anchor = true",
            "require_trust_anchor = false",
        );
        let err = parse_ship_profile(&toml).expect_err("must reject require_trust_anchor=false");
        match err {
            ShipProfileError::Validation(msg) => {
                assert!(msg.contains("require_trust_anchor"), "msg: {msg}")
            }
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_missing_persistent_paths() {
        // Empty out every required path and confirm at least one fires.
        // We loop so a future field-addition does not silently bypass.
        let cases = [
            ("state_dir = \"/var/lib/mai\"", "state_dir = \"\""),
            ("config_dir = \"/etc/mai\"", "config_dir = \"\""),
            ("log_dir = \"/var/log/mai\"", "log_dir = \"\""),
            ("run_dir = \"/run/mai\"", "run_dir = \"\""),
            ("backup_dir = \"/var/backups/mai\"", "backup_dir = \"\""),
        ];
        for (from, to) in cases {
            let toml = baseline().replace(from, to);
            let err = parse_ship_profile(&toml).expect_err("must reject empty path");
            match err {
                ShipProfileError::Validation(msg) => {
                    assert!(
                        msg.contains("paths."),
                        "expected paths.* error for {from}, got {msg}"
                    )
                }
                other => panic!("expected Validation error, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_stub_vault() {
        let toml = baseline().replace("backend = \"zfs\"", "backend = \"stub\"");
        let err = parse_ship_profile(&toml).expect_err("must reject stub vault");
        match err {
            ShipProfileError::Validation(msg) => assert!(msg.contains("vault"), "msg: {msg}"),
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_memory_audit_writer() {
        let toml = baseline().replace("api_writer = \"wal\"", "api_writer = \"memory\"");
        let err = parse_ship_profile(&toml).expect_err("must reject memory api writer");
        match err {
            ShipProfileError::Validation(msg) => {
                assert!(msg.contains("audit.api_writer"), "msg: {msg}")
            }
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_accept_all_verifier() {
        let toml = baseline().replace("verifier = \"ml-dsa\"", "verifier = \"accept-all\"");
        let err = parse_ship_profile(&toml).expect_err("must reject accept-all verifier");
        match err {
            ShipProfileError::Validation(msg) => {
                assert!(msg.contains("trust.verifier"), "msg: {msg}")
            }
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_wildcard_bind() {
        let toml = baseline().replace("bind_address = \"127.0.0.1\"", "bind_address = \"0.0.0.0\"");
        let err = parse_ship_profile(&toml).expect_err("must reject 0.0.0.0");
        match err {
            ShipProfileError::Validation(msg) => {
                assert!(msg.contains("bind_address"), "msg: {msg}")
            }
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn local_dev_mode_skips_production_invariants() {
        // local-dev is permitted to set allow_demo_defaults = true and
        // still parse. This is the convenience path validate() leaves
        // open for SHIP-02..SHIP-07 to migrate into the runtime guard.
        let toml = baseline()
            .replace("mode = \"production\"", "mode = \"local-dev\"")
            .replace("allow_demo_defaults = false", "allow_demo_defaults = true")
            .replace("fail_closed = true", "fail_closed = false")
            .replace("backend = \"zfs\"", "backend = \"stub\"")
            .replace("api_writer = \"wal\"", "api_writer = \"memory\"")
            .replace(
                "compliance_writer = \"wal\"",
                "compliance_writer = \"memory\"",
            )
            .replace("verifier = \"ml-dsa\"", "verifier = \"accept-all\"");
        let p = parse_ship_profile(&toml).expect("local-dev should accept demo-shaped values");
        assert!(!p.is_production());
        assert!(p.profile.allow_demo_defaults);
    }

    #[test]
    fn rejects_empty_profile_name() {
        let toml = baseline().replace("name = \"ship\"", "name = \"\"");
        let err = parse_ship_profile(&toml).expect_err("must reject empty name");
        match err {
            ShipProfileError::Validation(msg) => {
                assert!(msg.contains("profile.name"), "msg: {msg}")
            }
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn deserializes_openbao_section_with_defaults() {
        let toml = format!(
            "{}\n{}",
            baseline(),
            r#"
[openbao]
address = "http://localhost:8200"
role_id = "8053c291-8f60-381f-e283-5e645e5907f4"
"#
        );
        let p = parse_ship_profile(&toml).expect("profile with openbao must parse");
        let ob = p.openbao.expect("openbao section must be present");
        assert_eq!(ob.address, "http://localhost:8200");
        assert_eq!(ob.role_id, "8053c291-8f60-381f-e283-5e645e5907f4");
        assert_eq!(ob.timeout_secs, 10);
        assert_eq!(ob.transit.claim_signer_key, "lamprey-claim-signer");
        assert_eq!(ob.transit.bundle_signer_key, "lamprey-bundle-signer");
        assert_eq!(
            ob.transit.revocation_signer_key,
            "lamprey-revocation-signer"
        );
        assert_eq!(ob.kv.tenant_path, "kv/tenants");
        assert_eq!(ob.kv.revocation_path, "kv/revocations");
        assert_eq!(ob.pki.role, "mai-appliance");
        assert!(ob.trust_refresh.enabled);
        assert_eq!(ob.trust_refresh.interval_secs, 300);
    }

    #[test]
    fn deserializes_openbao_section_with_custom_values() {
        let toml = format!(
            "{}\n{}",
            baseline(),
            r#"
[openbao]
address = "https://bao.example.com:8200"
role_id = "custom-role"
timeout_secs = 30

[openbao.transit]
claim_signer_key = "custom-claim-key"
bundle_signer_key = "custom-bundle-key"
revocation_signer_key = "custom-revocation-key"

[openbao.kv]
tenant_path = "secret/tenants"
revocation_path = "secret/revocations"

[openbao.pki]
role = "custom-role"

[openbao.trust_refresh]
enabled = false
interval_secs = 600
"#
        );
        let p = parse_ship_profile(&toml).expect("profile with custom openbao must parse");
        let ob = p.openbao.expect("openbao section must be present");
        assert_eq!(ob.address, "https://bao.example.com:8200");
        assert_eq!(ob.role_id, "custom-role");
        assert_eq!(ob.timeout_secs, 30);
        assert_eq!(ob.transit.claim_signer_key, "custom-claim-key");
        assert_eq!(ob.transit.bundle_signer_key, "custom-bundle-key");
        assert_eq!(ob.transit.revocation_signer_key, "custom-revocation-key");
        assert_eq!(ob.kv.tenant_path, "secret/tenants");
        assert_eq!(ob.kv.revocation_path, "secret/revocations");
        assert_eq!(ob.pki.role, "custom-role");
        assert!(!ob.trust_refresh.enabled);
        assert_eq!(ob.trust_refresh.interval_secs, 600);
    }

    #[test]
    fn openbao_section_absent_is_none() {
        let p = parse_ship_profile(baseline()).expect("baseline must validate");
        assert!(
            p.openbao.is_none(),
            "openbao is optional and must default to None"
        );
    }
}
