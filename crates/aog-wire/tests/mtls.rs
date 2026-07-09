//! Per-node mTLS on the `aog-wire` raft transport (doctrine I-3,
//! sender-constrained; I-4, fail-closed). A [`NodeTls`] server built from the
//! estate CA **requires** a CA-signed client certificate before any RPC is
//! decoded. Three facts are asserted over a real loopback handshake:
//!
//! 1. a peer presenting a CA-signed certificate completes the *mutual* handshake,
//! 2. a peer presenting *no* client certificate is rejected,
//! 3. a peer whose certificate a *rogue* CA signed is rejected.
//!
//! Certificates are minted at test time with the system `openssl` (no new crate
//! enters the lock, nothing is checked in). The estate CA signs P-256 leaves
//! carrying the EKUs webpki requires (serverAuth / clientAuth) and, for the
//! server, a `localhost` SAN.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use aog_wire::WireNetwork;
use aog_wire::tls::NodeTls;
use rustls::crypto::ring;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

// ───────────────────────── openssl cert-gen (test time) ─────────────────────────

/// Run `openssl` with `args`, asserting it succeeded. Output is captured (quiet
/// on success) and surfaced only if the command fails.
fn openssl(args: &[&str]) {
    let out = Command::new("openssl")
        .args(args)
        .output()
        .expect("openssl on PATH");
    assert!(
        out.status.success(),
        "openssl {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A self-signed P-256 CA: its certificate and key, in PEM.
struct Ca {
    cert_pem: PathBuf,
    key_pem: PathBuf,
}

/// Mint a self-signed P-256 CA (`CA:TRUE`, cert-sign key usage) in `dir`.
fn gen_ca(dir: &Path, name: &str) -> Ca {
    let cert_pem = dir.join(format!("{name}.pem"));
    let key_pem = dir.join(format!("{name}.key.pem"));
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
    Ca { cert_pem, key_pem }
}

/// Mint a P-256 leaf signed by `ca`, carrying the extensions in `ext`. Returns
/// the DER paths of the certificate and its PKCS#8 key.
fn gen_leaf(dir: &Path, name: &str, ca: &Ca, ext: &str) -> (PathBuf, PathBuf) {
    let key_pem = dir.join(format!("{name}.key.pem"));
    let csr = dir.join(format!("{name}.csr"));
    let cert_pem = dir.join(format!("{name}.pem"));
    let cert_der = dir.join(format!("{name}.der"));
    let key_der = dir.join(format!("{name}.key.der"));
    let ext_file = dir.join(format!("{name}.ext"));
    std::fs::write(&ext_file, ext).unwrap();
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
        &format!("/CN={name}"),
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
        ext_file.to_str().unwrap(),
        "-extensions",
        "v3",
    ]);
    to_der(&cert_pem, &cert_der, "x509");
    to_der(&key_pem, &key_der, "pkcs8");
    (cert_der, key_der)
}

/// Convert a PEM `src` to DER `dst` (`kind` is `x509` for a cert, `pkcs8` for a key).
fn to_der(src: &Path, dst: &Path, kind: &str) {
    match kind {
        "x509" => openssl(&[
            "x509",
            "-in",
            src.to_str().unwrap(),
            "-outform",
            "DER",
            "-out",
            dst.to_str().unwrap(),
        ]),
        _ => openssl(&[
            "pkcs8",
            "-topk8",
            "-nocrypt",
            "-in",
            src.to_str().unwrap(),
            "-outform",
            "DER",
            "-out",
            dst.to_str().unwrap(),
        ]),
    }
}

/// The estate CA certificate in DER, for the trust root.
fn ca_der(dir: &Path, ca: &Ca) -> CertificateDer<'static> {
    let der = dir.join("estate-ca.der");
    to_der(&ca.cert_pem, &der, "x509");
    cert(&der)
}

const SERVER_EXT: &str = "[ v3 ]\nsubjectAltName = DNS:localhost, IP:127.0.0.1\nextendedKeyUsage = serverAuth\nbasicConstraints = CA:FALSE\n";
const CLIENT_EXT: &str = "[ v3 ]\nextendedKeyUsage = clientAuth\nbasicConstraints = CA:FALSE\n";

fn cert(path: &Path) -> CertificateDer<'static> {
    CertificateDer::from(std::fs::read(path).unwrap())
}

fn key(path: &Path) -> PrivateKeyDer<'static> {
    PrivateKeyDer::try_from(std::fs::read(path).unwrap()).unwrap()
}

fn roots(ca: CertificateDer<'static>) -> RootCertStore {
    let mut store = RootCertStore::empty();
    store.add(ca).unwrap();
    store
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ─────────────────────────────── handshake ───────────────────────────────

/// Drive one mTLS handshake plus an app-layer ping/pong over loopback. Returns
/// `true` iff BOTH ends completed the mutual handshake and exchanged the bytes.
async fn handshake(server: ServerConfig, client: ClientConfig) -> bool {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server));
    let connector = TlsConnector::from(Arc::new(client));

    let server_task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.ok()?;
        let mut tls = acceptor.accept(tcp).await.ok()?;
        let mut buf = [0u8; 4];
        tls.read_exact(&mut buf).await.ok()?;
        tls.write_all(b"pong").await.ok()?;
        tls.flush().await.ok()?;
        Some(())
    });

    let client_ok = tokio::time::timeout(Duration::from_secs(10), async move {
        let tcp = TcpStream::connect(addr).await.ok()?;
        let name = ServerName::try_from("localhost").ok()?;
        let mut tls = connector.connect(name, tcp).await.ok()?;
        tls.write_all(b"ping").await.ok()?;
        tls.flush().await.ok()?;
        let mut buf = [0u8; 4];
        tls.read_exact(&mut buf).await.ok()?;
        Some(buf == *b"pong")
    })
    .await;

    let server_ok = tokio::time::timeout(Duration::from_secs(10), server_task).await;
    matches!(client_ok, Ok(Some(true))) && matches!(server_ok, Ok(Ok(Some(()))))
}

// ─────────────────────────────── the gate ───────────────────────────────

#[tokio::test]
async fn wire_mtls_requires_ca_signed_client_cert() {
    let dir = scratch("aog-wire-mtls");

    // The estate CA and two members' CA-signed identities.
    let ca = gen_ca(&dir, "estate-ca");
    let anchor = ca_der(&dir, &ca);
    let (server_cert, server_key) = gen_leaf(&dir, "server", &ca, SERVER_EXT);
    let (client_cert, client_key) = gen_leaf(&dir, "client", &ca, CLIENT_EXT);

    let server_tls =
        NodeTls::from_der(anchor.clone(), vec![cert(&server_cert)], key(&server_key)).unwrap();
    let client_tls =
        NodeTls::from_der(anchor.clone(), vec![cert(&client_cert)], key(&client_key)).unwrap();

    // (1) Two CA-signed peers complete the mutual handshake.
    assert!(
        handshake(
            server_tls.server_config().unwrap(),
            client_tls.client_config().unwrap(),
        )
        .await,
        "a CA-signed peer must complete the mutual handshake"
    );

    // (2) A peer presenting NO client certificate is rejected (sender constraint).
    let no_cert = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots(anchor.clone()))
        .with_no_client_auth();
    assert!(
        !handshake(server_tls.server_config().unwrap(), no_cert).await,
        "a peer with no client certificate must be rejected (fail-closed)"
    );

    // (3) A peer whose certificate a ROGUE CA signed is rejected — it still trusts
    // the estate server (so it dials), but the estate CA never signed its identity.
    let rogue_ca = gen_ca(&dir, "rogue-ca");
    let (rogue_cert, rogue_key) = gen_leaf(&dir, "rogue-client", &rogue_ca, CLIENT_EXT);
    let rogue_tls =
        NodeTls::from_der(anchor.clone(), vec![cert(&rogue_cert)], key(&rogue_key)).unwrap();
    assert!(
        !handshake(
            server_tls.server_config().unwrap(),
            rogue_tls.client_config().unwrap(),
        )
        .await,
        "a peer whose certificate the estate CA did not sign must be rejected"
    );
}

#[test]
fn with_tls_builds_a_wire_network() {
    let dir = scratch("aog-wire-mtls-build");
    let ca = gen_ca(&dir, "estate-ca");
    let anchor = ca_der(&dir, &ca);
    let (client_cert, client_key) = gen_leaf(&dir, "client", &ca, CLIENT_EXT);
    let tls = NodeTls::from_der(anchor, vec![cert(&client_cert)], key(&client_key)).unwrap();
    // The mutually-authenticated client config is accepted by the reqwest-backed
    // transport constructor (`WireNetwork::with_tls`).
    assert!(
        WireNetwork::with_tls(tls.client_config().unwrap()).is_ok(),
        "the estate mTLS client config must build a wire transport"
    );
}
