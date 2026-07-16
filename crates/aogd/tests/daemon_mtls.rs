//! C2 control-plane gate: three `aogd` nodes converge over real mutual TLS.
//! No-certificate, wrong-CA, wrong-node RPC identity, and HTTP membership
//! attempts are denied before they can invoke Raft.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use aog_store::Op;
use aog_store::raft::types::TypeConfig;
use aog_wire::tls::{NodeIdentityContract, NodeTls, TlsListener, TlsPeer};
use aogd::{Client, Config, Daemon, Member, NodeTlsProvisioning, Precondition};
use openraft::Vote;
use openraft::raft::{AppendEntriesRequest, InstallSnapshotRequest, VoteRequest};
use tokio::net::TcpListener;

fn openssl(args: &[&str]) {
    let output = Command::new("openssl")
        .args(args)
        .output()
        .expect("openssl on PATH");
    assert!(
        output.status.success(),
        "openssl {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Ca {
    cert_pem: PathBuf,
    key_pem: PathBuf,
    cert_der: PathBuf,
}

struct NodeMaterial {
    cert_pem: PathBuf,
    key_pem: PathBuf,
    cert_der: PathBuf,
    key_der: PathBuf,
}

fn gen_ca(dir: &Path, name: &str) -> Ca {
    let cert_pem = dir.join(format!("{name}.pem"));
    let key_pem = dir.join(format!("{name}.key.pem"));
    let cert_der = dir.join(format!("{name}.der"));
    openssl(&[
        "req",
        "-x509",
        "-newkey",
        "ec",
        "-pkeyopt",
        "ec_paramgen_curve:prime256v1",
        "-nodes",
        "-keyout",
        key_pem.to_str().unwrap(),
        "-out",
        cert_pem.to_str().unwrap(),
        "-subj",
        &format!("/CN={name}"),
        "-days",
        "36500",
        "-addext",
        "basicConstraints=critical,CA:TRUE",
        "-addext",
        "keyUsage=critical,keyCertSign,cRLSign",
    ]);
    openssl(&[
        "x509",
        "-in",
        cert_pem.to_str().unwrap(),
        "-outform",
        "DER",
        "-out",
        cert_der.to_str().unwrap(),
    ]);
    Ca {
        cert_pem,
        key_pem,
        cert_der,
    }
}

fn gen_node(dir: &Path, node_id: u64, ca: &Ca) -> NodeMaterial {
    let stem = format!("node-{node_id}");
    let cert_pem = dir.join(format!("{stem}.pem"));
    let key_pem = dir.join(format!("{stem}.key.pem"));
    let cert_der = dir.join(format!("{stem}.der"));
    let key_der = dir.join(format!("{stem}.key.der"));
    let csr = dir.join(format!("{stem}.csr"));
    let ext = dir.join(format!("{stem}.ext"));
    std::fs::write(
        &ext,
        format!(
            "[ v3 ]\nsubjectAltName = IP:127.0.0.1, URI:spiffe://loom/node/{node_id}\nextendedKeyUsage = serverAuth,clientAuth\nbasicConstraints = CA:FALSE\n"
        ),
    )
    .unwrap();
    openssl(&[
        "req",
        "-newkey",
        "ec",
        "-pkeyopt",
        "ec_paramgen_curve:prime256v1",
        "-nodes",
        "-keyout",
        key_pem.to_str().unwrap(),
        "-out",
        csr.to_str().unwrap(),
        "-subj",
        &format!("/CN={stem}"),
    ]);
    openssl(&[
        "x509",
        "-req",
        "-in",
        csr.to_str().unwrap(),
        "-CA",
        ca.cert_pem.to_str().unwrap(),
        "-CAkey",
        ca.key_pem.to_str().unwrap(),
        "-CAcreateserial",
        "-out",
        cert_pem.to_str().unwrap(),
        "-days",
        "36500",
        "-extfile",
        ext.to_str().unwrap(),
        "-extensions",
        "v3",
    ]);
    openssl(&[
        "x509",
        "-in",
        cert_pem.to_str().unwrap(),
        "-outform",
        "DER",
        "-out",
        cert_der.to_str().unwrap(),
    ]);
    openssl(&[
        "pkcs8",
        "-topk8",
        "-nocrypt",
        "-in",
        key_pem.to_str().unwrap(),
        "-outform",
        "DER",
        "-out",
        key_der.to_str().unwrap(),
    ]);
    NodeMaterial {
        cert_pem,
        key_pem,
        cert_der,
        key_der,
    }
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join("aogd-c2-mtls");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn tls(ca: &Ca, node: &NodeMaterial, node_id: u64, advertise: &str) -> NodeTls {
    let contract =
        NodeIdentityContract::new(node_id, advertise, Duration::from_secs(3600)).unwrap();
    NodeTls::for_node_der(
        vec![std::fs::read(&ca.cert_der).unwrap()],
        vec![std::fs::read(&node.cert_der).unwrap()],
        std::fs::read(&node.key_der).unwrap(),
        &contract,
    )
    .unwrap()
}

fn mtls_http(ca: &Ca, node: &NodeMaterial, node_id: u64, advertise: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .use_preconfigured_tls(tls(ca, node, node_id, advertise).client_config().unwrap())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

async fn await_health(client: &Client) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if matches!(client.healthz().await, Ok(true)) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn three_node_raft_requires_mutual_tls_and_exact_sender_identity() {
    let dir = scratch();
    let ca = gen_ca(&dir, "estate-ca");
    let nodes: Vec<NodeMaterial> = (1..=3).map(|id| gen_node(&dir, id, &ca)).collect();

    let mut listeners = Vec::new();
    let mut urls = Vec::new();
    for _ in 0..3 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        urls.push(format!("https://{}", listener.local_addr().unwrap()));
        listeners.push(listener);
    }

    for (index, listener) in listeners.into_iter().enumerate() {
        let node_id = (index + 1) as u64;
        let daemon = Daemon::start(Config {
            node_id,
            data_dir: dir.join(format!("data-{node_id}")),
            listen: listener.local_addr().unwrap(),
            advertise: urls[index].clone(),
            anchor_pubkey: None,
            openbao: None,
            node_tls: Some(NodeTlsProvisioning::Files {
                ca_der_path: ca.cert_der.clone(),
                cert_der_path: nodes[index].cert_der.clone(),
                key_der_path: nodes[index].key_der.clone(),
                minimum_remaining: Duration::from_secs(3600),
            }),
            allow_insecure_admin: true,
        })
        .await
        .unwrap();
        let server = daemon.node_tls().unwrap().server_config().unwrap();
        let app = daemon.app();
        tokio::spawn(async move {
            axum::serve(
                TlsListener::new(listener, server),
                app.into_make_service_with_connect_info::<TlsPeer>(),
            )
            .await
            .unwrap();
        });
    }

    let http = mtls_http(&ca, &nodes[0], 1, &urls[0]);
    let clients: Vec<Client> = urls
        .iter()
        .map(|url| Client::with_http(url.clone(), http.clone()))
        .collect();
    for client in &clients {
        assert!(await_health(client).await, "mTLS daemon answers healthz");
    }

    clients[0]
        .initialize(vec![Member {
            id: 1,
            addr: urls[0].clone(),
        }])
        .await
        .unwrap();
    clients[0]
        .add_learner(Member {
            id: 2,
            addr: urls[1].clone(),
        })
        .await
        .unwrap();
    clients[0]
        .add_learner(Member {
            id: 3,
            addr: urls[2].clone(),
        })
        .await
        .unwrap();
    clients[0].change_membership(vec![1, 2, 3]).await.unwrap();
    let response = clients[0]
        .write(Op::Put {
            key: "Workload/mtls".to_owned(),
            value: b"v1".to_vec(),
            expected: Precondition::Any,
        })
        .await
        .unwrap();
    assert!(matches!(response, aogd::RaftResponse::Applied { .. }));

    for client in &clients {
        let value = client.get("Workload/mtls").await.unwrap().unwrap();
        assert_eq!(value.value, b"v1");
    }

    let no_cert = reqwest::Client::builder()
        .add_root_certificate(
            reqwest::Certificate::from_der(&std::fs::read(&ca.cert_der).unwrap()).unwrap(),
        )
        .build()
        .unwrap();
    assert!(
        no_cert
            .get(format!("{}/healthz", urls[0]))
            .send()
            .await
            .is_err(),
        "a client without a certificate must fail before HTTP"
    );

    let rogue_dir = dir.join("rogue");
    std::fs::create_dir_all(&rogue_dir).unwrap();
    let rogue_ca = gen_ca(&rogue_dir, "rogue-ca");
    let rogue = gen_node(&rogue_dir, 99, &rogue_ca);
    let mut identity_pem = std::fs::read(&rogue.cert_pem).unwrap();
    identity_pem.extend_from_slice(&std::fs::read(&rogue.key_pem).unwrap());
    let rogue_http = reqwest::Client::builder()
        .add_root_certificate(
            reqwest::Certificate::from_der(&std::fs::read(&ca.cert_der).unwrap()).unwrap(),
        )
        .identity(reqwest::Identity::from_pem(&identity_pem).unwrap())
        .build()
        .unwrap();
    assert!(
        rogue_http
            .get(format!("{}/healthz", urls[0]))
            .send()
            .await
            .is_err(),
        "a certificate from a rogue CA must fail before HTTP"
    );

    let http_membership = clients[0]
        .add_learner(Member {
            id: 4,
            addr: "http://127.0.0.1:9".to_owned(),
        })
        .await
        .unwrap_err();
    assert!(http_membership.to_string().contains("requires https://"));

    let forged_vote = VoteRequest::new(Vote::new(999, 3), None);
    let response = http
        .post(format!("{}/raft/vote", urls[1]))
        .body(serde_json::to_vec(&forged_vote).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

    let forged_append = AppendEntriesRequest::<TypeConfig> {
        vote: Vote::new(999, 3),
        prev_log_id: None,
        entries: vec![],
        leader_commit: None,
    };
    let response = http
        .post(format!("{}/raft/append-entries", urls[1]))
        .body(serde_json::to_vec(&forged_append).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

    let forged_snapshot = InstallSnapshotRequest::<TypeConfig> {
        vote: Vote::new(999, 3),
        meta: openraft::SnapshotMeta::default(),
        offset: 0,
        data: vec![],
        done: true,
    };
    let response = http
        .post(format!("{}/raft/install-snapshot", urls[1]))
        .body(serde_json::to_vec(&forged_snapshot).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
}
