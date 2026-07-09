//! Per-node mTLS for the `aog-wire` raft transport (doctrine I-3:
//! sender-constrained). Consensus RPCs carry over mutually-authenticated TLS: a
//! node presents its own certificate and verifies every peer's against a shared
//! estate CA on BOTH ends. The raft server **requires** a client certificate
//! signed by the estate CA, so a peer with no certificate — or one from an
//! untrusted CA — cannot open the connection, before any RPC is even decoded.
//! Speed still comes from local crypto, never from skipping the check.

use std::sync::Arc;

use rustls::crypto::{CryptoProvider, ring};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, RootCertStore, ServerConfig};

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
