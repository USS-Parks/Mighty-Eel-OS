//! Integration test for the ship profile.
//!
//! Confirms that the canonical `deployment/ship/profile.toml` checked
//! into the repo agrees with the Rust schema in
//! `mai_api::ship_profile`. If this test breaks, either the TOML
//! drifted from the contract or the Rust types did — both are
//! acceptance-test failures.
//!
//! Per `SHIP-HARDENING-PLAN.md` §3 the "Done When" line says
//! `cargo test -p mai-api ship_profile` must pass. The unit tests in
//! `mai-api/src/ship_profile.rs` cover the negative cases against an
//! inline baseline; this file covers the positive case against the
//! on-disk artifact.

use std::path::PathBuf;

use mai_api::ship_profile::{
    AuditWriter, LogFormat, MetricsExporter, ProfileMode, TlsMode, TrustVerifier, VaultBackend,
    load_ship_profile, parse_ship_profile,
};

/// Resolve the workspace-relative `deployment/ship/profile.toml` path.
/// `CARGO_MANIFEST_DIR` points at `<repo>/mai/mai-api`; the deployment
/// directory is one level up under `<repo>/mai/deployment/ship/`.
fn ship_profile_path() -> PathBuf {
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by cargo");
    PathBuf::from(manifest_dir)
        .parent()
        .expect("mai-api manifest dir must have a parent")
        .join("deployment")
        .join("ship")
        .join("profile.toml")
}

#[test]
fn deployment_ship_profile_parses_and_validates() {
    let path = ship_profile_path();
    let profile = load_ship_profile(&path)
        .unwrap_or_else(|e| panic!("loading {} must succeed: {e}", path.display()));

    // Profile meta contract.
    assert_eq!(profile.profile.name, "ship");
    assert_eq!(profile.profile.mode, ProfileMode::Production);
    assert!(profile.profile.fail_closed);
    assert!(!profile.profile.allow_demo_defaults);
    assert!(profile.is_production());

    // Paths: every required directory is populated.
    assert!(!profile.paths.state_dir.as_os_str().is_empty());
    assert!(!profile.paths.config_dir.as_os_str().is_empty());
    assert!(!profile.paths.log_dir.as_os_str().is_empty());
    assert!(!profile.paths.run_dir.as_os_str().is_empty());
    assert!(!profile.paths.backup_dir.as_os_str().is_empty());

    // Vault: real backend, sealed master key, PQC required, stub forbidden.
    assert_eq!(profile.vault.backend, VaultBackend::Zfs);
    assert!(profile.vault.require_sealed_master_key);
    assert!(profile.vault.require_pqc);
    assert!(!profile.vault.allow_stub);

    // Audit: persistent WAL on both paths, hash chain + PQC + AEAD required.
    assert_eq!(profile.audit.api_writer, AuditWriter::Wal);
    assert_eq!(profile.audit.compliance_writer, AuditWriter::Wal);
    assert!(profile.audit.require_hash_chain);
    assert!(profile.audit.require_pqc_checkpoints);
    assert!(profile.audit.require_encryption_at_rest);
    assert!(!profile.audit.allow_memory_writer);
    assert!(!profile.audit.allow_null_sealer);

    // Trust: ML-DSA verifier, anchor present, bundle-on-boot, no synthetic exchange.
    assert_eq!(profile.trust.verifier, TrustVerifier::MlDsa);
    assert!(!profile.trust.allow_accept_all_verifier);
    assert!(!profile.trust.allow_local_dev_exchange);
    assert!(profile.trust.require_trust_anchor);
    assert!(profile.trust.require_bundle_on_boot);
    assert!(!profile.trust.anchors_dir.as_os_str().is_empty());
    assert!(!profile.trust.bundle_cache_dir.as_os_str().is_empty());

    // Auth: non-empty key store, no internal-profile-header bypass.
    assert!(!profile.auth.allow_internal_profile_header);
    assert!(profile.auth.require_nonempty_key_store);
    assert!(!profile.auth.auth_keys_path.as_os_str().is_empty());

    // Dashboard: enabled, default admin token forbidden.
    assert!(profile.dashboard.enabled);
    assert!(!profile.dashboard.allow_default_admin_token);

    // Network: loopback bind, reverse-proxy TLS.
    assert_eq!(profile.network.bind_address, "127.0.0.1");
    assert_eq!(profile.network.tls_mode, TlsMode::ReverseProxyRequired);

    // Observability: JSON + Prometheus + alerts on.
    assert_eq!(profile.observability.log_format, LogFormat::Json);
    assert_eq!(
        profile.observability.metrics_exporter,
        MetricsExporter::Prometheus
    );
    assert!(profile.observability.log_rotation);
    assert!(profile.observability.alerts_enabled);
}

#[test]
fn ship_profile_production_example_template_parses_and_validates() {
    // The annotated operator template under config/ must also satisfy
    // the production contract. If a comment drift accidentally toggles
    // a field this test catches it.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir");
    let path = PathBuf::from(manifest_dir)
        .parent()
        .expect("manifest dir parent")
        .join("config")
        .join("production.example.toml");
    let profile = load_ship_profile(&path)
        .unwrap_or_else(|e| panic!("loading {} must succeed: {e}", path.display()));
    assert!(profile.is_production());
    assert!(profile.profile.fail_closed);
}

#[test]
fn parse_ship_profile_is_pure_string_path() {
    // Smoke: the in-memory parser exposed to the validator CLI
    // accepts the same content that load_ship_profile reads from disk.
    let path = ship_profile_path();
    let content = std::fs::read_to_string(&path).expect("read profile.toml");
    let parsed = parse_ship_profile(&content).expect("parse profile.toml");
    assert!(parsed.is_production());
}
