//! A Raft snapshot compacts the state machine, and a
//! restart recovers the **exact** estate (every key, value, and revision) from
//! the snapshot plus the log tail. Point-in-time restore is "reopen the durable
//! stores"; the snapshot is what lets the log before it be purged without losing
//! any committed state.

use std::time::Duration;

use aog_store::raft::RaftNode;
use aog_store::{Op, Precondition};

fn base(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[tokio::test]
async fn snapshot_then_restart_reproduces_the_exact_estate() {
    let dir = base("loom-h3-snapshot");

    // Write an estate, snapshot it, and capture the exact committed state.
    let (expected, revision) = {
        let node = RaftNode::bootstrap(1, &dir).await.unwrap();
        for i in 0..20 {
            node.write(Op::Put {
                key: format!("Workload/w{i:02}"),
                value: format!("v{i}").into_bytes(),
                expected: Precondition::Any,
            })
            .await
            .unwrap();
        }
        node.snapshot(Duration::from_secs(10)).await.unwrap();
        assert!(
            node.last_snapshot().is_some(),
            "a snapshot was built (the log can be compacted past it)"
        );
        let estate = node.range("").await.unwrap();
        let rev = node.revision().await;
        assert_eq!(estate.len(), 20, "all writes are in the snapshotted estate");
        node.shutdown().await.unwrap();
        (estate, rev)
    };

    // Restart (restore) from the same dir: the estate is reproduced exactly.
    let node = RaftNode::bootstrap(1, &dir).await.unwrap();
    let restored = node.range("").await.unwrap();
    assert_eq!(
        restored, expected,
        "restore reproduces the exact estate — keys, values, and revisions"
    );
    assert_eq!(
        node.revision().await,
        revision,
        "the global revision is preserved across restore"
    );
    let spot = node
        .get("Workload/w05")
        .await
        .unwrap()
        .expect("key present");
    assert_eq!(spot.value, b"v5", "a spot-checked value survives verbatim");
    node.shutdown().await.unwrap();
}
