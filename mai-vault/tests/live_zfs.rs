//! V5/V6/V9 live gate — real ZFS dataset property proof + snapshot lifecycle +
//! encrypted-weights restart recovery.
//!
//! Env-gated (no `#[ignore]`, repo live-test pattern): runs only when
//! `MAI_ZFS_TEST_DATASET` names a **disposable** ZFS dataset this test may
//! write files into, snapshot, roll back, and destroy `mai-snap-*` /
//! `mai-live-gate*` snapshots on. Without the env var it returns cleanly so
//! the offline suite stays green on hosts with no ZFS.
//!
//! Provision a throwaway rig (root on a ZFS-capable host):
//!
//! ```text
//! truncate -s 1G /tmp/mai-zfs-test.img
//! head -c 32 /dev/urandom > /tmp/mai-zfs-test.key
//! zpool create -f -O compression=lz4 -O encryption=on \
//!   -O keyformat=raw -O keylocation=file:///tmp/mai-zfs-test.key \
//!   mai-zfs-test /tmp/mai-zfs-test.img
//! zfs create mai-zfs-test/vault
//! MAI_ZFS_TEST_DATASET=mai-zfs-test/vault \
//!   cargo test -p mai-vault --test live_zfs -- --nocapture
//! zpool destroy mai-zfs-test   # teardown
//! ```
//!
//! What this proves live (plan V5/V6, the mechanism half of the V9 gate):
//! encryption/keystatus/mountpoint/readonly/compression verified against the
//! actual dataset; a nonexistent dataset fails readiness (the "ordinary
//! directory masquerading as ZFS" control); create → list → rollback →
//! destroy run as real bounded `zfs` argv; every snapshot operation lands a
//! receipt on the hash-chained, ML-DSA-signed audit log and the chain
//! verifies afterwards.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;

use mai_core::vault::{AuditStore, ModelStorage, VaultAuditAction, VaultError, VaultInterface};
use mai_vault::audit::AuditWriter;
use mai_vault::pqc::PqcEngine;
use mai_vault::{VaultConfig, ZfsOps, ZfsVault};
use tempfile::TempDir;

fn test_dataset() -> Option<String> {
    std::env::var("MAI_ZFS_TEST_DATASET").ok()
}

const MODEL_ID: &str = "live-gate-model";
const MODEL_BYTES: &[u8] = b"v9-live-gate model weights fixture";

#[tokio::test]
async fn zfs_property_proof_and_snapshot_lifecycle() {
    let Some(dataset) = test_dataset() else {
        eprintln!("SKIP live_zfs: MAI_ZFS_TEST_DATASET unset (V5/V6 live gate)");
        return;
    };

    // --- Discover the rig and self-clean leftover gate snapshots. ----------
    let ops = ZfsOps::system();
    let props = ops
        .dataset_properties(&dataset)
        .await
        .expect("dataset must exist and answer `zfs get`");
    assert_eq!(props.dataset_type, "filesystem", "gate needs a filesystem");
    for snap in ops.list_snapshots(&dataset).await.expect("list") {
        if snap.name.starts_with("mai-snap-") || snap.name.starts_with("mai-live-gate") {
            ops.destroy_snapshot(&dataset, &snap.name)
                .await
                .expect("self-clean leftover gate snapshot");
        }
    }

    // --- V5 negative control: a dataset that does not exist fails, hard. ---
    let missing = format!("{dataset}-noexist-gate");
    let err = ops
        .dataset_properties(&missing)
        .await
        .expect_err("missing dataset must fail the property proof");
    assert!(matches!(err, VaultError::ZfsError(_)));

    // --- Build the vault: PQC + audit wired, real ZFS ops, side dirs kept
    //     OUTSIDE the dataset so rollback cannot rewind them. ---------------
    let side = TempDir::new().unwrap();
    let mut cfg = VaultConfig::default();
    cfg.storage.dataset = dataset.clone();
    cfg.storage.mount_point = std::path::PathBuf::from(&props.mountpoint);
    cfg.storage.staging_dir = side.path().join("staging");
    cfg.storage.compression_enabled = props.compression != "off";
    cfg.pqc.key_store_path = side.path().join("keys");
    cfg.audit.db_path = side.path().join("audit.db");

    let pqc = Arc::new(PqcEngine::new(cfg.pqc.clone()));
    pqc.initialize().await.expect("pqc init");
    let audit = Arc::new(AuditWriter::with_pqc(cfg.audit.clone(), pqc.clone()));
    audit.initialize().await.expect("audit init");

    // Keep a config clone for the V9 restart leg (the vault takes `cfg` by value).
    let cfg_reborn = cfg.clone();
    let vault = ZfsVault::with_engines(cfg, pqc, audit.clone()).with_zfs(ZfsOps::system());

    // --- V5: initialize() runs the live dataset property proof. ------------
    vault.initialize().await.expect("V5 property proof");

    // --- V6: store → snapshot → damage → rollback → destroy. ---------------
    let model_dir = std::path::PathBuf::from(&props.mountpoint).join(MODEL_ID);
    if model_dir.exists() {
        std::fs::remove_dir_all(&model_dir).expect("clean model fixture");
    }
    vault
        .store_model_package(MODEL_ID, MODEL_BYTES)
        .await
        .expect("store model in dataset");

    let snap = vault
        .create_snapshot("v9-live-gate")
        .await
        .expect("snapshot");
    let listed = vault.list_snapshots().await.expect("list snapshots");
    let ours = listed
        .iter()
        .find(|s| s.name == snap.name)
        .expect("created snapshot is listed live");
    assert!(ours.created_at > 0, "creation time comes from ZFS");

    // Damage the dataset state, then roll back to the snapshot.
    std::fs::remove_dir_all(&model_dir).expect("simulate damage");
    assert!(!model_dir.exists());
    vault
        .rollback_snapshot(&snap.name)
        .await
        .expect("rollback to snapshot");
    let restored = vault
        .load_model_weights(MODEL_ID)
        .await
        .expect("model restored by rollback");
    assert_eq!(restored, MODEL_BYTES, "restored bytes match the fixture");

    // Destroy the snapshot; it must vanish from the live listing.
    vault
        .delete_snapshot(&snap.name)
        .await
        .expect("destroy snapshot");
    assert!(
        !vault
            .list_snapshots()
            .await
            .expect("list after destroy")
            .iter()
            .any(|s| s.name == snap.name),
        "destroyed snapshot no longer listed"
    );

    // Unknown snapshot fails precisely.
    let err = vault
        .rollback_snapshot("no-such-snap")
        .await
        .expect_err("miss");
    assert!(matches!(err, VaultError::SnapshotNotFound(_)));

    // --- Receipts: every snapshot op landed on the signed chain. -----------
    let recent = audit.read_recent(50).await.expect("read audit");
    for want in [
        VaultAuditAction::SnapshotCreate,
        VaultAuditAction::SnapshotRollback,
        VaultAuditAction::SnapshotDelete,
    ] {
        assert!(
            recent.iter().any(|e| e.action == want),
            "missing {want:?} receipt"
        );
    }
    audit.verify_chain().await.expect("audit chain verifies");

    // --- V9: encrypted weights survive a process restart on real ZFS. Store a
    //     fresh model, drop the whole vault (in-memory KEM keys gone), then
    //     bring up a NEW vault over the SAME dataset mount + key store: it must
    //     recover the persisted, KEK-wrapped model key and decrypt. -----------
    const RESTART_ID: &str = "v9-restart-model";
    const RESTART_BYTES: &[u8] = b"weights that must survive a real-ZFS restart";
    let restart_dir = std::path::PathBuf::from(&props.mountpoint).join(RESTART_ID);
    if restart_dir.exists() {
        std::fs::remove_dir_all(&restart_dir).expect("clean restart fixture");
    }
    vault
        .store_model_package(RESTART_ID, RESTART_BYTES)
        .await
        .expect("store restart fixture (sealed at rest)");
    // At rest the weights are the KEM+AEAD envelope, not plaintext.
    let sealed = std::fs::read(restart_dir.join("weights.bin")).expect("read sealed weights");
    assert_ne!(
        sealed, RESTART_BYTES,
        "weights must be encrypted on the dataset"
    );

    drop(vault); // simulate process exit: memory cleared; dataset + key store persist.

    let reborn_pqc = Arc::new(PqcEngine::new(cfg_reborn.pqc.clone()));
    reborn_pqc.initialize().await.expect("reborn pqc init");
    // Fresh audit log for the reborn process (a separate file avoids contending
    // with the first writer); the key store is deliberately the SAME.
    let mut cfg_reborn = cfg_reborn;
    cfg_reborn.audit.db_path = side.path().join("audit-reborn.db");
    let reborn_audit = Arc::new(AuditWriter::with_pqc(
        cfg_reborn.audit.clone(),
        reborn_pqc.clone(),
    ));
    reborn_audit.initialize().await.expect("reborn audit init");
    let reborn =
        ZfsVault::with_engines(cfg_reborn, reborn_pqc, reborn_audit).with_zfs(ZfsOps::system());
    reborn.initialize().await.expect("reborn property proof");
    let recovered = reborn
        .load_model_weights(RESTART_ID)
        .await
        .expect("reborn vault recovers the persisted model key and decrypts");
    assert_eq!(
        recovered, RESTART_BYTES,
        "weights sealed before the restart decrypt after it"
    );

    // --- V9 migration: a legacy plaintext model on the dataset is sealed in
    //     place and still loads back to the original bytes. ------------------
    const LEGACY_ID: &str = "v9-legacy-model";
    const LEGACY_BYTES: &[u8] = b"legacy plaintext weights on real ZFS";
    let legacy_dir = std::path::PathBuf::from(&props.mountpoint).join(LEGACY_ID);
    if legacy_dir.exists() {
        std::fs::remove_dir_all(&legacy_dir).expect("clean legacy fixture");
    }
    std::fs::create_dir_all(&legacy_dir).expect("legacy dir");
    std::fs::write(legacy_dir.join("weights.bin"), LEGACY_BYTES).expect("legacy weights");
    std::fs::write(
        legacy_dir.join("manifest.json"),
        serde_json::json!({
            "model_id": LEGACY_ID, "sha256": "legacy", "size_bytes": LEGACY_BYTES.len(),
        })
        .to_string(),
    )
    .expect("legacy manifest");
    reborn
        .initialize()
        .await
        .expect("rescan picks up legacy model");
    assert!(
        reborn
            .migrate_model_to_encrypted(LEGACY_ID)
            .await
            .expect("migrate legacy model"),
        "legacy model migrated"
    );
    let sealed_legacy = std::fs::read(legacy_dir.join("weights.bin")).expect("read migrated");
    assert_ne!(
        sealed_legacy, LEGACY_BYTES,
        "migrated weights are sealed at rest"
    );
    let migrated = reborn
        .load_model_weights(LEGACY_ID)
        .await
        .expect("migrated model loads");
    assert_eq!(migrated, LEGACY_BYTES, "migration preserves the payload");

    println!(
        "V5/V6/V9 live gate PASSED on dataset {dataset} (encryption={}, compression={}): \
         property proof + create/list/rollback/destroy + signed receipts + \
         encrypted-weights restart recovery + legacy-plaintext migration",
        props.encryption, props.compression
    );
}
