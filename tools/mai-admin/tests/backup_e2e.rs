//! Acceptance tests for `mai-admin backup create` /
//! `backup verify` driven through the library surface.
//!
//! The test fixture lays out a realistic ship-state directory under a
//! tempdir (config/, audit WAL with a valid 2-entry chain, trust
//! anchors, vault root, model registry, reports) and then runs the
//! create -> verify cycle. Failure cases tamper with one component at
//! a time and assert verify catches each one.

use std::path::{Path, PathBuf};

use mai_admin::audit::{AuditEntry, GENESIS_HASH};
use mai_admin::profile::{
    AuditConfig, AuthConfig, BackupSourceProfile, PathsConfig, ProfileMeta, TrustConfig,
    VaultConfig,
};
use mai_admin::{
    BackupOptions, MLDSA87_PK_LEN, MLDSA87_SIG_LEN, MLDSA87_SK_LEN, VerifyOutcome, create_backup,
    verify_backup,
};

// ─── helpers ──────────────────────────────────────────────────────────

/// Mirror of mai-api's `compute_entry_hash`. Vendored here so tests
/// can synthesise a real chain without depending on mai-api during
/// the parallel-session breakage.
fn compute_entry_hash(
    previous_hash: &str,
    timestamp: u64,
    profile_id: &str,
    method: &str,
    path: &str,
    status_code: u16,
) -> String {
    use sha3::{Digest, Sha3_256};
    let mut h = Sha3_256::new();
    h.update(previous_hash.as_bytes());
    h.update(timestamp.to_le_bytes());
    h.update(profile_id.as_bytes());
    h.update(method.as_bytes());
    h.update(path.as_bytes());
    h.update(status_code.to_le_bytes());
    hex::encode(h.finalize())
}

fn entry(prev: &str, ts: u64, profile: &str, path: &str, status: u16) -> AuditEntry {
    let entry_hash = compute_entry_hash(prev, ts, profile, "GET", path, status);
    AuditEntry {
        entry_id: format!("entry-{ts}"),
        timestamp: ts,
        previous_hash: prev.to_string(),
        entry_hash,
        profile_id: profile.to_string(),
        profile_role: "operator".to_string(),
        method: "GET".to_string(),
        path: path.to_string(),
        status_code: status,
        duration_ms: 1,
        model_name: None,
        request_type: None,
        context: None,
        pqc_signature: None,
    }
}

fn fresh_mldsa_keypair() -> (Vec<u8>, Vec<u8>) {
    use ml_dsa::{B32, KeyGen, MlDsa87};
    use rand::RngCore;
    let mut seed_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed_bytes);
    let seed = B32::from(seed_bytes);
    let kp = MlDsa87::key_gen_internal(&seed);
    (
        kp.verifying_key().encode().to_vec(),
        kp.signing_key().encode().to_vec(),
    )
}

struct Fixture {
    _tempdir: tempfile::TempDir,
    state_root: PathBuf,
    config_root: PathBuf,
    audit_dir: PathBuf,
    trust_anchors_dir: PathBuf,
    trust_bundle_dir: PathBuf,
    auth_keys: PathBuf,
}

fn fixture() -> Fixture {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().to_path_buf();

    let state_root = root.join("var/lib/mai");
    let config_root = root.join("etc/mai");
    let audit_dir = state_root.join("audit");
    let trust_anchors_dir = config_root.join("trust-anchors");
    let trust_bundle_dir = state_root.join("trust");

    for p in [
        &state_root,
        &config_root,
        &audit_dir,
        &trust_anchors_dir,
        &trust_bundle_dir,
        &state_root.join("models"),
        &state_root.join("reports"),
    ] {
        std::fs::create_dir_all(p).unwrap();
    }

    // Profile + auth_keys + dashboard-logging templates.
    let profile_path = config_root.join("profile.toml");
    std::fs::write(&profile_path, b"[profile]\nname = \"ship\"\n").unwrap();
    let auth_keys = config_root.join("auth_keys.toml");
    std::fs::write(
        &auth_keys,
        b"[keys]\nadmin = \"secret-key-1\"\noperator = \"secret-key-2\"\n",
    )
    .unwrap();
    std::fs::write(config_root.join("dashboard-logging.json"), b"{}").unwrap();

    // Real audit WAL: two entries chained from genesis.
    let e0 = entry(GENESIS_HASH, 1000, "alice", "/v1/health", 200);
    let e1 = entry(&e0.entry_hash, 1001, "bob", "/v1/chat", 200);
    let mut wal_content = String::new();
    wal_content.push_str(&serde_json::to_string(&e0).unwrap());
    wal_content.push('\n');
    wal_content.push_str(&serde_json::to_string(&e1).unwrap());
    wal_content.push('\n');
    std::fs::write(audit_dir.join("current.jsonl"), wal_content).unwrap();

    // Trust anchor file (exactly 2592 bytes - real ML-DSA-87 size).
    std::fs::write(
        trust_anchors_dir.join("anchor-a.pub"),
        vec![0u8; MLDSA87_PK_LEN],
    )
    .unwrap();

    // Trust bundle cache (single file, opaque payload).
    std::fs::write(trust_bundle_dir.join("bundle.json"), b"{}").unwrap();

    // Model registry + a report file.
    std::fs::write(state_root.join("models/registry.json"), b"{\"models\":[]}").unwrap();
    std::fs::write(state_root.join("reports/2026-05-23.pdf"), b"pretend pdf").unwrap();

    Fixture {
        _tempdir: td,
        state_root,
        config_root,
        audit_dir,
        trust_anchors_dir,
        trust_bundle_dir,
        auth_keys,
    }
}

fn profile_from(f: &Fixture) -> BackupSourceProfile {
    BackupSourceProfile {
        profile: ProfileMeta {
            name: "ship".to_string(),
        },
        paths: PathsConfig {
            state_dir: f.state_root.clone(),
            config_dir: f.config_root.clone(),
        },
        vault: VaultConfig {
            backend: "zfs".to_string(),
            root: f.state_root.join("vault"),
        },
        audit: AuditConfig {
            wal_dir: f.audit_dir.clone(),
        },
        trust: TrustConfig {
            anchors_dir: f.trust_anchors_dir.clone(),
            bundle_cache_dir: f.trust_bundle_dir.clone(),
        },
        auth: AuthConfig {
            auth_keys_path: f.auth_keys.clone(),
        },
    }
}

fn options(out: &Path, backup_id: &str) -> BackupOptions {
    BackupOptions {
        output_root: out.to_path_buf(),
        backup_id: Some(backup_id.to_string()),
        now_secs: 1_716_460_800,
        signing_key: None,
        anchor_id: None,
        mai_version: "0.1.0-test".to_string(),
        git_commit: "deadbee".to_string(),
        migration_version: "test".to_string(),
        host: "test-host".to_string(),
    }
}

// ─── unsigned path ────────────────────────────────────────────────────

#[test]
fn unsigned_create_then_verify_passes() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let report = create_backup(&p, options(out.path(), "bk-unsigned-1")).unwrap();
    assert_eq!(report.backup_id, "bk-unsigned-1");
    assert!(!report.signed);
    assert!(
        report.component_count >= 8,
        "should produce at least 8 components, got {}",
        report.component_count
    );

    let verify = verify_backup(&report.backup_dir, None, false).unwrap();
    assert!(
        verify.is_clean(),
        "unsigned verify failures: {:?}",
        verify.failures
    );
    assert_eq!(verify.signature_outcome, VerifyOutcome::Unsigned);
}

#[test]
fn unsigned_require_signed_fails() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let report = create_backup(&p, options(out.path(), "bk-need-sig")).unwrap();

    let verify = verify_backup(&report.backup_dir, None, true).unwrap();
    assert!(!verify.is_clean(), "require_signed must reject unsigned");
    assert!(
        verify
            .failures
            .iter()
            .any(|f| f.contains("--require-signed"))
    );
}

// ─── signed path ──────────────────────────────────────────────────────

#[test]
fn signed_create_then_verify_passes() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let (pk, sk) = fresh_mldsa_keypair();
    assert_eq!(sk.len(), MLDSA87_SK_LEN);
    assert_eq!(pk.len(), MLDSA87_PK_LEN);

    let mut opts = options(out.path(), "bk-signed-1");
    opts.signing_key = Some(sk);
    opts.anchor_id = Some("anchor-test".to_string());

    let report = create_backup(&p, opts).unwrap();
    assert!(report.signed);

    let verify = verify_backup(&report.backup_dir, Some(&pk), true).unwrap();
    assert!(
        verify.is_clean(),
        "signed verify failures: {:?}",
        verify.failures
    );
    assert_eq!(
        verify.signature_outcome,
        VerifyOutcome::Signed {
            anchor_id: "anchor-test".to_string()
        }
    );
}

#[test]
fn signed_create_with_missing_anchor_id_errors() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let (_pk, sk) = fresh_mldsa_keypair();
    let mut opts = options(out.path(), "bk-missing-anchor");
    opts.signing_key = Some(sk);
    // anchor_id intentionally absent.
    let err = create_backup(&p, opts).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("anchor_id"), "expected anchor_id error: {msg}");
}

#[test]
fn signed_create_with_wrong_length_key_errors() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let mut opts = options(out.path(), "bk-bad-sk-len");
    opts.signing_key = Some(vec![0u8; 100]);
    opts.anchor_id = Some("anchor-test".to_string());
    let err = create_backup(&p, opts).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("ML-DSA-87"),
        "expected ML-DSA-87 length error: {msg}"
    );
}

// ─── tamper detection ────────────────────────────────────────────────

#[test]
fn verify_rejects_tampered_component_file() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let report = create_backup(&p, options(out.path(), "bk-tamper")).unwrap();
    // Tamper: rewrite the auth_key_hashes file with different content.
    let hashes_path = report.backup_dir.join("auth/key-hashes.json");
    assert!(hashes_path.is_file());
    std::fs::write(&hashes_path, b"{\"changed\":true}").unwrap();
    let verify = verify_backup(&report.backup_dir, None, false).unwrap();
    assert!(!verify.is_clean());
    assert!(
        verify
            .failures
            .iter()
            .any(|s| s.contains("auth_key_hashes") && s.contains("mismatch"))
    );
}

#[test]
fn verify_rejects_tampered_signed_manifest() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let (pk, sk) = fresh_mldsa_keypair();
    let mut opts = options(out.path(), "bk-sig-tamper");
    opts.signing_key = Some(sk);
    opts.anchor_id = Some("anchor-test".to_string());
    let report = create_backup(&p, opts).unwrap();
    // Tamper: edit the manifest's host field but leave the signature.
    let manifest_path = report.backup_dir.join("manifest.json");
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    let tampered = text.replace("test-host", "attacker-host");
    std::fs::write(&manifest_path, tampered).unwrap();
    let verify = verify_backup(&report.backup_dir, Some(&pk), true).unwrap();
    assert!(!verify.is_clean());
    assert!(
        verify
            .failures
            .iter()
            .any(|s| s.contains("signature") || s.contains("digest"))
    );
}

#[test]
fn verify_rejects_wrong_verifying_key() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let (_pk, sk) = fresh_mldsa_keypair();
    let (other_pk, _) = fresh_mldsa_keypair();
    let mut opts = options(out.path(), "bk-wrong-pk");
    opts.signing_key = Some(sk);
    opts.anchor_id = Some("anchor-test".to_string());
    let report = create_backup(&p, opts).unwrap();
    let verify = verify_backup(&report.backup_dir, Some(&other_pk), true).unwrap();
    assert!(!verify.is_clean());
    assert!(verify.failures.iter().any(|s| s.contains("signature")));
}

#[test]
fn verify_reports_missing_component_file() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let report = create_backup(&p, options(out.path(), "bk-missing")).unwrap();
    // Delete the auth/key-hashes.json after backup.
    std::fs::remove_file(report.backup_dir.join("auth/key-hashes.json")).unwrap();
    let verify = verify_backup(&report.backup_dir, None, false).unwrap();
    assert!(!verify.is_clean());
    assert!(
        verify
            .failures
            .iter()
            .any(|s| s.contains("missing on disk"))
    );
}

// ─── content guarantees ──────────────────────────────────────────────

#[test]
fn backup_dir_contains_manifest_and_components() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let report = create_backup(&p, options(out.path(), "bk-contents")).unwrap();
    assert!(report.backup_dir.join("manifest.json").is_file());
    assert!(report.backup_dir.join("metadata/build-info.json").is_file());
    assert!(
        report
            .backup_dir
            .join("metadata/config-checksums.json")
            .is_file()
    );
    assert!(report.backup_dir.join("audit/api/current.jsonl").is_file());
    assert!(
        report
            .backup_dir
            .join("trust/anchors/anchor-a.pub")
            .is_file()
    );
    assert!(report.backup_dir.join("vault/snapshot-ref.json").is_file());
    assert!(report.backup_dir.join("auth/key-hashes.json").is_file());
    assert!(report.backup_dir.join("models/registry.json").is_file());
}

#[test]
fn auth_key_hashes_never_contain_raw_secrets() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let report = create_backup(&p, options(out.path(), "bk-no-raw-keys")).unwrap();
    let text = std::fs::read_to_string(report.backup_dir.join("auth/key-hashes.json")).unwrap();
    assert!(
        !text.contains("secret-key-1"),
        "backup leaked raw API key: {text}"
    );
    assert!(
        !text.contains("secret-key-2"),
        "backup leaked raw API key: {text}"
    );
    // Should contain the key IDs (admin, operator) and their hashes.
    assert!(text.contains("admin"));
    assert!(text.contains("operator"));
}

#[test]
fn audit_wal_last_entry_hash_recorded_in_manifest() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let report = create_backup(&p, options(out.path(), "bk-wal-meta")).unwrap();
    let manifest_text = std::fs::read_to_string(&report.manifest_path).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_text).unwrap();
    let components = manifest["components"].as_array().unwrap();
    let api_wal = components
        .iter()
        .find(|c| c["name"] == "api_audit_wal")
        .expect("api_audit_wal component present");
    assert_eq!(api_wal["entry_count"], serde_json::json!(2));
    // last_entry_hash should be present (non-null, hex-looking).
    let last = api_wal["last_entry_hash"].as_str().unwrap();
    assert_eq!(last.len(), 64);
    assert!(last.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn duplicate_backup_id_refuses_to_overwrite() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let _first = create_backup(&p, options(out.path(), "bk-dup")).unwrap();
    let err = create_backup(&p, options(out.path(), "bk-dup")).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("already exists"),
        "expected refuse-overwrite: {msg}"
    );
}

#[test]
fn verify_warns_when_signed_but_no_key_supplied() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let (_pk, sk) = fresh_mldsa_keypair();
    let mut opts = options(out.path(), "bk-warn-no-key");
    opts.signing_key = Some(sk);
    opts.anchor_id = Some("anchor-test".to_string());
    let report = create_backup(&p, opts).unwrap();
    let verify = verify_backup(&report.backup_dir, None, false).unwrap();
    // Signature wasn't checked, but the manifest claims it's signed.
    assert!(matches!(
        verify.signature_outcome,
        VerifyOutcome::Signed { .. }
    ));
    assert!(
        verify
            .warnings
            .iter()
            .any(|w| w.contains("no verifying key"))
    );
}

#[test]
fn manifest_signature_length_matches_mldsa87() {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let (_pk, sk) = fresh_mldsa_keypair();
    let mut opts = options(out.path(), "bk-sig-len");
    opts.signing_key = Some(sk);
    opts.anchor_id = Some("a".to_string());
    let report = create_backup(&p, opts).unwrap();
    let manifest_text = std::fs::read_to_string(&report.manifest_path).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_text).unwrap();
    let hex_sig = manifest["signatures"]["manifest_mldsa"].as_str().unwrap();
    let bytes = hex::decode(hex_sig).unwrap();
    assert_eq!(bytes.len(), MLDSA87_SIG_LEN);
}
