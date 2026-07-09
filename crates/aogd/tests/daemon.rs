//! Three `aogd` daemons form a control plane and replicate a write,
//! driven entirely through the admin API over the `aog-wire` transport (not the
//! in-process `Cluster`). Proves the daemon + admin surface + wire transport are a
//! working multi-node control plane — the unit the Phase-V containerized harness
//! will run in Docker.

use std::time::{Duration, Instant};

use aogd::{Client, Config, Daemon, Member, Op, Precondition};
use tokio::net::TcpListener;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn await_health(client: &Client, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if matches!(client.healthz().await, Ok(true)) {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn await_value(client: &Client, key: &str, value: &[u8], timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if let Ok(Some(v)) = client.get(key).await
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
async fn three_aogd_daemons_form_consensus_through_the_admin_api() {
    let base = scratch("loom-vh2-aogd");

    // Bind loopback listeners first, so peer URLs are known before membership.
    let mut listeners = Vec::new();
    let mut urls = Vec::new();
    for _ in 0..3 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        urls.push(format!("http://127.0.0.1:{port}"));
        listeners.push(l);
    }

    // Start each aogd daemon and serve its combined (raft + admin) app.
    for (i, l) in listeners.into_iter().enumerate() {
        let id = (i + 1) as u64;
        let listen = l.local_addr().unwrap();
        let daemon = Daemon::start(Config {
            node_id: id,
            data_dir: base.join(format!("n{id}")),
            listen,
            advertise: urls[i].clone(),
            anchor_pubkey: None,
            openbao: None,
        })
        .await
        .unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(l, daemon.app()).await;
        });
    }

    let clients: Vec<Client> = urls.iter().map(|u| Client::new(u.clone())).collect();
    for c in &clients {
        assert!(
            await_health(c, Duration::from_secs(5)).await,
            "daemon answers healthz"
        );
    }

    // Form the cluster through node 1's admin API, addressing each peer by URL.
    clients[0]
        .initialize(vec![Member {
            id: 1,
            addr: urls[0].clone(),
        }])
        .await
        .expect("initialize");
    clients[0]
        .add_learner(Member {
            id: 2,
            addr: urls[1].clone(),
        })
        .await
        .expect("add learner 2 over the wire");
    clients[0]
        .add_learner(Member {
            id: 3,
            addr: urls[2].clone(),
        })
        .await
        .expect("add learner 3 over the wire");
    clients[0]
        .change_membership(vec![1, 2, 3])
        .await
        .expect("promote learners to voters");

    // A leader emerges, reported through the admin API.
    let mut leader = None;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if let Ok(status) = clients[0].leader().await
            && let Some(id) = status.leader
        {
            leader = Some(id);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let leader = leader.expect("a leader is elected");
    assert!(
        (1..=3).contains(&leader),
        "the elected leader is a cluster member"
    );

    // A committed write on node 1 replicates to the followers, read back through
    // each follower's own admin API — consensus + replication through the daemon.
    let response = clients[0]
        .write(Op::Put {
            key: "Workload/aogd".to_owned(),
            value: b"v1".to_vec(),
            expected: Precondition::Any,
        })
        .await
        .expect("leader write commits");
    assert!(
        matches!(response, aogd::RaftResponse::Applied { .. }),
        "the write was applied: {response:?}"
    );

    assert!(
        await_value(&clients[1], "Workload/aogd", b"v1", Duration::from_secs(5)).await,
        "the committed write replicated to node 2 over the wire"
    );
    assert!(
        await_value(&clients[2], "Workload/aogd", b"v1", Duration::from_secs(5)).await,
        "the committed write replicated to node 3 over the wire"
    );
}
