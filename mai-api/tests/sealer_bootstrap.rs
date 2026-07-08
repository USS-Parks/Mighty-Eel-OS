//! SHIP-05 acceptance tests for `mai_api::sealer_builder::build_sealer`.
//!
//! Covers the behavior matrix in `sealer_builder.rs`. Each test
//! constructs a `ShipProfile` programmatically (bypassing the
//! parse-time validator) and asserts the builder's decision and the
//! resulting sealer's observable behavior.

use std::fs;
use std::path::PathBuf;

use mai_api::sealer_builder::{SealerBuildError, build_sealer, sealer_key_path};
use mai_api::ship_profile::{
    AuditConfig, AuditWriter, AuthConfig, DashboardConfig, LogFormat, MetricsExporter,
    NetworkConfig, ObservabilityConfig, PathsConfig, ProfileMeta, ProfileMode, ShipProfile,
    TlsMode, TrustConfig, TrustVerifier, VaultBackend, VaultConfig,
};

const AEAD_KEY_LEN: usize = 32;
const AEAD_NONCE_LEN: usize = 12;

/// Baseline profile with the supplied mode and `audit.wal_dir`.
fn baseline(mode: ProfileMode, wal_dir: PathBuf) -> ShipProfile {
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
            root: PathBuf::from("/var/lib/mai/vault"),
            dataset: None,
            require_sealed_master_key: false,
            require_pqc: false,
            allow_stub: false,
        },
        audit: AuditConfig {
            api_writer: AuditWriter::Wal,
            compliance_writer: AuditWriter::Wal,
            wal_dir,
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
fn production_rejects_allow_null_sealer_true() {
    let temp = tempfile::tempdir().unwrap();
    let mut p = baseline(ProfileMode::Production, temp.path().to_path_buf());
    p.audit.allow_null_sealer = true;
    let err = build_sealer(&p)
        .map(|_| ())
        .expect_err("production must refuse allow_null_sealer=true");
    assert!(
        matches!(err, SealerBuildError::NullSealerAllowedInProduction),
        "got {err:?}"
    );
}

#[test]
fn production_requires_key_file() {
    let temp = tempfile::tempdir().unwrap();
    // Note: wal_dir exists but no sealer.key inside it.
    let p = baseline(ProfileMode::Production, temp.path().to_path_buf());
    let err = build_sealer(&p)
        .map(|_| ())
        .expect_err("missing key file must fail in production");
    assert!(
        matches!(err, SealerBuildError::KeyFileMissing { .. }),
        "got {err:?}"
    );
}

#[test]
fn production_rejects_wrong_length_key_file() {
    let temp = tempfile::tempdir().unwrap();
    let p = baseline(ProfileMode::Production, temp.path().to_path_buf());
    fs::write(sealer_key_path(&p), [0u8; 16]).unwrap();
    let err = build_sealer(&p)
        .map(|_| ())
        .expect_err("wrong-length key must fail");
    assert!(
        matches!(
            err,
            SealerBuildError::KeyFileLengthInvalid {
                expected: 32,
                actual: 16,
                ..
            }
        ),
        "got {err:?}"
    );
}

#[test]
fn production_builds_aead_when_key_file_present() {
    let temp = tempfile::tempdir().unwrap();
    let p = baseline(ProfileMode::Production, temp.path().to_path_buf());
    fs::write(sealer_key_path(&p), [0x42u8; AEAD_KEY_LEN]).unwrap();
    let sealer = build_sealer(&p).expect("production AeadSealer must build");

    // Behavior probe: AeadSealer prepends a 12-byte nonce and adds a
    // 16-byte GCM tag, so the sealed output is strictly longer than
    // plaintext + nonce.
    let plaintext = b"audit-entry";
    let ciphertext = sealer.seal(plaintext);
    assert!(
        ciphertext.len() > plaintext.len() + AEAD_NONCE_LEN,
        "AeadSealer output must include nonce + tag overhead; got {} bytes",
        ciphertext.len()
    );
    assert_ne!(
        &ciphertext[AEAD_NONCE_LEN..],
        plaintext,
        "ciphertext must differ from plaintext"
    );
}

#[test]
fn local_dev_allows_null_sealer_when_flag_set() {
    let temp = tempfile::tempdir().unwrap();
    let mut p = baseline(ProfileMode::LocalDev, temp.path().to_path_buf());
    p.audit.allow_null_sealer = true;
    let sealer = build_sealer(&p).expect("local-dev NullSealer must build");

    // Behavior probe: NullSealer is identity — output equals plaintext.
    let plaintext = b"dev-entry";
    assert_eq!(sealer.seal(plaintext), plaintext);
}

#[test]
fn local_dev_uses_ephemeral_aead_by_default() {
    let temp = tempfile::tempdir().unwrap();
    let p = baseline(ProfileMode::LocalDev, temp.path().to_path_buf());
    let sealer = build_sealer(&p).expect("local-dev default AeadSealer must build");

    let plaintext = b"dev-entry";
    let ciphertext = sealer.seal(plaintext);
    assert!(
        ciphertext.len() > plaintext.len() + AEAD_NONCE_LEN,
        "default local-dev sealer must be AEAD, not Null"
    );
}

#[test]
fn aead_sealer_produces_distinct_nonces() {
    let temp = tempfile::tempdir().unwrap();
    let p = baseline(ProfileMode::Production, temp.path().to_path_buf());
    fs::write(sealer_key_path(&p), [0x11u8; AEAD_KEY_LEN]).unwrap();
    let sealer = build_sealer(&p).expect("build must succeed");

    let pt = b"same input";
    let a = sealer.seal(pt);
    let b = sealer.seal(pt);
    assert_ne!(a, b, "AeadSealer must use fresh nonce per seal");
    assert_ne!(&a[..AEAD_NONCE_LEN], &b[..AEAD_NONCE_LEN]);
}
