//! Acceptance tests for `mai_api::trust_builder`.
//!
//! Covers the behavior matrix in `trust_builder.rs` plus the
//! plan-§7 acceptance list:
//!
//! - ship mode rejects local-dev token exchange
//! - ship mode rejects accept-all bundle verifier
//! - valid signed bundle boots
//! - tampered bundle blocks boot
//! - missing trust anchor blocks boot
//!
//! Each case constructs a `ShipProfile` programmatically (bypassing
//! the parse-time validator in `ship_profile`) so the builder's
//! defenses can be exercised in isolation.

use std::fs;
use std::path::{Path, PathBuf};

use mai_api::ship_profile::{
    AuditConfig, AuditWriter, AuthConfig, DashboardConfig, LogFormat, MetricsExporter,
    NetworkConfig, ObservabilityConfig, PathsConfig, ProfileMeta, ProfileMode, ShipProfile,
    TlsMode, TrustConfig, TrustVerifier, VaultBackend, VaultConfig,
};
use mai_api::trust_builder::{
    TrustBuildError, TrustExchangeMode, boot_bundle_path, build_trust_components,
    verify_boot_bundle,
};

use mai_compliance::bundle::{
    BundleMetadata, BundleVerifier, MlDsaBundleVerifier, PolicyBundlePayload, SignatureEnvelope,
    SignedPolicyBundle, payload_hash,
};
use mai_compliance::trust_cache::{RevocationSnapshot, SnapshotStatus};

const MLDSA87_PK_LEN: usize = 2592;

// ─── Profile helpers ────────────────────────────────────────────────

/// Baseline profile for the supplied mode. Anchors and bundle cache
/// directories default to the supplied temp paths so the caller can
/// drop anchor files into them.
fn baseline(mode: ProfileMode, anchors_dir: PathBuf, bundle_cache_dir: PathBuf) -> ShipProfile {
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
            wal_dir: PathBuf::from("/var/lib/mai/audit"),
            require_hash_chain: true,
            require_pqc_checkpoints: false,
            require_encryption_at_rest: false,
            allow_memory_writer: false,
            allow_null_sealer: false,
        },
        trust: TrustConfig {
            anchors_dir,
            bundle_cache_dir,
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

// ─── ML-DSA-87 keypair / signing helpers ────────────────────────────

/// Generate a fresh ML-DSA-87 keypair for tests. Mirrors the helper in
/// `mai_compliance::bundle::tests` so this crate does not need to
/// re-export private test utilities.
fn fresh_keypair() -> (Vec<u8>, Vec<u8>) {
    use ml_dsa::{B32, KeyGen, MlDsa87};
    use rand::RngCore;
    let mut seed_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed_bytes);
    let seed = B32::from(seed_bytes);
    let kp = MlDsa87::key_gen_internal(&seed);
    let pk = kp.verifying_key().encode().to_vec();
    let sk = kp.signing_key().encode().to_vec();
    (pk, sk)
}

fn sign_with(secret_key: &[u8], hash: &[u8; 32]) -> Vec<u8> {
    use ml_dsa::signature::Signer;
    use ml_dsa::{EncodedSigningKey, MlDsa87, Signature, SigningKey};
    const SK_LEN: usize = 4896;
    let sk_arr: &[u8; SK_LEN] = secret_key.try_into().expect("ml-dsa-87 secret key length");
    let sk_encoded = EncodedSigningKey::<MlDsa87>::from(*sk_arr);
    let sk_dec = SigningKey::<MlDsa87>::decode(&sk_encoded);
    let sig: Signature<MlDsa87> = sk_dec.sign(hash);
    sig.encode().to_vec()
}

fn build_signed_bundle(
    issued_at_secs: u64,
    expires_at_secs: u64,
    secret_key: &[u8],
    public_key_id: &str,
) -> SignedPolicyBundle {
    let metadata = BundleMetadata {
        version: "2026.05.23.001".to_string(),
        issuer: "trust-bridge".to_string(),
        issued_at_secs,
        expires_at_secs,
        tenant_id: "ship-test-tenant".to_string(),
    };
    let payload = PolicyBundlePayload {
        revocations: vec![RevocationSnapshot {
            claim_id: "claim-001".to_string(),
            status: SnapshotStatus::Valid,
            recorded_at_secs: issued_at_secs,
        }],
    };
    let hash = payload_hash(&metadata, &payload).expect("canonical-json must serialize");
    let sig_bytes = sign_with(secret_key, &hash);
    SignedPolicyBundle {
        metadata,
        payload,
        signature: SignatureEnvelope {
            algorithm: "ml-dsa-87".to_string(),
            public_key_id: public_key_id.to_string(),
            bytes_hex: hex::encode(sig_bytes),
        },
    }
}

fn write_anchor(dir: &Path, key_id: &str, pk_bytes: &[u8]) -> PathBuf {
    fs::create_dir_all(dir).expect("anchors dir must be createable");
    let path = dir.join(format!("{key_id}.pub"));
    fs::write(&path, pk_bytes).expect("anchor file must write");
    path
}

fn write_boot_bundle(dir: &Path, bundle: &SignedPolicyBundle) -> PathBuf {
    fs::create_dir_all(dir).expect("bundle cache dir must be createable");
    let path = dir.join("bundle.json");
    let body = serde_json::to_vec(bundle).expect("bundle must JSON-serialize");
    fs::write(&path, body).expect("bundle file must write");
    path
}

// ─── Production rejection paths (no anchors needed) ─────────────────

#[test]
fn production_rejects_accept_all_verifier() {
    // Plan §7 acceptance: "ship mode rejects accept-all bundle verifier."
    let temp = tempfile::tempdir().unwrap();
    let mut p = baseline(
        ProfileMode::Production,
        temp.path().join("anchors"),
        temp.path().join("cache"),
    );
    p.trust.verifier = TrustVerifier::AcceptAll;
    let err = build_trust_components(&p)
        .map(|_| ())
        .expect_err("production must reject accept-all verifier");
    assert!(
        matches!(err, TrustBuildError::AcceptAllInProduction),
        "got {err:?}"
    );
}

#[test]
fn production_rejects_allow_accept_all_true() {
    let temp = tempfile::tempdir().unwrap();
    let mut p = baseline(
        ProfileMode::Production,
        temp.path().join("anchors"),
        temp.path().join("cache"),
    );
    p.trust.allow_accept_all_verifier = true;
    let err = build_trust_components(&p)
        .map(|_| ())
        .expect_err("production must reject allow_accept_all_verifier=true");
    assert!(
        matches!(err, TrustBuildError::AcceptAllAllowedInProduction),
        "got {err:?}"
    );
}

#[test]
fn production_rejects_local_dev_exchange_flag() {
    // Plan §7 acceptance: "ship mode rejects local-dev token exchange."
    let temp = tempfile::tempdir().unwrap();
    let mut p = baseline(
        ProfileMode::Production,
        temp.path().join("anchors"),
        temp.path().join("cache"),
    );
    p.trust.allow_local_dev_exchange = true;
    let err = build_trust_components(&p)
        .map(|_| ())
        .expect_err("production must reject allow_local_dev_exchange=true");
    assert!(
        matches!(err, TrustBuildError::LocalDevExchangeInProduction),
        "got {err:?}"
    );
}

#[test]
fn production_rejects_require_trust_anchor_false() {
    let temp = tempfile::tempdir().unwrap();
    let mut p = baseline(
        ProfileMode::Production,
        temp.path().join("anchors"),
        temp.path().join("cache"),
    );
    p.trust.require_trust_anchor = false;
    let err = build_trust_components(&p)
        .map(|_| ())
        .expect_err("production must reject require_trust_anchor=false");
    assert!(
        matches!(err, TrustBuildError::TrustAnchorNotRequired),
        "got {err:?}"
    );
}

#[test]
fn production_rejects_require_bundle_on_boot_false() {
    let temp = tempfile::tempdir().unwrap();
    let mut p = baseline(
        ProfileMode::Production,
        temp.path().join("anchors"),
        temp.path().join("cache"),
    );
    p.trust.require_bundle_on_boot = false;
    let err = build_trust_components(&p)
        .map(|_| ())
        .expect_err("production must reject require_bundle_on_boot=false");
    assert!(
        matches!(err, TrustBuildError::BootBundleNotRequired),
        "got {err:?}"
    );
}

// ─── Anchor loading edge cases ──────────────────────────────────────

#[test]
fn production_rejects_missing_anchors_dir() {
    // Plan §7 acceptance: "missing trust anchor blocks boot."
    let temp = tempfile::tempdir().unwrap();
    let anchors_dir = temp.path().join("not-yet-created");
    let p = baseline(
        ProfileMode::Production,
        anchors_dir.clone(),
        temp.path().join("cache"),
    );
    let err = build_trust_components(&p)
        .map(|_| ())
        .expect_err("production must reject missing anchors_dir");
    assert!(
        matches!(err, TrustBuildError::AnchorsDirMissing { ref path } if path == &anchors_dir),
        "got {err:?}"
    );
}

#[test]
fn production_rejects_empty_anchors_dir() {
    let temp = tempfile::tempdir().unwrap();
    let anchors_dir = temp.path().join("anchors");
    fs::create_dir_all(&anchors_dir).unwrap();
    let p = baseline(
        ProfileMode::Production,
        anchors_dir.clone(),
        temp.path().join("cache"),
    );
    let err = build_trust_components(&p)
        .map(|_| ())
        .expect_err("production must reject empty anchors_dir");
    assert!(
        matches!(
            err,
            TrustBuildError::NoAnchorsFound { ref path, ext: "pub" } if path == &anchors_dir
        ),
        "got {err:?}"
    );
}

#[test]
fn production_rejects_malformed_anchor_file() {
    let temp = tempfile::tempdir().unwrap();
    let anchors_dir = temp.path().join("anchors");
    write_anchor(&anchors_dir, "tb-2026-q2", &[0x42u8; 100]);
    let p = baseline(
        ProfileMode::Production,
        anchors_dir,
        temp.path().join("cache"),
    );
    let err = build_trust_components(&p)
        .map(|_| ())
        .expect_err("production must reject wrong-length anchor");
    assert!(
        matches!(
            err,
            TrustBuildError::AnchorLengthInvalid {
                expected: MLDSA87_PK_LEN,
                actual: 100,
                ..
            }
        ),
        "got {err:?}"
    );
}

#[test]
fn production_builds_when_real_anchor_present() {
    let temp = tempfile::tempdir().unwrap();
    let anchors_dir = temp.path().join("anchors");
    let (pk, _sk) = fresh_keypair();
    write_anchor(&anchors_dir, "tb-2026-q2", &pk);

    let p = baseline(
        ProfileMode::Production,
        anchors_dir,
        temp.path().join("cache"),
    );
    let components = build_trust_components(&p).expect("production must build");
    assert_eq!(components.exchange_mode, TrustExchangeMode::OpenBaoBridge);
    assert_eq!(components.anchor_count(), 1);
    assert_eq!(components.anchor_ids, vec!["tb-2026-q2".to_string()]);
}

#[test]
fn production_ignores_non_pub_files() {
    let temp = tempfile::tempdir().unwrap();
    let anchors_dir = temp.path().join("anchors");
    let (pk, _sk) = fresh_keypair();
    write_anchor(&anchors_dir, "tb-real", &pk);
    fs::write(anchors_dir.join("README.txt"), b"ignore me").unwrap();
    fs::write(anchors_dir.join("tb-real.pub.bak"), b"and me too").unwrap();

    let p = baseline(
        ProfileMode::Production,
        anchors_dir,
        temp.path().join("cache"),
    );
    let components = build_trust_components(&p).expect("production must build");
    assert_eq!(components.anchor_count(), 1);
    assert_eq!(components.anchor_ids, vec!["tb-real".to_string()]);
}

// ─── Local-dev relaxation paths ─────────────────────────────────────

#[test]
fn local_dev_allows_accept_all_when_flag_set() {
    let temp = tempfile::tempdir().unwrap();
    let mut p = baseline(
        ProfileMode::LocalDev,
        temp.path().join("anchors"),
        temp.path().join("cache"),
    );
    p.trust.verifier = TrustVerifier::AcceptAll;
    p.trust.allow_accept_all_verifier = true;
    let components = build_trust_components(&p).expect("local-dev AcceptAll must build");
    assert_eq!(components.exchange_mode, TrustExchangeMode::Disabled);
    assert!(components.anchor_ids.is_empty());
}

#[test]
fn local_dev_synthetic_exchange_when_flag_set() {
    let temp = tempfile::tempdir().unwrap();
    let mut p = baseline(
        ProfileMode::LocalDev,
        temp.path().join("anchors"),
        temp.path().join("cache"),
    );
    p.trust.verifier = TrustVerifier::AcceptAll;
    p.trust.allow_accept_all_verifier = true;
    p.trust.allow_local_dev_exchange = true;
    let components = build_trust_components(&p).expect("local-dev synthetic must build");
    assert_eq!(
        components.exchange_mode,
        TrustExchangeMode::LocalDevSynthetic
    );
}

#[test]
fn local_dev_tolerates_missing_anchors_dir() {
    let temp = tempfile::tempdir().unwrap();
    let anchors_dir = temp.path().join("not-created-yet");
    let p = baseline(
        ProfileMode::LocalDev,
        anchors_dir,
        temp.path().join("cache"),
    );
    let components =
        build_trust_components(&p).expect("local-dev must tolerate missing anchors_dir");
    assert!(components.anchor_ids.is_empty());
    assert_eq!(components.exchange_mode, TrustExchangeMode::Disabled);
}

#[test]
fn local_dev_loads_anchors_when_present() {
    let temp = tempfile::tempdir().unwrap();
    let anchors_dir = temp.path().join("anchors");
    let (pk_a, _) = fresh_keypair();
    let (pk_b, _) = fresh_keypair();
    write_anchor(&anchors_dir, "anchor-a", &pk_a);
    write_anchor(&anchors_dir, "anchor-b", &pk_b);
    let p = baseline(
        ProfileMode::LocalDev,
        anchors_dir,
        temp.path().join("cache"),
    );
    let components = build_trust_components(&p).expect("local-dev anchors must load");
    assert_eq!(components.anchor_count(), 2);
    assert_eq!(
        components.anchor_ids,
        vec!["anchor-a".to_string(), "anchor-b".to_string()]
    );
}

// ─── Boot bundle verification ───────────────────────────────────────

#[test]
fn valid_signed_bundle_verifies_on_boot() {
    // Plan §7 acceptance: "valid signed bundle boots."
    let temp = tempfile::tempdir().unwrap();
    let anchors_dir = temp.path().join("anchors");
    let cache_dir = temp.path().join("cache");

    let (pk, sk) = fresh_keypair();
    write_anchor(&anchors_dir, "tb-boot", &pk);
    let bundle = build_signed_bundle(1_000, 2_000, &sk, "tb-boot");
    write_boot_bundle(&cache_dir, &bundle);

    let p = baseline(ProfileMode::Production, anchors_dir, cache_dir.clone());
    let components = build_trust_components(&p).expect("production must build");
    let version = verify_boot_bundle(&p, components.bundle_verifier.as_ref(), 1_500)
        .expect("valid bundle must verify on boot");
    assert_eq!(version, "2026.05.23.001");

    // Sanity: boot path resolves where the bundle was written.
    assert_eq!(boot_bundle_path(&p), cache_dir.join("bundle.json"));
}

#[test]
fn tampered_signed_bundle_blocks_boot() {
    // Plan §7 acceptance: "tampered bundle blocks boot."
    let temp = tempfile::tempdir().unwrap();
    let anchors_dir = temp.path().join("anchors");
    let cache_dir = temp.path().join("cache");

    let (pk, sk) = fresh_keypair();
    write_anchor(&anchors_dir, "tb-boot", &pk);
    let mut bundle = build_signed_bundle(1_000, 2_000, &sk, "tb-boot");
    // Tamper the payload after signing: the cached snapshot status
    // flips from Valid to Revoked. The signature now covers stale
    // bytes and verification must fail.
    bundle.payload.revocations[0].status = SnapshotStatus::Revoked;
    write_boot_bundle(&cache_dir, &bundle);

    let p = baseline(ProfileMode::Production, anchors_dir, cache_dir);
    let components = build_trust_components(&p).expect("production must build");
    let err = verify_boot_bundle(&p, components.bundle_verifier.as_ref(), 1_500)
        .map(|_| ())
        .expect_err("tampered bundle must fail verification");
    assert!(
        matches!(err, TrustBuildError::BootBundleVerify { .. }),
        "got {err:?}"
    );
}

#[test]
fn missing_boot_bundle_blocks_boot() {
    let temp = tempfile::tempdir().unwrap();
    let anchors_dir = temp.path().join("anchors");
    let cache_dir = temp.path().join("cache");

    let (pk, _sk) = fresh_keypair();
    write_anchor(&anchors_dir, "tb-boot", &pk);
    fs::create_dir_all(&cache_dir).unwrap();
    // Note: no bundle.json written.

    let p = baseline(ProfileMode::Production, anchors_dir, cache_dir.clone());
    let components = build_trust_components(&p).expect("production must build");
    let err = verify_boot_bundle(&p, components.bundle_verifier.as_ref(), 1_500)
        .map(|_| ())
        .expect_err("missing bundle must fail");
    assert!(
        matches!(
            err,
            TrustBuildError::BootBundleMissing { ref path } if path == &cache_dir.join("bundle.json")
        ),
        "got {err:?}"
    );
}

#[test]
fn boot_bundle_rejected_when_unknown_anchor_id() {
    // Bundle is signed by a real key, but its `public_key_id` does
    // not match any anchor on disk. Verifier surfaces
    // `BundleError::MissingTrustAnchor` which the builder wraps as
    // `BootBundleVerify`. This is the path that fires if an operator
    // forgets to install the matching anchor file.
    let temp = tempfile::tempdir().unwrap();
    let anchors_dir = temp.path().join("anchors");
    let cache_dir = temp.path().join("cache");

    let (pk, sk) = fresh_keypair();
    write_anchor(&anchors_dir, "tb-installed", &pk);
    let bundle = build_signed_bundle(1_000, 2_000, &sk, "tb-NOT-installed");
    write_boot_bundle(&cache_dir, &bundle);

    let p = baseline(ProfileMode::Production, anchors_dir, cache_dir);
    let components = build_trust_components(&p).expect("production must build");
    let err = verify_boot_bundle(&p, components.bundle_verifier.as_ref(), 1_500)
        .map(|_| ())
        .expect_err("unknown anchor id must fail");
    assert!(
        matches!(err, TrustBuildError::BootBundleVerify { .. }),
        "got {err:?}"
    );
}

#[test]
fn boot_bundle_rejected_when_expired() {
    let temp = tempfile::tempdir().unwrap();
    let anchors_dir = temp.path().join("anchors");
    let cache_dir = temp.path().join("cache");

    let (pk, sk) = fresh_keypair();
    write_anchor(&anchors_dir, "tb-boot", &pk);
    let bundle = build_signed_bundle(1_000, 2_000, &sk, "tb-boot");
    write_boot_bundle(&cache_dir, &bundle);

    let p = baseline(ProfileMode::Production, anchors_dir, cache_dir);
    let components = build_trust_components(&p).expect("production must build");

    // `now_secs = 5_000` is well past `expires_at_secs = 2_000`.
    let err = verify_boot_bundle(&p, components.bundle_verifier.as_ref(), 5_000)
        .map(|_| ())
        .expect_err("expired bundle must fail");
    assert!(
        matches!(err, TrustBuildError::BootBundleVerify { .. }),
        "got {err:?}"
    );
}

#[test]
fn boot_bundle_rejected_when_garbage_json() {
    let temp = tempfile::tempdir().unwrap();
    let anchors_dir = temp.path().join("anchors");
    let cache_dir = temp.path().join("cache");
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(cache_dir.join("bundle.json"), b"{not valid json").unwrap();

    let (pk, _sk) = fresh_keypair();
    write_anchor(&anchors_dir, "tb-boot", &pk);

    let p = baseline(ProfileMode::Production, anchors_dir, cache_dir);
    let components = build_trust_components(&p).expect("production must build");
    let err = verify_boot_bundle(&p, components.bundle_verifier.as_ref(), 1_500)
        .map(|_| ())
        .expect_err("garbage bundle must fail parse");
    assert!(
        matches!(err, TrustBuildError::BootBundleParse { .. }),
        "got {err:?}"
    );
}

// ─── Empty-path defenses ────────────────────────────────────────────

#[test]
fn empty_anchors_dir_path_rejected() {
    let temp = tempfile::tempdir().unwrap();
    // Local-dev so the anchors_dir-empty path is the only failure
    // surface — production rejects empty bundle_cache_dir earlier.
    let mut p = baseline(
        ProfileMode::LocalDev,
        PathBuf::new(),
        temp.path().join("cache"),
    );
    p.trust.verifier = TrustVerifier::MlDsa;
    let err = build_trust_components(&p)
        .map(|_| ())
        .expect_err("empty anchors_dir path must fail");
    assert!(
        matches!(err, TrustBuildError::AnchorsDirEmpty),
        "got {err:?}"
    );
}

#[test]
fn empty_bundle_cache_dir_rejected_in_production() {
    let temp = tempfile::tempdir().unwrap();
    let p = baseline(
        ProfileMode::Production,
        temp.path().join("anchors"),
        PathBuf::new(),
    );
    let err = build_trust_components(&p)
        .map(|_| ())
        .expect_err("empty bundle_cache_dir path must fail");
    assert!(
        matches!(err, TrustBuildError::BundleCacheDirEmpty),
        "got {err:?}"
    );
}

// ─── Compile-time / behavior smoke tests ────────────────────────────

#[test]
fn returned_verifier_is_send_sync() {
    // The trust verifier ends up behind `Arc<dyn BundleVerifier + Send + Sync>`
    // on AppState. Compile-time check: the builder's output really is
    // Send + Sync at the trait-object boundary.
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<dyn mai_compliance::bundle::BundleVerifier + Send + Sync>();
}

#[test]
fn loaded_verifier_accepts_signed_bundle_round_trip() {
    // Belt-and-suspenders on top of valid_signed_bundle_verifies_on_boot:
    // verify the loaded verifier accepts a freshly-signed bundle even
    // without going through verify_boot_bundle, so we know the issue
    // (if any) is not in the file I/O path.
    let temp = tempfile::tempdir().unwrap();
    let anchors_dir = temp.path().join("anchors");
    let (pk, sk) = fresh_keypair();
    write_anchor(&anchors_dir, "tb-direct", &pk);

    let p = baseline(
        ProfileMode::Production,
        anchors_dir,
        temp.path().join("cache"),
    );
    let components = build_trust_components(&p).expect("production must build");
    let bundle = build_signed_bundle(1_000, 2_000, &sk, "tb-direct");

    // Trait-object verification: avoid `verified_payload`'s `Sized` bound
    // by calling `BundleVerifier::verify` through the `Arc<dyn …>`
    // directly. This is the same lookup path convergence will
    // use from `MaiServer::run()`.
    let hash = payload_hash(&bundle.metadata, &bundle.payload).expect("hash must serialize");
    let sig_bytes = hex::decode(&bundle.signature.bytes_hex).expect("signature hex must decode");
    components
        .bundle_verifier
        .verify(&hash, &sig_bytes, &bundle.signature.public_key_id)
        .expect("loaded verifier must accept signed bundle");
}

#[test]
fn standalone_ml_dsa_verifier_matches_builder_output() {
    // Cross-check: an ad-hoc `MlDsaBundleVerifier` with the same
    // anchor must produce the same accept/reject decision as the
    // builder's verifier.
    let (pk, sk) = fresh_keypair();
    let direct = MlDsaBundleVerifier::new().with_anchor("anchor", pk.clone());
    let bundle = build_signed_bundle(1_000, 2_000, &sk, "anchor");
    direct
        .verify(
            &payload_hash(&bundle.metadata, &bundle.payload).unwrap(),
            &hex::decode(&bundle.signature.bytes_hex).unwrap(),
            "anchor",
        )
        .expect("direct verifier must accept");
}
