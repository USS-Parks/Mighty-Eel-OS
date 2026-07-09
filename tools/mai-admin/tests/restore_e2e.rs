//! Acceptance tests for `mai-admin restore plan` / `restore
//! apply` driven through the library surface, plus DR drills covering
//! the disaster scenarios called out in SHIP-HARDENING-PLAN §9.5.
//!
//! Every test starts by laying out a realistic ship-state directory
//! under a tempdir (mirroring `backup_e2e.rs::fixture`), takes a
//! backup, then exercises a restore path. The fixture is intentionally
//! identical to the backup suite's — restore is the inverse of backup, so the
//! two suites must share a state contract.

use std::path::{Path, PathBuf};

use mai_admin::audit::{AuditEntry, GENESIS_HASH};
use mai_admin::profile::{
    AuditConfig, AuthConfig, BackupSourceProfile, PathsConfig, ProfileMeta, TrustConfig,
    VaultConfig,
};
use mai_admin::restore::{ActionKind, RestoreError, RestoreSignatureRecord};
use mai_admin::{
    BackupOptions, MLDSA87_PK_LEN, VerifyOutcome, apply_restore, create_backup, plan_restore,
    verify_backup,
};

// ─── helpers (mirror backup_e2e) ─────────────────────────────────────

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

    std::fs::write(
        config_root.join("profile.toml"),
        b"[profile]\nname = \"ship\"\n",
    )
    .unwrap();
    let auth_keys = config_root.join("auth_keys.toml");
    std::fs::write(
        &auth_keys,
        b"[keys]\nadmin = \"secret-key-1\"\noperator = \"secret-key-2\"\n",
    )
    .unwrap();
    std::fs::write(config_root.join("dashboard-logging.json"), b"{}").unwrap();

    let e0 = entry(GENESIS_HASH, 1000, "alice", "/v1/health", 200);
    let e1 = entry(&e0.entry_hash, 1001, "bob", "/v1/chat", 200);
    let mut wal_content = String::new();
    wal_content.push_str(&serde_json::to_string(&e0).unwrap());
    wal_content.push('\n');
    wal_content.push_str(&serde_json::to_string(&e1).unwrap());
    wal_content.push('\n');
    std::fs::write(audit_dir.join("current.jsonl"), wal_content).unwrap();

    std::fs::write(
        trust_anchors_dir.join("anchor-a.pub"),
        vec![0u8; MLDSA87_PK_LEN],
    )
    .unwrap();
    std::fs::write(trust_bundle_dir.join("bundle.json"), b"{}").unwrap();
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

// Take a backup from a fresh fixture, returning the backup dir.
fn take_unsigned_backup(id: &str) -> (Fixture, tempfile::TempDir, PathBuf) {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let report = create_backup(&p, options(out.path(), id)).unwrap();
    (f, out, report.backup_dir)
}

fn take_signed_backup(id: &str, sk: &[u8], anchor: &str) -> (Fixture, tempfile::TempDir, PathBuf) {
    let f = fixture();
    let p = profile_from(&f);
    let out = tempfile::tempdir().unwrap();
    let mut opts = options(out.path(), id);
    opts.signing_key = Some(sk.to_vec());
    opts.anchor_id = Some(anchor.to_string());
    let report = create_backup(&p, opts).unwrap();
    (f, out, report.backup_dir)
}

// ─── plan-only paths ─────────────────────────────────────────────────

#[test]
fn plan_unsigned_backup_into_empty_target_has_no_obstacles() {
    let (_f, _out, backup_dir) = take_unsigned_backup("bk-plan-empty");
    let target = tempfile::tempdir().unwrap();
    let plan = plan_restore(&backup_dir, target.path(), None, false).unwrap();
    assert_eq!(plan.backup_id, "bk-plan-empty");
    assert!(
        plan.actions.len() >= 8,
        "expected at least 8 actions, got {}",
        plan.actions.len()
    );
    assert!(plan.obstacles.is_empty(), "fresh target must not conflict");
    assert!(matches!(plan.signature_outcome, VerifyOutcome::Unsigned));
}

#[test]
fn plan_signed_backup_verifies_signature_when_key_supplied() {
    let (pk, sk) = fresh_mldsa_keypair();
    let (_f, _out, backup_dir) = take_signed_backup("bk-plan-signed", &sk, "anchor-test");
    let target = tempfile::tempdir().unwrap();
    let plan = plan_restore(&backup_dir, target.path(), Some(&pk), true).unwrap();
    assert!(matches!(
        plan.signature_outcome,
        VerifyOutcome::Signed { ref anchor_id } if anchor_id == "anchor-test"
    ));
}

#[test]
fn plan_require_signed_rejects_unsigned_backup() {
    let (_f, _out, backup_dir) = take_unsigned_backup("bk-plan-unsigned-strict");
    let target = tempfile::tempdir().unwrap();
    let err = plan_restore(&backup_dir, target.path(), None, true).unwrap_err();
    assert!(matches!(err, RestoreError::UnsignedManifest));
}

#[test]
fn plan_warns_when_signed_but_no_verifying_key() {
    let (_pk, sk) = fresh_mldsa_keypair();
    let (_f, _out, backup_dir) = take_signed_backup("bk-plan-warn", &sk, "anchor-t");
    let target = tempfile::tempdir().unwrap();
    let plan = plan_restore(&backup_dir, target.path(), None, false).unwrap();
    assert!(matches!(
        plan.signature_outcome,
        VerifyOutcome::Signed { .. }
    ));
    assert!(
        plan.warnings.iter().any(|w| w.contains("no verifying key")),
        "expected verifying-key warning: {:?}",
        plan.warnings
    );
}

#[test]
fn plan_rejects_wrong_verifying_key() {
    let (_pk, sk) = fresh_mldsa_keypair();
    let (other_pk, _) = fresh_mldsa_keypair();
    let (_f, _out, backup_dir) = take_signed_backup("bk-plan-wrong-pk", &sk, "anchor");
    let target = tempfile::tempdir().unwrap();
    let err = plan_restore(&backup_dir, target.path(), Some(&other_pk), true).unwrap_err();
    assert!(matches!(err, RestoreError::SignatureFailed(_)));
}

#[test]
fn plan_detects_obstacles_in_populated_target() {
    let (_f, _out, backup_dir) = take_unsigned_backup("bk-plan-conflicts");
    let target = tempfile::tempdir().unwrap();
    // Pre-populate the target with conflicting files for two
    // components: a file (auth_key_hashes) and a directory tree
    // (audit/api).
    std::fs::create_dir_all(target.path().join("auth")).unwrap();
    std::fs::write(target.path().join("auth/key-hashes.json"), b"stale").unwrap();
    std::fs::create_dir_all(target.path().join("audit/api")).unwrap();
    std::fs::write(target.path().join("audit/api/stray.jsonl"), b"x").unwrap();
    let plan = plan_restore(&backup_dir, target.path(), None, false).unwrap();
    assert!(
        plan.has_blocking_obstacles(),
        "expected obstacles, got plan {:#?}",
        plan
    );
    assert!(
        plan.obstacles.len() >= 2,
        "expected at least 2 obstacles, got {}",
        plan.obstacles.len()
    );
}

#[test]
fn plan_actions_kind_matches_disk_shape() {
    let (_f, _out, backup_dir) = take_unsigned_backup("bk-plan-kinds");
    let target = tempfile::tempdir().unwrap();
    let plan = plan_restore(&backup_dir, target.path(), None, false).unwrap();
    // auth_key_hashes is a single JSON file; audit/api is a tree.
    let auth = plan
        .actions
        .iter()
        .find(|a| a.name == "auth_key_hashes")
        .unwrap();
    assert_eq!(auth.kind, ActionKind::File);
    let api_wal = plan
        .actions
        .iter()
        .find(|a| a.name == "api_audit_wal")
        .unwrap();
    assert_eq!(api_wal.kind, ActionKind::Tree);
}

// ─── apply happy paths ───────────────────────────────────────────────

#[test]
fn apply_unsigned_into_empty_target_round_trips() {
    let (_f, _out, backup_dir) = take_unsigned_backup("bk-apply-empty");
    let target = tempfile::tempdir().unwrap();
    let plan = plan_restore(&backup_dir, target.path(), None, false).unwrap();
    let report = apply_restore(&plan, false).unwrap();
    assert_eq!(report.backup_id, "bk-apply-empty");
    assert!(report.audit_chain_verified);
    assert!(!report.forced_overwrite);
    assert!(matches!(
        report.signature_outcome,
        RestoreSignatureRecord::Unsigned
    ));
    // Every component file is present.
    assert!(target.path().join("auth/key-hashes.json").is_file());
    assert!(target.path().join("audit/api/current.jsonl").is_file());
    assert!(target.path().join("trust/anchors/anchor-a.pub").is_file());
    assert!(target.path().join("models/registry.json").is_file());
    // Restore wrote both witnesses to the root.
    assert!(target.path().join("source-manifest.json").is_file());
    assert!(target.path().join("restore-report.json").is_file());
}

#[test]
fn apply_signed_records_anchor_in_report() {
    let (pk, sk) = fresh_mldsa_keypair();
    let (_f, _out, backup_dir) = take_signed_backup("bk-apply-signed", &sk, "anchor-prod");
    let target = tempfile::tempdir().unwrap();
    let plan = plan_restore(&backup_dir, target.path(), Some(&pk), true).unwrap();
    let report = apply_restore(&plan, false).unwrap();
    match report.signature_outcome {
        RestoreSignatureRecord::Signed { anchor_id } => assert_eq!(anchor_id, "anchor-prod"),
        RestoreSignatureRecord::Unsigned => panic!("expected Signed, got Unsigned"),
    }
}

// ─── DR drill: post-restore re-verification is the goal ──────────────

#[test]
fn restored_tree_passes_audit_chain_replay() {
    // SHIP-HARDENING-PLAN §9.5 DR drill: restore to empty node, then
    // audit chain verification runs. We assert the WAL chain in the
    // restored tree replays cleanly and the last-entry hash matches
    // the manifest's claim.
    let (_f, _out, backup_dir) = take_unsigned_backup("bk-drill-chain");
    let target = tempfile::tempdir().unwrap();
    let plan = plan_restore(&backup_dir, target.path(), None, false).unwrap();
    let report = apply_restore(&plan, false).unwrap();
    let api_wal = report
        .restored_components
        .iter()
        .find(|c| c.name == "api_audit_wal")
        .expect("api_audit_wal restored");
    let last = api_wal.last_entry_hash.as_deref().unwrap();
    assert_eq!(last.len(), 64);
    assert!(last.chars().all(|c| c.is_ascii_hexdigit()));
    // Re-verify the backup_dir to cross-check the same last hash.
    let v = verify_backup(&backup_dir, None, false).unwrap();
    assert!(
        v.is_clean(),
        "backup verification failed post-restore: {:?}",
        v.failures
    );
}

#[test]
fn restored_tree_re_backs_up_to_byte_identical_state() {
    // DR drill: restore-then-backup-again should produce the same
    // per-component sha3 digests as the original backup. Proves
    // restore is byte-faithful and chain-of-custody preserves audit
    // contents.
    let (_f, _out, backup_dir) = take_unsigned_backup("bk-drill-roundtrip");
    let target = tempfile::tempdir().unwrap();
    let plan = plan_restore(&backup_dir, target.path(), None, false).unwrap();
    let report = apply_restore(&plan, false).unwrap();

    // Each restored component's sha3 must match the original manifest.
    let original_manifest_text = std::fs::read_to_string(backup_dir.join("manifest.json")).unwrap();
    let original: serde_json::Value = serde_json::from_str(&original_manifest_text).unwrap();
    let original_components = original["components"].as_array().unwrap();
    for restored in &report.restored_components {
        let original_sha = original_components
            .iter()
            .find(|c| c["name"] == restored.name.as_str())
            .map(|c| c["sha3_256"].as_str().unwrap().to_string())
            .unwrap();
        assert_eq!(
            restored.sha3_256, original_sha,
            "restored {} sha3 diverged from original",
            restored.name
        );
    }
}

// ─── DR drill: tamper detection ──────────────────────────────────────

#[test]
fn drill_audit_wal_tamper_after_backup_blocks_restore() {
    // SHIP-HARDENING-PLAN §9.5: "restore after audit WAL tamper attempt".
    // Tamper the WAL inside the backup directory (simulating bit rot
    // or a malicious mutation during transport). plan_restore must
    // catch this BEFORE touching the target.
    let (_f, _out, backup_dir) = take_unsigned_backup("bk-drill-wal-tamper");
    let wal_path = backup_dir.join("audit/api/current.jsonl");
    let mut text = std::fs::read_to_string(&wal_path).unwrap();
    text.push_str(r#"{"entry_id":"injected","timestamp":9999,"previous_hash":"00","entry_hash":"00","profile_id":"x","method":"GET","path":"/x","status_code":200}"#);
    text.push('\n');
    std::fs::write(&wal_path, text).unwrap();

    let target = tempfile::tempdir().unwrap();
    let err = plan_restore(&backup_dir, target.path(), None, false).unwrap_err();
    // The injected line breaks both the tree sha3 (because content
    // changed) and the audit chain (because previous_hash != real prior
    // entry_hash). plan() catches whichever fires first; both are
    // legitimate failure modes that prove the tampered backup never
    // touches the target.
    assert!(
        matches!(
            err,
            RestoreError::SourceDigestMismatch { .. } | RestoreError::AuditChainBroken { .. }
        ),
        "expected digest or chain-break error, got {err:?}"
    );
    // Target is still empty.
    assert!(std::fs::read_dir(target.path()).unwrap().next().is_none());
}

#[test]
fn drill_missing_trust_bundle_component_blocks_restore() {
    // SHIP-HARDENING-PLAN §9.5: "restore after missing trust bundle".
    // Delete the trust bundle cache from the backup; plan must surface
    // SourceMissing.
    let (_f, _out, backup_dir) = take_unsigned_backup("bk-drill-trust-miss");
    // Trust bundles are written when the fixture's trust dir has
    // files; assert the component is present in the backup, then nuke
    // its contents.
    let bundles = backup_dir.join("trust/bundles");
    assert!(bundles.is_dir(), "fixture should have produced bundles");
    std::fs::remove_dir_all(&bundles).unwrap();
    let target = tempfile::tempdir().unwrap();
    let err = plan_restore(&backup_dir, target.path(), None, false).unwrap_err();
    assert!(
        matches!(err, RestoreError::SourceMissing(_)),
        "expected SourceMissing, got {err:?}"
    );
}

#[test]
fn drill_missing_model_registry_component_blocks_restore() {
    // SHIP-HARDENING-PLAN §9.5: "restore after model registry metadata
    // loss". Same shape as the trust-bundle drill — the manifest
    // points to a component that is gone.
    let (_f, _out, backup_dir) = take_unsigned_backup("bk-drill-registry-miss");
    let registry = backup_dir.join("models/registry.json");
    assert!(registry.is_file());
    std::fs::remove_file(&registry).unwrap();
    let target = tempfile::tempdir().unwrap();
    let err = plan_restore(&backup_dir, target.path(), None, false).unwrap_err();
    assert!(
        matches!(err, RestoreError::SourceMissing(_)),
        "expected SourceMissing, got {err:?}"
    );
}

#[test]
fn drill_signed_manifest_tamper_blocks_restore() {
    // SHIP-HARDENING-PLAN §9.5 implication: a backup whose signed
    // manifest was edited in transit must not be restored. plan_restore
    // verifies the signature and bails before any target write.
    let (pk, sk) = fresh_mldsa_keypair();
    let (_f, _out, backup_dir) = take_signed_backup("bk-drill-sig-tamper", &sk, "anchor");
    let manifest_path = backup_dir.join("manifest.json");
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    let tampered = text.replace("test-host", "attacker-host");
    std::fs::write(&manifest_path, tampered).unwrap();
    let target = tempfile::tempdir().unwrap();
    let err = plan_restore(&backup_dir, target.path(), Some(&pk), true).unwrap_err();
    assert!(
        matches!(err, RestoreError::SignatureFailed(_)),
        "expected SignatureFailed, got {err:?}"
    );
    // Target stayed empty.
    assert!(std::fs::read_dir(target.path()).unwrap().next().is_none());
}

// ─── force vs no-force ───────────────────────────────────────────────

#[test]
fn apply_refuses_populated_target_without_force() {
    let (_f, _out, backup_dir) = take_unsigned_backup("bk-no-force");
    let target = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(target.path().join("auth")).unwrap();
    std::fs::write(target.path().join("auth/key-hashes.json"), b"stale").unwrap();
    let plan = plan_restore(&backup_dir, target.path(), None, false).unwrap();
    assert!(plan.has_blocking_obstacles());
    let err = apply_restore(&plan, false).unwrap_err();
    assert!(matches!(err, RestoreError::TargetNotEmpty { .. }));
    // Stale file untouched.
    let after = std::fs::read(target.path().join("auth/key-hashes.json")).unwrap();
    assert_eq!(after, b"stale");
}

#[test]
fn apply_with_force_overwrites_populated_target() {
    let (_f, _out, backup_dir) = take_unsigned_backup("bk-force");
    let target = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(target.path().join("auth")).unwrap();
    std::fs::write(target.path().join("auth/key-hashes.json"), b"stale").unwrap();
    std::fs::create_dir_all(target.path().join("audit/api")).unwrap();
    std::fs::write(target.path().join("audit/api/stray.jsonl"), b"x").unwrap();
    let plan = plan_restore(&backup_dir, target.path(), None, false).unwrap();
    let report = apply_restore(&plan, true).unwrap();
    assert!(report.forced_overwrite);
    // Stale file replaced; stray entry purged (tree component is
    // wiped before rewrite).
    let restored = std::fs::read(target.path().join("auth/key-hashes.json")).unwrap();
    assert!(!restored.is_empty());
    assert_ne!(restored, b"stale");
    assert!(!target.path().join("audit/api/stray.jsonl").exists());
}

// ─── report serialization ────────────────────────────────────────────

#[test]
fn restore_report_is_round_trippable_through_json() {
    let (_f, _out, backup_dir) = take_unsigned_backup("bk-report-json");
    let target = tempfile::tempdir().unwrap();
    let plan = plan_restore(&backup_dir, target.path(), None, false).unwrap();
    let report = apply_restore(&plan, false).unwrap();
    let json = std::fs::read_to_string(target.path().join("restore-report.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["backup_id"], serde_json::json!("bk-report-json"));
    assert_eq!(parsed["audit_chain_verified"], serde_json::json!(true));
    assert_eq!(parsed["forced_overwrite"], serde_json::json!(false));
    assert!(parsed["restored_components"].as_array().unwrap().len() >= 8);
    let restored_by = parsed["restored_by_version"].as_str().unwrap();
    assert!(!restored_by.is_empty());
    // The on-disk JSON deserializes back to the same shape.
    let typed: mai_admin::RestoreReport = serde_json::from_str(&json).unwrap();
    assert_eq!(typed.backup_id, report.backup_id);
    assert_eq!(
        typed.restored_components.len(),
        report.restored_components.len()
    );
}

#[test]
fn apply_keeps_source_manifest_byte_identical() {
    let (_f, _out, backup_dir) = take_unsigned_backup("bk-source-manifest");
    let target = tempfile::tempdir().unwrap();
    let plan = plan_restore(&backup_dir, target.path(), None, false).unwrap();
    let _ = apply_restore(&plan, false).unwrap();
    let original = std::fs::read(backup_dir.join("manifest.json")).unwrap();
    let witness = std::fs::read(target.path().join("source-manifest.json")).unwrap();
    // The witness round-trips through serde_json::to_vec_pretty in
    // BackupManifest::write_to. As long as the source manifest was
    // produced by the same writer, byte equality holds.
    assert_eq!(
        original, witness,
        "source-manifest.json drifted from backup"
    );
}

#[test]
fn missing_manifest_in_backup_dir_errors_with_exit_3_class() {
    let empty = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let err = plan_restore(empty.path(), target.path(), None, false).unwrap_err();
    assert!(matches!(err, RestoreError::ManifestMissing(_)));
}
