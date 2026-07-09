//! Node registration. A node joins with a `fabric-identity` leaf signed by
//! the trust anchor, plus its declared attestation profile + capacity (the
//! [`Node`] spec). The control plane admits it only if the leaf verifies against
//! the roster's anchor key **and** names this node; a mis-signed, expired-kind,
//! or wrong-named identity is refused (fail-closed, I-4). A node that cannot
//! prove its identity does not join.

use chrono::{Duration, Utc};

use fabric_contracts::{Identity, IdentityKind, Signature};
use fabric_crypto::{Signer, Verifier};

use aog_estate::Node;

/// Mint a node identity leaf for `node_name` in `tenant`, signed by `issuer`
/// (the trust anchor). The leaf binds the node's name and PKI fingerprint; only
/// the anchor can produce one that verifies, so a node cannot self-assert its
/// membership. `ttl` bounds validity (zero standing privilege, I-1).
pub fn mint_node_identity(
    node_name: &str,
    tenant: &str,
    issuer: &dyn Signer,
    ttl: Duration,
) -> Result<Identity, RegistrationError> {
    let now = Utc::now();
    let identity = Identity {
        identity_id: format!("node:{node_name}"),
        kind: IdentityKind::Workload,
        tenant_id: tenant.to_owned(),
        subject_id: node_name.to_owned(),
        subject_hash: format!("node:{node_name}"),
        service_identity: Some(format!("aog-node/{node_name}")),
        spiffe_id: format!("spiffe://loom/node/{node_name}"),
        pki_cert_fingerprint: issuer.key_id().to_owned(),
        parent_id: None,
        issued_at: now.to_rfc3339(),
        expires_at: (now + ttl).to_rfc3339(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    };
    fabric_identity::mint(identity, issuer).map_err(|e| RegistrationError::Mint(e.to_string()))
}

/// The payload a node presents to join: its declared [`Node`] (attestation
/// profile + capacity) and the identity leaf authorizing the registration.
#[derive(Debug, Clone)]
pub struct NodeRegistration {
    /// The node the agent is registering.
    pub node: Node,
    /// The anchor-signed identity leaf proving membership.
    pub identity: Identity,
}

/// Admits node registrations, verifying each identity against the trust roster's
/// anchor public key.
#[derive(Debug, Clone)]
pub struct Registrar {
    anchor_public_key: Vec<u8>,
}

impl Registrar {
    /// Build a registrar that trusts `anchor_public_key` (the roster key).
    #[must_use]
    pub fn new(anchor_public_key: Vec<u8>) -> Self {
        Self { anchor_public_key }
    }

    /// Verify a registration and return the [`Node`] to admit. Fails closed: a
    /// non-workload identity, one that does not name this node, or one that does
    /// not verify against the anchor is refused — the estate never records a node
    /// whose identity was not proven.
    pub fn admit(
        &self,
        registration: &NodeRegistration,
        verifier: &dyn Verifier,
    ) -> Result<Node, RegistrationError> {
        let identity = &registration.identity;
        if identity.kind != IdentityKind::Workload {
            return Err(RegistrationError::WrongKind);
        }
        if identity.subject_id != registration.node.metadata.name {
            return Err(RegistrationError::SubjectMismatch {
                identity: identity.subject_id.clone(),
                node: registration.node.metadata.name.clone(),
            });
        }
        fabric_identity::verify(identity, verifier, &self.anchor_public_key)
            .map_err(|e| RegistrationError::Unverified(e.to_string()))?;
        Ok(registration.node.clone())
    }
}

/// Why a node registration was refused.
#[derive(Debug, thiserror::Error)]
pub enum RegistrationError {
    /// The identity leaf could not be signed.
    #[error("identity mint failed: {0}")]
    Mint(String),
    /// The identity did not verify against the trust roster anchor.
    #[error("identity did not verify against the trust roster: {0}")]
    Unverified(String),
    /// The identity is not a workload/node identity.
    #[error("identity is not a workload identity")]
    WrongKind,
    /// The identity names a different node than the one registering.
    #[error("identity subject {identity:?} does not name node {node:?}")]
    SubjectMismatch {
        /// The subject the identity claims.
        identity: String,
        /// The node actually registering.
        node: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use aog_estate::{AttestationProfile, Capacity, NodeSpec, Resource};
    use fabric_contracts::Classification;
    use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};

    fn node(name: &str) -> Node {
        Resource::new(
            name,
            NodeSpec {
                ring: 1,
                attestation_floor: Classification::Secret,
                attestation: AttestationProfile::default(),
                capacity: Capacity::default(),
            },
        )
    }

    fn hour() -> Duration {
        Duration::hours(1)
    }

    #[test]
    fn an_anchor_signed_identity_joins() {
        let anchor = RustCryptoMlDsa87::generate("anchor").unwrap();
        let identity = mint_node_identity("node-a", "acme", &anchor, hour()).unwrap();
        let registration = NodeRegistration {
            node: node("node-a"),
            identity,
        };
        let registrar = Registrar::new(anchor.public_key().to_vec());
        assert!(registrar.admit(&registration, &MlDsa87Verifier).is_ok());
    }

    #[test]
    fn a_spoofed_identity_is_rejected() {
        // The leaf is signed by an attacker key, not the anchor.
        let anchor = RustCryptoMlDsa87::generate("anchor").unwrap();
        let attacker = RustCryptoMlDsa87::generate("attacker").unwrap();
        let identity = mint_node_identity("node-a", "acme", &attacker, hour()).unwrap();
        let registration = NodeRegistration {
            node: node("node-a"),
            identity,
        };
        let registrar = Registrar::new(anchor.public_key().to_vec());
        assert!(matches!(
            registrar.admit(&registration, &MlDsa87Verifier),
            Err(RegistrationError::Unverified(_))
        ));
    }

    #[test]
    fn an_identity_naming_another_node_is_rejected() {
        let anchor = RustCryptoMlDsa87::generate("anchor").unwrap();
        // A valid anchor-signed leaf, but for node-b — presented to register node-a.
        let identity = mint_node_identity("node-b", "acme", &anchor, hour()).unwrap();
        let registration = NodeRegistration {
            node: node("node-a"),
            identity,
        };
        let registrar = Registrar::new(anchor.public_key().to_vec());
        assert!(matches!(
            registrar.admit(&registration, &MlDsa87Verifier),
            Err(RegistrationError::SubjectMismatch { .. })
        ));
    }
}
