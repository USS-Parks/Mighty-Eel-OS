//! VH3 gate — edge `aog-noded` daemons register with the control plane and stay
//! live. A bootstrapped `aogd` control-plane node plus three edge daemons over
//! loopback: each edge writes its `Node` through the admin API and heartbeats, and
//! the control plane sees all three `Ready` and fresh. Proves the edge daemon joins
//! and holds membership over real sockets — the worker side of the containerized
//! Phase-V estate (VH4+).

use std::time::{Duration, Instant};

use aog_estate::{Capacity, Node};
use aog_node::heartbeat::is_stale;
use aog_noded::{NodeAgent, NodeConfig};
use aogd::{Client, Config, Daemon, Member};
use chrono::Utc;
use fabric_contracts::Classification;
use tokio::net::TcpListener;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[tokio::test]
async fn edge_daemons_register_and_heartbeat_into_the_control_plane() {
    let base = scratch("loom-vh3-edge");

    // Control plane: one bootstrapped aogd node.
    let cp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cp_addr = cp_listener.local_addr().unwrap();
    let cp_url = format!("http://127.0.0.1:{}", cp_addr.port());
    let cp = Daemon::start(Config {
        node_id: 1,
        data_dir: base.join("cp"),
        listen: cp_addr,
        advertise: cp_url.clone(),
        anchor_pubkey: None,
        openbao: None,
    })
    .await
    .unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(cp_listener, cp.app()).await;
    });

    let cp_client = Client::new(cp_url.clone());

    // Wait for the control plane, then bootstrap it as a single voter.
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if matches!(cp_client.healthz().await, Ok(true)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cp_client
        .initialize(vec![Member {
            id: 1,
            addr: cp_url.clone(),
        }])
        .await
        .expect("bootstrap control plane");
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if matches!(cp_client.leader().await, Ok(status) if status.leader.is_some()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Edge nodes: start three aog-noded daemons pointed at the control plane.
    let names = ["node-a", "node-b", "node-c"];
    for name in names {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let agent = NodeAgent::new(NodeConfig {
            name: name.to_owned(),
            tenant: "acme".to_owned(),
            ring: 1,
            attestation_floor: Classification::Secret,
            capacity: Capacity {
                cpu_millis: 8000,
                memory_mb: 16384,
                gpu: 0,
                max_workloads: 4,
            },
            control_plane: cp_url.clone(),
            listen: l.local_addr().unwrap(),
            heartbeat: Duration::from_millis(200),
        });
        tokio::spawn(async move {
            let _ = agent.serve(l).await;
        });
    }

    // Each node appears Ready + fresh in the control-plane store, written over the
    // wire through the admin API.
    for name in names {
        let key = format!("Node/{name}");
        let mut live = false;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if let Ok(Some(v)) = cp_client.get(key.as_str()).await
                && let Ok(node) = serde_json::from_slice::<Node>(&v.value)
                && let Some(status) = node.status.as_ref()
                && status.ready
                && !is_stale(status, Utc::now(), 5)
            {
                live = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            live,
            "node {name} registered Ready + fresh in the control plane"
        );
    }
}
