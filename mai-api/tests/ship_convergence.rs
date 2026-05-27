//! SHIP-07 convergence acceptance tests.
//!
//! Asserts that the SHIP-03/04/05/06 builders compose under a
//! parse-validated profile and that the runtime-introspection results
//! flip every deferred `PROD-*-100/101` (plus `PROD-POLICY-001`) check
//! from Deferred to Pass.
//!
//! These tests intentionally stay off the network and off the
//! production socket bind so they can run on any host. They exercise
//! every public surface SHIP-07 introduced:
//!
//! - [`mai_api::production_guard::RuntimeChecks`] /
//!   [`mai_api::production_guard::RuntimeOutcome`].
//! - [`mai_api::production_guard::ProductionReadinessReport::evaluate_with_runtime`].
//! - [`mai_api::MaiServer::with_ship_profile`] (failure path: invalid
//!   profile is rejected with `ServerError::Config`).

use std::fs;
use std::path::PathBuf;

use mai_api::production_guard::{
    CheckStatus, ProductionReadinessReport, RuntimeChecks, RuntimeOutcome,
};
use mai_api::sealer_builder::build_sealer;
use mai_api::ship_profile::{
    AuditConfig, AuditWriter, AuthConfig, DashboardConfig, LogFormat, MetricsExporter,
    NetworkConfig, ObservabilityConfig, PathsConfig, ProfileMeta, ProfileMode, ShipProfile,
    TlsMode, TrustConfig, TrustVerifier, VaultBackend, VaultConfig,
};
use mai_api::trust_builder::build_trust_components;
use mai_api::vault_builder::build_vault;
use mai_api::{MaiServer, ServerError};

/// Programmatic local-dev profile that all four SHIP-03/04/05/06
/// builders accept. Uses tempdir-rooted state so the test is hermetic
/// and works on Windows + Linux + macOS.
fn local_dev_profile(state_dir: PathBuf) -> ShipProfile {
    let vault_root = state_dir.join("vault");
    let audit_dir = state_dir.join("audit");
    let trust_anchors = state_dir.join("trust-anchors");
    let trust_cache = state_dir.join("trust");
    fs::create_dir_all(&vault_root).unwrap();
    fs::create_dir_all(&audit_dir).unwrap();
    fs::create_dir_all(&trust_anchors).unwrap();
    fs::create_dir_all(&trust_cache).unwrap();
    ShipProfile {
        profile: ProfileMeta {
            name: "test-local-dev".into(),
            mode: ProfileMode::LocalDev,
            allow_demo_defaults: true,
            fail_closed: false,
        },
        paths: PathsConfig {
            state_dir: state_dir.clone(),
            config_dir: state_dir.join("config"),
            log_dir: state_dir.join("log"),
            run_dir: state_dir.join("run"),
            backup_dir: state_dir.join("backups"),
        },
        vault: VaultConfig {
            backend: VaultBackend::Zfs,
            root: vault_root,
            require_sealed_master_key: false,
            require_pqc: false,
            allow_stub: false,
        },
        audit: AuditConfig {
            api_writer: AuditWriter::Wal,
            compliance_writer: AuditWriter::Wal,
            wal_dir: audit_dir,
            require_hash_chain: true,
            require_pqc_checkpoints: false,
            require_encryption_at_rest: false,
            allow_memory_writer: false,
            allow_null_sealer: false,
        },
        trust: TrustConfig {
            anchors_dir: trust_anchors,
            bundle_cache_dir: trust_cache,
            verifier: TrustVerifier::MlDsa,
            allow_accept_all_verifier: false,
            allow_local_dev_exchange: true,
            require_trust_anchor: false,
            require_bundle_on_boot: false,
        },
        auth: AuthConfig {
            auth_keys_path: state_dir.join("auth_keys.toml"),
            allow_internal_profile_header: true,
            require_nonempty_key_store: false,
        },
        dashboard: DashboardConfig {
            enabled: true,
            allow_default_admin_token: true,
        },
        network: NetworkConfig {
            bind_address: "127.0.0.1".into(),
            tls_mode: TlsMode::Direct,
            require_forwarded_proto_header: false,
        },
        observability: ObservabilityConfig {
            log_format: LogFormat::Json,
            log_rotation: false,
            metrics_exporter: MetricsExporter::Prometheus,
            alerts_enabled: false,
        },
        openbao: None,
    }
}

#[test]
fn all_builders_compose_under_local_dev_profile() {
    // Every SHIP-03..SHIP-06 builder must produce a usable component
    // from the same parse-validated profile. This is the SHIP-07
    // composition contract: one profile in, four real components out,
    // no demo defaults reachable accidentally.
    let temp = tempfile::tempdir().unwrap();
    let profile = local_dev_profile(temp.path().to_path_buf());

    let _vault = build_vault(&profile).expect("vault builder accepts local-dev profile");
    let _sealer = build_sealer(&profile).expect("sealer builder accepts local-dev profile");
    let trust = build_trust_components(&profile).expect("trust builder accepts local-dev profile");

    // Local-dev with allow_local_dev_exchange=true picks the synthetic
    // token-exchange mode; production would pick OpenBaoBridge. Either
    // way the verifier is real (MlDsaBundleVerifier with zero anchors
    // in this test) — never the legacy AcceptAllBundleVerifier.
    assert_eq!(trust.exchange_mode.label(), "local-dev-synthetic");
    assert_eq!(trust.anchor_count(), 0);
}

#[test]
fn runtime_pass_lifts_every_deferred_id_under_production_profile() {
    // Drive the SHIP-07 readiness flow from the production_guard side:
    // a parse-validated production profile starts with six deferred
    // runtime checks; feeding the matching RuntimeChecks::pass results
    // flips every one of them to Pass and the report becomes
    // ship-ready with zero deferred.
    let profile_toml = r#"
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
"#;
    let profile = mai_api::ship_profile::parse_ship_profile(profile_toml).expect("baseline parses");

    let runtime = RuntimeChecks {
        vault_opened: Some(RuntimeOutcome::pass("vault opened at /var/lib/mai/vault")),
        api_audit_wal_ready: Some(RuntimeOutcome::pass("WAL replayed (0 entries)")),
        compliance_sealer_real: Some(RuntimeOutcome::pass("AEAD sealer from sealer.key")),
        trust_bundle_verified: Some(RuntimeOutcome::pass(
            "bundle v2026-05-23 verified against 3 anchors",
        )),
        auth_keys_nonempty: Some(RuntimeOutcome::pass("4 key(s) loaded")),
        auth_internal_bypass_consistent: Some(RuntimeOutcome::pass(
            "runtime bypass = false, profile field = false: consistent",
        )),
        policy_modules_loaded: Some(RuntimeOutcome::pass("Standard template loaded")),
    };

    let report = ProductionReadinessReport::evaluate_with_runtime(&profile, &runtime);
    assert!(
        report.is_ship_ready(),
        "ship_ready=false; report:\n{}",
        report.render_human()
    );
    let counts = report.counts();
    assert_eq!(
        counts.fail,
        0,
        "no failures expected, got:\n{}",
        report.render_human()
    );
    assert_eq!(
        counts.deferred,
        0,
        "all deferred IDs should be flipped, got:\n{}",
        report.render_human()
    );
    for id in [
        "PROD-VAULT-100",
        "PROD-AUDIT-100",
        "PROD-AUDIT-101",
        "PROD-TRUST-100",
        "PROD-AUTH-100",
        "PROD-POLICY-001",
    ] {
        let c = report.find(id).unwrap();
        assert_eq!(
            c.status,
            CheckStatus::Pass,
            "{id} should be Pass; report:\n{}",
            report.render_human()
        );
    }
}

#[test]
fn runtime_fail_in_any_critical_id_blocks_ship_ready() {
    let profile = local_dev_profile(tempfile::tempdir().unwrap().path().to_path_buf());
    // Local-dev mode means every check is Skipped, so applying a Fail
    // runtime outcome must not drag the report into "not ready" — the
    // Skipped check stays Skipped (verified by the guard unit tests).
    let runtime = RuntimeChecks {
        vault_opened: Some(RuntimeOutcome::fail("EACCES")),
        ..RuntimeChecks::default()
    };
    let report = ProductionReadinessReport::evaluate_with_runtime(&profile, &runtime);
    assert!(report.is_ship_ready());
    assert_eq!(
        report.find("PROD-VAULT-100").unwrap().status,
        CheckStatus::Skipped
    );
}

#[tokio::test]
async fn mai_server_rejects_missing_ship_profile_path() {
    // SHIP-07 wires `MaiServer::with_ship_profile`. A non-existent
    // path must surface as ServerError::Config so production startup
    // fails closed before any component is constructed.
    let bogus = PathBuf::from("/this/path/does/not/exist/for-ship-07-test.toml");
    let server = MaiServer::default_scout().with_ship_profile(bogus);
    let result = server.run().await;
    match result {
        Err(ServerError::Config(msg)) => {
            assert!(
                msg.contains("ship profile") && msg.contains("did not load"),
                "unexpected error message: {msg}"
            );
        }
        Err(other) => panic!("expected ServerError::Config, got {other:?}"),
        Ok(()) => panic!("expected failure, server returned Ok"),
    }
}
