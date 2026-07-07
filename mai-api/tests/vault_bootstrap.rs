//! SHIP-03 acceptance tests for `mai_api::vault_builder::build_vault`.
//!
//! Covers the behavior matrix declared in `vault_builder.rs`. Each
//! case constructs a `ShipProfile` programmatically (bypassing the
//! parse-time validator in `ship_profile`) and asserts the builder's
//! decision matches the doc table.

use std::path::PathBuf;

use mai_api::ship_profile::{
    AuditConfig, AuditWriter, AuthConfig, DashboardConfig, LogFormat, MetricsExporter,
    NetworkConfig, ObservabilityConfig, PathsConfig, ProfileMeta, ProfileMode, ShipProfile,
    TlsMode, TrustConfig, TrustVerifier, VaultBackend, VaultConfig,
};
use mai_api::vault_builder::{VaultBuildError, build_vault};

/// Baseline profile shared by every test. Production mode tightens
/// defaults to mirror a real ship profile; local-dev loosens them so
/// the parse-time invariants do not interfere with builder tests.
fn baseline(mode: ProfileMode, root: PathBuf) -> ShipProfile {
    let is_prod = matches!(mode, ProfileMode::Production);
    ShipProfile {
        profile: ProfileMeta {
            name: "test".into(),
            mode,
            allow_demo_defaults: !is_prod,
            fail_closed: is_prod,
        },
        paths: PathsConfig {
            state_dir: PathBuf::from("/var/lib/mai"),
            config_dir: PathBuf::from("/etc/mai"),
            log_dir: PathBuf::from("/var/log/mai"),
            run_dir: PathBuf::from("/run/mai"),
            backup_dir: PathBuf::from("/var/backups/mai"),
        },
        vault: VaultConfig {
            backend: VaultBackend::Zfs,
            root,
            require_sealed_master_key: false,
            require_pqc: false,
            allow_stub: false,
        },
        audit: AuditConfig {
            api_writer: AuditWriter::Wal,
            compliance_writer: AuditWriter::Wal,
            wal_dir: PathBuf::from("/var/lib/mai/audit"),
            require_hash_chain: true,
            require_pqc_checkpoints: false,
            require_encryption_at_rest: false,
            allow_memory_writer: false,
            allow_null_sealer: false,
        },
        trust: TrustConfig {
            anchors_dir: PathBuf::from("/etc/mai/trust-anchors"),
            bundle_cache_dir: PathBuf::from("/var/lib/mai/trust"),
            verifier: TrustVerifier::MlDsa,
            allow_accept_all_verifier: false,
            allow_local_dev_exchange: false,
            require_trust_anchor: true,
            require_bundle_on_boot: true,
        },
        auth: AuthConfig {
            auth_keys_path: PathBuf::from("/etc/mai/auth_keys.toml"),
            allow_internal_profile_header: false,
            require_nonempty_key_store: true,
        },
        dashboard: DashboardConfig {
            enabled: true,
            allow_default_admin_token: false,
        },
        network: NetworkConfig {
            bind_address: "127.0.0.1".into(),
            tls_mode: TlsMode::ReverseProxyRequired,
            require_forwarded_proto_header: false,
        },
        observability: ObservabilityConfig {
            log_format: LogFormat::Json,
            log_rotation: true,
            metrics_exporter: MetricsExporter::Prometheus,
            alerts_enabled: false,
        },
        openbao: None,
    }
}

#[test]
fn production_rejects_stub_backend() {
    let temp = tempfile::tempdir().unwrap();
    let mut p = baseline(ProfileMode::Production, temp.path().to_path_buf());
    p.vault.backend = VaultBackend::Stub;
    let err = build_vault(&p)
        .map(|_| ())
        .expect_err("production must refuse stub backend");
    assert!(
        matches!(err, VaultBuildError::StubInProduction { .. }),
        "got {err:?}"
    );
}

#[test]
fn production_rejects_file_dev_backend() {
    // AF-005: file-dev is a plaintext-capable dev backend; production must refuse
    // it even when the root exists.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("vault");
    std::fs::create_dir_all(&root).unwrap();
    let mut p = baseline(ProfileMode::Production, root);
    p.vault.backend = VaultBackend::FileDev;
    let err = build_vault(&p)
        .map(|_| ())
        .expect_err("production must refuse the plaintext-capable file-dev backend");
    assert!(
        matches!(err, VaultBuildError::FileDevInProduction),
        "got {err:?}"
    );
}

#[test]
fn production_rejects_allow_stub_true() {
    let temp = tempfile::tempdir().unwrap();
    let mut p = baseline(ProfileMode::Production, temp.path().to_path_buf());
    p.vault.allow_stub = true;
    let err = build_vault(&p)
        .map(|_| ())
        .expect_err("production must refuse allow_stub=true");
    assert!(
        matches!(err, VaultBuildError::StubAllowedInProduction),
        "got {err:?}"
    );
}

#[test]
fn production_accepts_zfs_when_root_exists() {
    let temp = tempfile::tempdir().unwrap();
    let p = baseline(ProfileMode::Production, temp.path().to_path_buf());
    let _vault = build_vault(&p).expect("production zfs build must succeed");
}

#[test]
fn production_rejects_missing_root() {
    // A path that cannot exist on either Linux or Windows test hosts.
    let p = baseline(
        ProfileMode::Production,
        PathBuf::from("/__ship_03_nonexistent_vault_root__/zzz"),
    );
    let err = build_vault(&p)
        .map(|_| ())
        .expect_err("missing root must fail in production");
    assert!(
        matches!(err, VaultBuildError::RootMissing { .. }),
        "got {err:?}"
    );
}

#[test]
fn production_rejects_empty_root() {
    let p = baseline(ProfileMode::Production, PathBuf::new());
    let err = build_vault(&p)
        .map(|_| ())
        .expect_err("empty root must fail");
    assert!(matches!(err, VaultBuildError::EmptyRoot), "got {err:?}");
}

#[test]
fn local_dev_allows_stub_when_flag_set() {
    let temp = tempfile::tempdir().unwrap();
    let mut p = baseline(ProfileMode::LocalDev, temp.path().to_path_buf());
    p.vault.backend = VaultBackend::Stub;
    p.vault.allow_stub = true;
    let _vault = build_vault(&p).expect("local-dev stub vault must build when allow_stub=true");
}

#[test]
fn local_dev_rejects_stub_when_flag_clear() {
    let temp = tempfile::tempdir().unwrap();
    let mut p = baseline(ProfileMode::LocalDev, temp.path().to_path_buf());
    p.vault.backend = VaultBackend::Stub;
    p.vault.allow_stub = false;
    let err = build_vault(&p)
        .map(|_| ())
        .expect_err("stub backend without allow_stub must fail");
    assert!(
        matches!(err, VaultBuildError::StubNotAllowed),
        "got {err:?}"
    );
}

#[tokio::test]
async fn file_dev_backend_is_accepted() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("vault");
    std::fs::create_dir_all(&root).unwrap();
    let mut p = baseline(ProfileMode::LocalDev, root);
    p.vault.backend = VaultBackend::FileDev;
    let vault = build_vault(&p).expect("file-dev must be accepted in local-dev mode");
    assert!(
        vault.load_model_weights("any").await.is_err(),
        "stub-like: no models on disk"
    );
}

#[tokio::test]
async fn local_dev_stub_responds_as_vault_interface() {
    let temp = tempfile::tempdir().unwrap();
    let mut p = baseline(ProfileMode::LocalDev, temp.path().to_path_buf());
    p.vault.backend = VaultBackend::Stub;
    p.vault.allow_stub = true;
    let vault = build_vault(&p).expect("build must succeed");
    let res = vault.load_model_weights("does-not-exist").await;
    assert!(res.is_err(), "stub must report model-not-found");
}
