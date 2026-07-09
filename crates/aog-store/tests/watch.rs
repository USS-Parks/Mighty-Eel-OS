//! The informer tracks applied changes, and reconstructs the full
//! state after a dropped/lagged watch — no missed final state.

use std::path::PathBuf;

use aog_store::raft::RaftNode;
use aog_store::{Op, Precondition};

fn put(key: &str, val: &str) -> Op {
    Op::Put {
        key: key.to_owned(),
        value: val.as_bytes().to_vec(),
        expected: Precondition::Any,
    }
}

fn del(key: &str) -> Op {
    Op::Delete {
        key: key.to_owned(),
        expected: Precondition::Any,
    }
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[tokio::test]
async fn informer_tracks_writes_and_deletes() {
    let dir = fresh_dir("aog-k4-track");
    let node = RaftNode::bootstrap(1, &dir).await.unwrap();

    let mut informer = node.informer("tenant/");
    informer.resync().await.unwrap();
    assert!(informer.snapshot().is_empty());

    node.write(put("tenant/acme", "v1")).await.unwrap();
    node.write(put("other/x", "z")).await.unwrap(); // outside the prefix
    informer.poll().await.unwrap();
    assert_eq!(informer.snapshot().len(), 1);
    assert!(informer.snapshot().contains_key("tenant/acme"));

    node.write(put("tenant/acme", "v2")).await.unwrap();
    informer.poll().await.unwrap();
    assert_eq!(
        informer.snapshot().get("tenant/acme").unwrap().value,
        b"v2".to_vec()
    );

    node.write(del("tenant/acme")).await.unwrap();
    informer.poll().await.unwrap();
    assert!(informer.snapshot().is_empty());

    node.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn informer_reconstructs_full_state_after_lagged_drop() {
    let dir = fresh_dir("aog-k4-lag");
    let node = RaftNode::bootstrap(1, &dir).await.unwrap();

    // Some state exists before the informer starts (a reconnect).
    for i in 0..5 {
        node.write(put(&format!("tenant/{i:03}"), "v"))
            .await
            .unwrap();
    }

    let mut informer = node.informer("tenant/");
    informer.resync().await.unwrap();
    assert_eq!(informer.snapshot().len(), 5);

    // Flood the stream far past the watch buffer (64) without polling — the
    // subscription lags and drops events (a dropped connection).
    for i in 5..105 {
        node.write(put(&format!("tenant/{i:03}"), "v"))
            .await
            .unwrap();
    }

    // Poll must detect the lag and re-list, reconstructing the full state.
    informer.poll().await.unwrap();

    let authoritative = node.range("tenant/").await.unwrap();
    assert_eq!(
        informer.snapshot().len(),
        authoritative.len(),
        "informer must match authoritative state after a lagged drop"
    );
    assert_eq!(informer.snapshot().len(), 105);
    for (key, _) in &authoritative {
        assert!(
            informer.snapshot().contains_key(key),
            "missing {key} after resync"
        );
    }

    node.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
