//! A 3-node Loom control plane forms consensus over the real
//! `aog-wire` HTTP transport (not the in-process `Cluster`): a leader is elected,
//! and a committed write replicates to the followers across sockets.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aog_store::raft::RaftNode;
use aog_store::{Op, Precondition};
use aog_wire::{WireNetwork, router};
use openraft::BasicNode;
use tokio::net::TcpListener;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn applied(node: &RaftNode, key: &str, value: &[u8], timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if let Ok(Some(v)) = node.get(key).await
            && v.value == value
        {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn a_three_node_cluster_forms_consensus_over_the_wire() {
    let base = scratch("loom-vh1-wire");

    // Bind loopback listeners first, so peer URLs are known before membership.
    let mut listeners = Vec::new();
    let mut urls = Vec::new();
    for _ in 0..3 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        urls.push(format!("http://127.0.0.1:{port}"));
        listeners.push(l);
    }

    // Start each node on the wire transport and serve its Raft endpoints.
    let mut nodes: Vec<Arc<RaftNode>> = Vec::new();
    for (i, l) in listeners.into_iter().enumerate() {
        let id = (i + 1) as u64;
        let node = Arc::new(
            RaftNode::start_with_network(id, base.join(format!("n{id}")), WireNetwork::new())
                .await
                .unwrap(),
        );
        let app = router(Arc::clone(&node));
        tokio::spawn(async move {
            let _ = axum::serve(l, app).await;
        });
        nodes.push(node);
    }
    // Let the servers begin accepting before any RPC is issued.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Form the cluster from node 1, addressing each peer by its URL.
    let raft1 = nodes[0].raft();
    raft1
        .initialize(BTreeMap::from([(1u64, BasicNode::new(urls[0].clone()))]))
        .await
        .expect("initialize");
    raft1
        .add_learner(2, BasicNode::new(urls[1].clone()), true)
        .await
        .expect("add learner 2 over the wire");
    raft1
        .add_learner(3, BasicNode::new(urls[2].clone()), true)
        .await
        .expect("add learner 3 over the wire");
    raft1
        .change_membership(BTreeSet::from([1u64, 2, 3]), false)
        .await
        .expect("promote learners to voters");

    let leader = nodes[0]
        .wait_for_leader(Duration::from_secs(10))
        .await
        .expect("a leader is elected over the wire");
    assert!(
        (1u64..=3).contains(&leader),
        "the elected leader is a cluster member"
    );

    // A committed write on the leader replicates to the followers across sockets.
    nodes[0]
        .write(Op::Put {
            key: "Workload/wire".to_owned(),
            value: b"v1".to_vec(),
            expected: Precondition::Any,
        })
        .await
        .expect("leader write commits");
    assert!(
        applied(&nodes[1], "Workload/wire", b"v1", Duration::from_secs(5)).await,
        "the committed write replicated to node 2 over the wire"
    );
    assert!(
        applied(&nodes[2], "Workload/wire", b"v1", Duration::from_secs(5)).await,
        "the committed write replicated to node 3 over the wire"
    );
}
