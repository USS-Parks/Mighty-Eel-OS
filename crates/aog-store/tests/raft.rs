//! A linearizable write is committed + applied through openraft, and
//! committed state survives a node restart (recovered from the durable stores).

use std::path::PathBuf;

use aog_store::raft::RaftNode;
use aog_store::raft::types::RaftResponse;
use aog_store::{Op, Precondition};

fn put(key: &str, val: &str, expected: Precondition) -> Op {
    Op::Put {
        key: key.to_owned(),
        value: val.as_bytes().to_vec(),
        expected,
    }
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[tokio::test]
async fn linearizable_write_is_committed_and_applied() {
    let dir = fresh_dir("aog-raft-k3-linearizable");
    let node = RaftNode::bootstrap(1, &dir).await.unwrap();

    let response = node
        .write(put("tenant/acme", "v1", Precondition::Absent))
        .await
        .unwrap();
    assert!(
        matches!(response, RaftResponse::Applied { created: true, .. }),
        "expected an applied create, got {response:?}"
    );

    // The committed state machine reflects the write.
    let value = node.get("tenant/acme").await.unwrap().unwrap();
    assert_eq!(value.value, b"v1".to_vec());
    assert_eq!(node.revision().await, 1);

    // A failed precondition is a rejection value, not a Raft error.
    let response = node
        .write(put("tenant/acme", "v2", Precondition::Absent))
        .await
        .unwrap();
    assert!(matches!(response, RaftResponse::Rejected { .. }));

    node.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn committed_state_survives_restart() {
    let dir = fresh_dir("aog-raft-k3-restart");

    {
        let node = RaftNode::bootstrap(1, &dir).await.unwrap();
        node.write(put("tenant/acme", "v1", Precondition::Absent))
            .await
            .unwrap();
        node.write(put("tenant/acme", "v2", Precondition::Any))
            .await
            .unwrap();
        assert_eq!(node.revision().await, 2);
        node.shutdown().await.unwrap();
    }

    // Restart from the durable stores: committed state must be intact.
    let node = RaftNode::start(1, &dir).await.unwrap();
    let value = node.get("tenant/acme").await.unwrap().unwrap();
    assert_eq!(value.value, b"v2".to_vec());
    assert_eq!(value.version, 2);
    assert_eq!(node.revision().await, 2, "revision recovered after restart");

    node.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
