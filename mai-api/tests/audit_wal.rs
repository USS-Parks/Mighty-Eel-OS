//! Acceptance tests for `mai_api::audit_wal`.
//!
//! Lives outside the crate so the tests only see the public API
//! (`mai_api::audit_wal::*` + `mai_api::audit::*`). If any test
//! breaks because it needs a private item, that's a signal to widen
//! the public surface — not to leak the internal.
//!
//! Plan §5 acceptance criteria covered here:
//! - API audit survives restart.
//! - Chain verification succeeds after restart across rotation.
//! - Tampered WAL fails verification.
//! - Missing WAL directory fails production startup.
//! - Audit write failure surfaces an Err (the production guard
//!   wires that into the fail-closed policy at convergence).

use std::sync::Arc;
use std::time::Duration;

use mai_api::audit::{AuditManager, AuditWriter, NullSigner, verify_chain};
use mai_api::audit_wal::{WalAuditConfig, WalAuditError, WalAuditWriter};

async fn record_n(mgr: &AuditManager, n: usize) {
    for i in 0..n {
        mgr.record(
            "test-user",
            "Admin",
            "POST",
            &format!("/v1/chat/completions/{i}"),
            200,
            Duration::from_millis(2),
            None,
            None,
        )
        .await
        .expect("record");
    }
}

#[tokio::test]
async fn ship04_api_audit_survives_restart() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = WalAuditConfig::for_dir(temp.path());

    {
        let writer = Arc::new(WalAuditWriter::open(cfg.clone()).await.unwrap());
        let mgr = AuditManager::new(writer.clone(), Arc::new(NullSigner), 0)
            .await
            .unwrap();
        record_n(&mgr, 7).await;
        assert_eq!(writer.entry_count().await.unwrap(), 7);
    } // writer drops here

    let reopened = WalAuditWriter::open(cfg).await.unwrap();
    assert_eq!(reopened.entry_count().await.unwrap(), 7);
    let recent = reopened.read_recent(100).await.unwrap();
    assert_eq!(recent.len(), 7);
}

#[tokio::test]
async fn ship04_chain_verifies_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = WalAuditConfig::for_dir(temp.path());
    {
        let writer = Arc::new(WalAuditWriter::open(cfg.clone()).await.unwrap());
        let mgr = AuditManager::new(writer, Arc::new(NullSigner), 0)
            .await
            .unwrap();
        record_n(&mgr, 12).await;
    }
    let reopened = WalAuditWriter::open(cfg).await.unwrap();
    let entries = reopened.read_recent(100).await.unwrap();
    verify_chain(&entries).expect("chain must verify after restart");
}

#[tokio::test]
async fn ship04_chain_verifies_across_rotation_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = WalAuditConfig::for_dir(temp.path());
    cfg.rotate_bytes = 512; // small enough to force several rotations

    {
        let writer = Arc::new(WalAuditWriter::open(cfg.clone()).await.unwrap());
        let mgr = AuditManager::new(writer, Arc::new(NullSigner), 0)
            .await
            .unwrap();
        record_n(&mgr, 15).await;
    }
    // Confirm we actually rotated, so this isn't a no-op coverage win.
    let rotated_count = std::fs::read_dir(cfg.rotated_dir()).unwrap().count();
    assert!(
        rotated_count >= 1,
        "test setup: expected rotations, got {rotated_count}"
    );

    let reopened = WalAuditWriter::open(cfg).await.unwrap();
    let entries = reopened.read_recent(100).await.unwrap();
    assert_eq!(entries.len(), 15);
    verify_chain(&entries).expect("chain must verify across rotation boundaries");
}

#[tokio::test]
async fn ship04_tampered_wal_fails_open() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = WalAuditConfig::for_dir(temp.path());
    {
        let writer = Arc::new(WalAuditWriter::open(cfg.clone()).await.unwrap());
        let mgr = AuditManager::new(writer, Arc::new(NullSigner), 0)
            .await
            .unwrap();
        record_n(&mgr, 4).await;
    }
    // Tamper: rewrite the last line's status_code in place.
    let current = cfg.current_path();
    let body = std::fs::read_to_string(&current).unwrap();
    let tampered = body.replace("\"status_code\":200", "\"status_code\":418");
    std::fs::write(&current, tampered).unwrap();

    let err = WalAuditWriter::open(cfg)
        .await
        .expect_err("tampered WAL must fail open");
    assert!(matches!(err, WalAuditError::ChainBroken { .. }), "{err:?}");
}

#[tokio::test]
async fn ship04_missing_wal_dir_fails_open() {
    let cfg = WalAuditConfig::for_dir("/__definitely_missing_ship04_wal_dir__/x");
    let err = WalAuditWriter::open(cfg)
        .await
        .expect_err("missing dir must fail open in production-like environments");
    assert!(
        matches!(err, WalAuditError::WalDirMissing { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn ship04_replay_outcome_reports_rotated_count() {
    use mai_api::audit_wal::replay_and_verify;

    let temp = tempfile::tempdir().unwrap();
    let mut cfg = WalAuditConfig::for_dir(temp.path());
    cfg.rotate_bytes = 256;
    {
        let writer = Arc::new(WalAuditWriter::open(cfg.clone()).await.unwrap());
        let mgr = AuditManager::new(writer, Arc::new(NullSigner), 0)
            .await
            .unwrap();
        record_n(&mgr, 8).await;
    }
    let outcome = replay_and_verify(&cfg).await.unwrap();
    assert_eq!(outcome.entry_count, 8);
    assert!(outcome.rotated_files >= 1);
    assert_ne!(
        outcome.last_hash,
        mai_api::audit::MemoryAuditWriter::new()
            .last_hash()
            .await
            .unwrap()
    ); // last_hash must have advanced past genesis
}

#[tokio::test]
async fn ship04_audit_write_failure_surfaces_error() {
    // Construct a writer, then delete its WAL dir under it. The next
    // write must return an error (which the convergence step
    // will route into the fail-closed policy for regulated requests).
    let temp = tempfile::tempdir().unwrap();
    let cfg = WalAuditConfig::for_dir(temp.path());
    let writer = WalAuditWriter::open(cfg.clone()).await.unwrap();

    // Record one entry so current.jsonl exists, then nuke the dir.
    let mgr = AuditManager::new(Arc::new(writer), Arc::new(NullSigner), 0)
        .await
        .unwrap();
    mgr.record(
        "u1",
        "Admin",
        "GET",
        "/v1/health",
        200,
        Duration::from_millis(1),
        None,
        None,
    )
    .await
    .expect("first write before tamper");

    // Drop the dir from under the open writer.
    std::fs::remove_dir_all(temp.path()).unwrap();

    // Next write should fail. On Linux, append to a deleted file may
    // silently succeed because the inode is still open until the file
    // descriptor is closed — but we open / close a file handle per
    // write, so the second attempt has to recreate the path, which
    // will fail when the parent directory is gone.
    let res = mgr
        .record(
            "u1",
            "Admin",
            "GET",
            "/v1/health",
            200,
            Duration::from_millis(1),
            None,
            None,
        )
        .await;
    assert!(res.is_err(), "audit write must fail when WAL dir is gone");
}
