//! Per-node mTLS for the `aog-wire` raft transport (doctrine I-3:
//! sender-constrained). Consensus RPCs carry over mutually-authenticated TLS: a
//! node presents its own certificate and verifies every peer's against a shared
//! estate CA on BOTH ends. The raft server **requires** a client certificate
//! signed by the estate CA, so a peer with no certificate — or one from an
//! untrusted CA — cannot open the connection, before any RPC is even decoded.
//! Speed still comes from local crypto, never from skipping the check.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::connect_info::Connected;
use axum::serve::{IncomingStream, Listener};
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::ServerCertVerifier;
use rustls::crypto::{CryptoProvider, ring};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::{FromDer, X509Certificate};

const NODE_SPIFFE_PREFIX: &str = "spiffe://loom/node/";

/// The immutable identity a node certificate must prove before `aogd` may use
/// it. The SPIFFE URI is derived from the numeric Raft node id rather than from
/// operator-controlled certificate metadata, and the HTTPS advertised host is
/// checked through the same WebPKI name verifier used by the live client leg.
#[derive(Debug, Clone)]
pub struct NodeIdentityContract {
    node_id: u64,
    advertise: String,
    server_name: ServerName<'static>,
    minimum_remaining: Duration,
}

impl NodeIdentityContract {
    /// Build the node identity contract for one advertised membership URI.
    ///
    /// # Errors
    /// Returns [`NodeTlsError::Advertise`] when the URI is not a credential-free
    /// HTTPS origin with a valid DNS name or IP host.
    pub fn new(
        node_id: u64,
        advertise: impl Into<String>,
        minimum_remaining: Duration,
    ) -> Result<Self, NodeTlsError> {
        let advertise = advertise.into();
        let url = reqwest::Url::parse(&advertise)
            .map_err(|e| NodeTlsError::Advertise(format!("invalid URI: {e}")))?;
        if url.scheme() != "https" {
            return Err(NodeTlsError::Advertise(
                "node membership URI must use https".to_owned(),
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(NodeTlsError::Advertise(
                "node membership URI must not contain credentials".to_owned(),
            ));
        }
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err(NodeTlsError::Advertise(
                "node membership URI must be an origin without path, query, or fragment".to_owned(),
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| NodeTlsError::Advertise("node membership URI has no host".to_owned()))?;
        let server_name = ServerName::try_from(host.to_owned())
            .map_err(|e| NodeTlsError::Advertise(format!("invalid host: {e}")))?;
        Ok(Self {
            node_id,
            advertise,
            server_name,
            minimum_remaining,
        })
    }

    /// Stable SPIFFE SAN URI required on this node's leaf certificate.
    #[must_use]
    pub fn spiffe_id(&self) -> String {
        format!("{NODE_SPIFFE_PREFIX}{}", self.node_id)
    }

    /// Canonical advertised HTTPS origin bound to the leaf certificate SAN.
    #[must_use]
    pub fn advertise(&self) -> &str {
        &self.advertise
    }
}

/// Public connection metadata extracted only after rustls has authenticated the
/// peer certificate against the estate CA. Certificate/key bytes never enter
/// request extensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TlsPeer {
    /// Remote TCP address.
    pub socket_addr: std::net::SocketAddr,
    /// Raft node id from the certificate URI SAN, when the authenticated client
    /// is a node. Non-node administrative workloads may legitimately be `None`.
    pub node_id: Option<u64>,
}

/// Axum listener that completes the mandatory client-certificate handshake
/// before returning an IO stream to HTTP parsing.
pub struct TlsListener {
    listener: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
}

impl TlsListener {
    /// Wrap a bound TCP listener with the node's mutual-TLS server config.
    #[must_use]
    pub fn new(listener: tokio::net::TcpListener, config: ServerConfig) -> Self {
        Self {
            listener,
            acceptor: tokio_rustls::TlsAcceptor::from(Arc::new(config)),
        }
    }
}

impl Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = TlsPeer;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (tcp, socket_addr) = match self.listener.accept().await {
                Ok(connection) => connection,
                Err(error) => {
                    tracing::warn!(%error, "AOG TLS listener accept failed");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            let tls = match tokio::time::timeout(Duration::from_secs(10), self.acceptor.accept(tcp))
                .await
            {
                Ok(Ok(tls)) => tls,
                Ok(Err(error)) => {
                    tracing::warn!(%socket_addr, %error, "AOG peer TLS handshake rejected");
                    continue;
                }
                Err(_) => {
                    tracing::warn!(%socket_addr, "AOG peer TLS handshake timed out");
                    continue;
                }
            };
            match authenticated_peer(&tls, socket_addr) {
                Ok(peer) => return (tls, peer),
                Err(error) => {
                    tracing::warn!(%socket_addr, %error, "AOG peer identity SAN rejected");
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr().map(|socket_addr| TlsPeer {
            socket_addr,
            node_id: None,
        })
    }
}

impl<'a> Connected<IncomingStream<'a, TlsListener>> for TlsPeer {
    fn connect_info(stream: IncomingStream<'a, TlsListener>) -> Self {
        *stream.remote_addr()
    }
}

fn authenticated_peer(
    tls: &tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    socket_addr: std::net::SocketAddr,
) -> Result<TlsPeer, NodeTlsError> {
    let leaf = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|chain| chain.first())
        .ok_or(NodeTlsError::Missing("authenticated peer certificate"))?;
    let (remainder, parsed) = X509Certificate::from_der(leaf.as_ref())
        .map_err(|e| NodeTlsError::Certificate(e.to_string()))?;
    if !remainder.is_empty() {
        return Err(NodeTlsError::Certificate(
            "trailing bytes after peer certificate".to_owned(),
        ));
    }
    let san = parsed
        .subject_alternative_name()
        .map_err(|e| NodeTlsError::Certificate(format!("peer subjectAltName: {e}")))?;
    let mut node_id = None;
    if let Some(san) = san {
        for name in &san.value.general_names {
            let GeneralName::URI(uri) = name else {
                continue;
            };
            let Some(raw_id) = uri.strip_prefix(NODE_SPIFFE_PREFIX) else {
                continue;
            };
            let parsed_id = raw_id.parse::<u64>().map_err(|_| {
                NodeTlsError::Certificate("malformed node SPIFFE URI SAN".to_owned())
            })?;
            if node_id.replace(parsed_id).is_some() {
                return Err(NodeTlsError::Certificate(
                    "multiple node SPIFFE URI SANs".to_owned(),
                ));
            }
        }
    }
    Ok(TlsPeer {
        socket_addr,
        node_id,
    })
}

/// Fail-closed node identity/provisioning errors. Variants deliberately contain
/// only field names and public certificate metadata; private-key bytes are never
/// formatted or attached as an error source.
#[derive(Debug, thiserror::Error)]
pub enum NodeTlsError {
    #[error("advertised node identity: {0}")]
    Advertise(String),
    #[error("missing node TLS material: {0}")]
    Missing(&'static str),
    #[error("invalid node certificate: {0}")]
    Certificate(String),
    #[error("node certificate is not bound to {0}")]
    NodeIdentity(String),
    #[error(
        "node certificate expires inside the rotation window (remaining {remaining_secs}s, required {required_secs}s)"
    )]
    Rotation {
        /// Whole seconds remaining before the leaf expires.
        remaining_secs: u64,
        /// Configured minimum remaining lifetime.
        required_secs: u64,
    },
    #[error("invalid node private key or certificate/key pairing: {0}")]
    PrivateKey(String),
}

/// A node's mTLS material: the estate CA roots every peer is verified against,
/// plus this node's own certificate chain and private key (its identity).
pub struct NodeTls {
    roots: Arc<RootCertStore>,
    cert_chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

impl NodeTls {
    /// Assemble from DER: the estate CA certificate, this node's certificate
    /// chain, and its PKCS#8 private key.
    ///
    /// # Errors
    /// [`rustls::Error`] if the CA certificate is malformed.
    pub fn from_der(
        ca: CertificateDer<'static>,
        cert_chain: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
    ) -> Result<Self, rustls::Error> {
        let mut roots = RootCertStore::empty();
        roots.add(ca)?;
        Ok(Self {
            roots: Arc::new(roots),
            cert_chain,
            key,
        })
    }

    /// Validate and assemble a production node identity from DER material.
    ///
    /// Validation happens before a listener is bound: roots and leaf must parse,
    /// the leaf must chain to an estate root for both server and client EKUs, its
    /// DNS/IP SAN must match the advertised membership host, its URI SAN must be
    /// the node-id-derived `spiffe://loom/node/<id>`, the certificate/key pair
    /// must build both rustls legs, and expiry must remain outside the configured
    /// rotation safety window.
    ///
    /// # Errors
    /// [`NodeTlsError`] identifies the rejected public contract field without
    /// ever including private-key bytes.
    pub fn for_node(
        ca_roots: Vec<CertificateDer<'static>>,
        cert_chain: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
        contract: &NodeIdentityContract,
    ) -> Result<Self, NodeTlsError> {
        if ca_roots.is_empty() {
            return Err(NodeTlsError::Missing("estate CA root"));
        }
        let leaf = cert_chain
            .first()
            .ok_or(NodeTlsError::Missing("node certificate chain"))?;

        let (remainder, parsed) = X509Certificate::from_der(leaf.as_ref())
            .map_err(|e| NodeTlsError::Certificate(e.to_string()))?;
        if !remainder.is_empty() {
            return Err(NodeTlsError::Certificate(
                "trailing bytes after leaf certificate".to_owned(),
            ));
        }
        let expected_spiffe = contract.spiffe_id();
        let san = parsed
            .subject_alternative_name()
            .map_err(|e| NodeTlsError::Certificate(format!("subjectAltName: {e}")))?
            .ok_or_else(|| NodeTlsError::NodeIdentity(expected_spiffe.clone()))?;
        let has_node_id = san
            .value
            .general_names
            .iter()
            .any(|name| matches!(name, GeneralName::URI(uri) if *uri == expected_spiffe));
        if !has_node_id {
            return Err(NodeTlsError::NodeIdentity(expected_spiffe));
        }

        let now = UnixTime::now();
        let not_after = parsed.validity().not_after.timestamp();
        let remaining_secs = not_after
            .checked_sub(i64::try_from(now.as_secs()).unwrap_or(i64::MAX))
            .and_then(|seconds| u64::try_from(seconds).ok())
            .unwrap_or(0);
        if remaining_secs < contract.minimum_remaining.as_secs() {
            return Err(NodeTlsError::Rotation {
                remaining_secs,
                required_secs: contract.minimum_remaining.as_secs(),
            });
        }

        let mut roots = RootCertStore::empty();
        for root in ca_roots {
            roots
                .add(root)
                .map_err(|e| NodeTlsError::Certificate(format!("estate CA root: {e}")))?;
        }
        let roots = Arc::new(roots);
        let intermediates = &cert_chain[1..];
        WebPkiServerVerifier::builder_with_provider(Arc::clone(&roots), Self::provider())
            .build()
            .map_err(|e| NodeTlsError::Certificate(format!("server verifier: {e}")))?
            .verify_server_cert(leaf, intermediates, &contract.server_name, &[], now)
            .map_err(|e| NodeTlsError::Certificate(format!("server identity: {e}")))?;
        WebPkiClientVerifier::builder_with_provider(Arc::clone(&roots), Self::provider())
            .build()
            .map_err(|e| NodeTlsError::Certificate(format!("client verifier: {e}")))?
            .verify_client_cert(leaf, intermediates, now)
            .map_err(|e| NodeTlsError::Certificate(format!("client identity: {e}")))?;

        let tls = Self {
            roots,
            cert_chain,
            key,
        };
        tls.server_config()
            .and_then(|_| tls.client_config().map(|_| ()))
            .map_err(|e| NodeTlsError::PrivateKey(e.to_string()))?;
        Ok(tls)
    }

    /// Convert untrusted DER byte buffers from a mounted file or OpenBao KV
    /// record, then apply [`Self::for_node`]. The private-key parse error is
    /// intentionally redacted to its material class.
    ///
    /// # Errors
    /// Returns [`NodeTlsError`] for malformed or contract-violating material.
    pub fn for_node_der(
        ca_roots: Vec<Vec<u8>>,
        cert_chain: Vec<Vec<u8>>,
        key_der: Vec<u8>,
        contract: &NodeIdentityContract,
    ) -> Result<Self, NodeTlsError> {
        let key = PrivateKeyDer::try_from(key_der)
            .map_err(|_| NodeTlsError::PrivateKey("unsupported DER encoding".to_owned()))?;
        Self::for_node(
            ca_roots.into_iter().map(CertificateDer::from).collect(),
            cert_chain.into_iter().map(CertificateDer::from).collect(),
            key,
            contract,
        )
    }

    /// The pure-Rust `ring` crypto provider (the same one `reqwest`'s rustls-tls
    /// uses here — no `aws-lc-rs`, so no new dependency enters the lock).
    fn provider() -> Arc<CryptoProvider> {
        Arc::new(ring::default_provider())
    }

    /// A raft-server config that **requires** a client certificate signed by the
    /// estate CA — the sender constraint. A peer with no certificate, or one the
    /// CA did not sign, is rejected at the TLS layer (fail-closed, doctrine I-4).
    ///
    /// # Errors
    /// [`rustls::Error`] if the client verifier or key material is invalid.
    pub fn server_config(&self) -> Result<ServerConfig, rustls::Error> {
        let verifier =
            WebPkiClientVerifier::builder_with_provider(Arc::clone(&self.roots), Self::provider())
                .build()
                .map_err(|e| rustls::Error::General(e.to_string()))?;
        ServerConfig::builder_with_provider(Self::provider())
            .with_safe_default_protocol_versions()?
            .with_client_cert_verifier(verifier)
            .with_single_cert(self.cert_chain.clone(), self.key.clone_key())
    }

    /// A raft-client config that presents this node's identity and verifies the
    /// peer's server certificate against the estate CA.
    ///
    /// # Errors
    /// [`rustls::Error`] if the key material is invalid.
    pub fn client_config(&self) -> Result<ClientConfig, rustls::Error> {
        ClientConfig::builder_with_provider(Self::provider())
            .with_safe_default_protocol_versions()?
            .with_root_certificates(Arc::clone(&self.roots))
            .with_client_auth_cert(self.cert_chain.clone(), self.key.clone_key())
    }
}
