//! C3 bounded bootstrap gate. Without trust, normal admin routes fail closed;
//! exactly one loopback initialize is available and remains consumed after a
//! restart. Remote eligibility is covered by the pure middleware matrix.

use std::time::Duration;

use aog_store::{Op, Precondition};
use aogd::{Client, Config, Daemon, Member};
use tokio::net::TcpListener;

fn scratch() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("aogd-bounded-bootstrap");
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn start(
    data_dir: std::path::PathBuf,
) -> (
    Client,
    String,
    std::sync::Arc<aog_store::raft::RaftNode>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");
    let daemon = Daemon::start(Config {
        node_id: 1,
        data_dir,
        listen: addr,
        advertise: base.clone(),
        anchor_pubkey: None,
        openbao: None,
        node_tls: None,
        allow_insecure_admin: false,
    })
    .await
    .unwrap();
    let node = daemon.node();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            daemon
                .app()
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = Client::new(base.clone());
    for _ in 0..50 {
        if matches!(client.healthz().await, Ok(true)) {
            return (client, base, node, task);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("bounded-bootstrap daemon did not become healthy");
}

#[tokio::test]
async fn loopback_bootstrap_is_one_time_and_persists_across_restart() {
    let data_dir = scratch();
    let (client, base, node, task) = start(data_dir.clone()).await;

    let prebootstrap_write = client
        .write(Op::Put {
            key: "forbidden".to_owned(),
            value: b"x".to_vec(),
            expected: Precondition::Any,
        })
        .await
        .unwrap_err();
    assert!(
        prebootstrap_write
            .to_string()
            .contains("only one local initialize")
    );

    client
        .initialize(vec![Member { id: 1, addr: base }])
        .await
        .unwrap();
    let replay = client
        .initialize(vec![Member {
            id: 1,
            addr: "http://127.0.0.1:9".to_owned(),
        }])
        .await
        .unwrap_err();
    assert!(replay.to_string().contains("only one local initialize"));

    node.raft().shutdown().await.unwrap();
    drop(node);
    task.abort();
    let _ = task.await;

    let (restarted, _base, node, task) = start(data_dir).await;
    let replay_after_restart = restarted
        .initialize(vec![Member {
            id: 1,
            addr: "http://127.0.0.1:9".to_owned(),
        }])
        .await
        .unwrap_err();
    assert!(
        replay_after_restart
            .to_string()
            .contains("only one local initialize")
    );
    node.raft().shutdown().await.unwrap();
    task.abort();
}
