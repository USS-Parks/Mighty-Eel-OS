//! H2 gate (load-bearing) — split-brain safety under a **real** partition: an
//! injected partition fences the minority (it serves no authoritative allow), and
//! the kill switch is honored under partition (the fenced minority authorizes
//! nothing). A leader isolated into a minority cannot confirm a quorum (openraft's
//! ReadIndex), so it fences even though its stale metrics still call it leader —
//! exactly the classic-Raft split-brain trap the quorum check defeats. The
//! majority elects a leader that CAN confirm and serves the authoritative estate.
//!
//! The partition is a real transport fault (the `Cluster` severs the node's RPCs,
//! so `append_entries`/`vote` genuinely fail) and openraft's real quorum check
//! reacts — not a simulated verdict (A3.2). In-process 3-node cluster; the wire
//! transport is deployment packaging.

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

/// Poll `node.confirm_leadership` until it equals `want` or `timeout` elapses.
async fn confirms(node: &RaftNode, want: bool, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if node.confirm_leadership(Duration::from_millis(500)).await == want {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn a_partitioned_minority_fences_and_the_kill_switch_holds() {
    let dir = base("loom-h2-splitbrain");
    let cluster = Cluster::new();
    let n1 = RaftNode::join(1, dir.join("n1"), &cluster).await.unwrap();
    let n2 = RaftNode::join(2, dir.join("n2"), &cluster).await.unwrap();
    let n3 = RaftNode::join(3, dir.join("n3"), &cluster).await.unwrap();

    n1.initialize(BTreeSet::from([1])).await.unwrap();
    n1.wait_for_leader(Duration::from_secs(10)).await.unwrap();
    n1.add_learner(2).await.unwrap();
    n1.add_learner(3).await.unwrap();
    n1.change_membership(BTreeSet::from([1, 2, 3]))
        .await
        .unwrap();

    // The authoritative estate carries one granted capability.
    n1.write(Op::Put {
        key: "Capability/cap".to_owned(),
        value: b"granted".to_vec(),
        expected: Precondition::Any,
    })
    .await
    .unwrap();
    assert!(
        n1.confirm_leadership(Duration::from_secs(2)).await,
        "a healthy leader confirms a quorum and serves allow"
    );

    // ── Inject a REAL partition: isolate the leader into a minority of one.
    cluster.isolate(1);

    // The minority leader can no longer confirm a quorum → it FENCES: it serves no
    // authoritative allow, even though its stale metrics still call it leader.
    assert!(
        confirms(&n1, false, Duration::from_secs(10)).await,
        "the partitioned minority fences: it cannot confirm a quorum"
    );
    assert!(
        n1.is_leader(),
        "yet its stale metrics still call it leader — the split-brain trap the quorum check defeats"
    );

    // A `SharedGate` driven by the quorum-confirmed check is closed on the
    // minority: its controllers serve no authoritative decision under partition.
    let minority_gate = SharedGate::new(true);
    minority_gate.set(n1.confirm_leadership(Duration::from_millis(500)).await);
    assert!(
        !minority_gate.is_leader(),
        "the minority's gate is closed — no allow served under partition"
    );

    // The majority {2, 3} elects a new leader that CAN confirm a quorum.
    assert!(
        eventually(
            || matches!(n2.current_leader(), Some(l) if l != 1),
            Duration::from_secs(10),
        )
        .await,
        "the majority elects a new leader"
    );
    let new_leader = n2.current_leader().unwrap();
    let survivor = if new_leader == 2 { &n2 } else { &n3 };
    let follower = if new_leader == 2 { &n3 } else { &n2 };
    assert!(
        survivor.confirm_leadership(Duration::from_secs(2)).await,
        "the majority's leader confirms a quorum and serves the authoritative estate"
    );

    // ── Kill switch under partition: revoke the capability on the authoritative
    // (majority) side. It commits there; the fenced minority cannot override it.
    survivor
        .write(Op::Delete {
            key: "Capability/cap".to_owned(),
            expected: Precondition::Any,
        })
        .await
        .unwrap();
    let revoked_on_majority = {
        let start = Instant::now();
        loop {
            if follower.get("Capability/cap").await.unwrap().is_none() {
                break true;
            }
            if start.elapsed() >= Duration::from_secs(5) {
                break false;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    assert!(
        revoked_on_majority,
        "the revocation commits across the majority"
    );

    // The minority still holds the stale `granted` value — but it is fenced, so it
    // authorizes nothing from it: the kill switch holds under the partition.
    assert_eq!(
        n1.get("Capability/cap").await.unwrap().map(|v| v.value),
        Some(b"granted".to_vec()),
        "the partitioned minority still holds stale state..."
    );
    assert!(
        !n1.confirm_leadership(Duration::from_millis(500)).await,
        "...but stays fenced, so a revoked capability is never honored via the minority"
    );

    n1.shutdown().await.ok();
    n2.shutdown().await.ok();
    n3.shutdown().await.ok();
}
