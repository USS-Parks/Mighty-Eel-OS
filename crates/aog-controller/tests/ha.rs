//! A ≥3-node Loom control plane runs real openraft consensus: it elects
//! a leader, replicates committed desired-state to the followers, and on **leader
//! loss elects a new leader within SLO with zero committed-state loss**. A
//! controller's `SharedGate` follows each node's leadership, so only the leader
//! reconciles — leadership is fenced to the gate, not assumed.
//!
//! This is an in-process 3-node cluster over the `Cluster` direct-call network:
//! real election/replication/commit, a real leader failure (the node is
//! partitioned off), a real re-election. (The over-the-wire mTLS transport is
//! deployment packaging; the split-brain-under-real-partition proof is V4.)

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use aog_controller::{LeaderGate, SharedGate};
use aog_store::raft::{Cluster, RaftNode};
use aog_store::{Op, Precondition};

fn base(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Poll a synchronous predicate until true or `timeout` elapses.
async fn eventually<F: Fn() -> bool>(pred: F, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if pred() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Wait until `node` has applied `value` at `key`.
async fn applied(node: &RaftNode, key: &str, value: &[u8], timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if let Some(v) = node.get(key).await.unwrap()
            && v.value == value
        {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn a_three_node_cluster_survives_leader_loss_with_no_committed_state_loss() {
    let dir = base("loom-h1-cluster");
    let cluster = Cluster::new();
    let n1 = RaftNode::join(1, dir.join("n1"), &cluster).await.unwrap();
    let n2 = RaftNode::join(2, dir.join("n2"), &cluster).await.unwrap();
    let n3 = RaftNode::join(3, dir.join("n3"), &cluster).await.unwrap();

    // Form the cluster: node 1 bootstraps, 2 & 3 join as learners then voters.
    n1.initialize(BTreeSet::from([1])).await.unwrap();
    n1.wait_for_leader(Duration::from_secs(10)).await.unwrap();
    n1.add_learner(2).await.unwrap();
    n1.add_learner(3).await.unwrap();
    n1.change_membership(BTreeSet::from([1, 2, 3]))
        .await
        .unwrap();
    assert_eq!(
        n1.current_leader(),
        Some(1),
        "the bootstrap node leads the fresh cluster"
    );

    // A SharedGate follows each node's leadership: only the leader's gate is open.
    let gate1 = SharedGate::new(false);
    let gate2 = SharedGate::new(false);
    let gate3 = SharedGate::new(false);
    gate1.follow(n1.leadership());
    gate2.follow(n2.leadership());
    gate3.follow(n3.leadership());
    assert!(
        eventually(
            || gate1.is_leader() && !gate2.is_leader() && !gate3.is_leader(),
            Duration::from_secs(5),
        )
        .await,
        "only the leader's SharedGate is open"
    );

    // A committed write on the leader replicates to the followers.
    n1.write(Op::Put {
        key: "Workload/gw".to_owned(),
        value: b"v1".to_vec(),
        expected: Precondition::Any,
    })
    .await
    .unwrap();
    assert!(
        applied(&n2, "Workload/gw", b"v1", Duration::from_secs(5)).await
            && applied(&n3, "Workload/gw", b"v1", Duration::from_secs(5)).await,
        "the committed write replicated to both followers"
    );

    // ── Leader loss: partition node 1 off the cluster.
    cluster.isolate(1);

    // A new leader is elected within SLO among the surviving majority {2, 3} —
    // the leader must actually change *off* the partitioned node 1 (a stale
    // `current_leader` still naming 1 does not count).
    assert!(
        eventually(
            || matches!(n2.current_leader(), Some(l) if l != 1),
            Duration::from_secs(10),
        )
        .await,
        "a surviving node takes leadership within SLO"
    );
    let new_leader = n2.current_leader().expect("a leader is established");
    assert!(
        new_leader == 2 || new_leader == 3,
        "the new leader is a survivor, not the partitioned node"
    );

    // Zero committed-state loss: the committed write is intact on the new term.
    let survivor = if new_leader == 2 { &n2 } else { &n3 };
    let value = survivor
        .get("Workload/gw")
        .await
        .unwrap()
        .expect("key present after failover");
    assert_eq!(
        value.value, b"v1",
        "the committed write survived leader loss"
    );

    // A fresh write commits under the new leader (the cluster is live again).
    survivor
        .write(Op::Put {
            key: "Workload/api".to_owned(),
            value: b"v2".to_vec(),
            expected: Precondition::Any,
        })
        .await
        .unwrap();
    let other = if new_leader == 2 { &n3 } else { &n2 };
    assert!(
        applied(other, "Workload/api", b"v2", Duration::from_secs(5)).await,
        "the new leader commits and replicates a fresh write"
    );

    // The SharedGate followed the failover: the new leader's gate opened, so
    // controllers now act on the new leader. (Fencing the *old* partitioned
    // leader — which in classic Raft still believes it leads until it sees a
    // higher term — is split-brain safety, proven under a real partition.)
    let new_gate = if new_leader == 2 { &gate2 } else { &gate3 };
    assert!(
        eventually(|| new_gate.is_leader(), Duration::from_secs(5)).await,
        "the new leader's SharedGate opens"
    );

    n1.shutdown().await.ok();
    n2.shutdown().await.ok();
    n3.shutdown().await.ok();
}
