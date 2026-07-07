//! `wsf-seal` — the WSF seal service. Regulated data crosses the boundary only
//! inside a `fabric-envelope`: sealed (AES-256-GCM under a per-envelope data key
//! that is **wrapped by OpenBao Transit**), labelled (readable without
//! unsealing), and threaded (ML-DSA provenance signature).
//!
//! This service lights up the F4 seal seam: on **seal** it mints a random data
//! key, wraps it via `transit/encrypt`, and stores only the opaque wrap on the
//! envelope; on **unseal** it re-checks the presenting token, `transit/decrypt`s
//! the data key, verifies the provenance thread, and only then recovers the
//! plaintext. **Every operation — allow or deny — is receipted** into a
//! BLAKE3 hash chain (the W4 ledger will ingest these).
//!
//! Fail-closed: a token that does not verify / is expired / lacks clearance for
//! the envelope's classification is denied (and the denial is receipted).

pub mod http;

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use fabric_contracts::{Classification, ComplianceScope, Envelope, Label, Route, TrustToken};
use fabric_crypto::Signer;
use fabric_crypto::providers::MlDsa87Verifier;
use fabric_envelope::{
    EnvelopeBinding, ThreadSpec, envelope_binding, open_envelope, seal_envelope,
};
use fabric_proof::{ChainLink, GENESIS_HASH, canonical_hash, chain_link};
use fabric_revocation::RevocationSnapshot;
use serde::{Deserialize, Serialize};
use wsf_bridge::OpenBaoAuth;

/// Failures from seal-service operations.
#[derive(Debug, thiserror::Error)]
pub enum SealError {
    /// The presenting token failed trust or clearance checks (the deny path).
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    /// An OpenBao interaction (login / transit) failed.
    #[error("openbao: {0}")]
    OpenBao(#[from] wsf_bridge::OpenBaoError),
    /// An envelope operation (seal / unseal / thread) failed.
    #[error("envelope: {0}")]
    Envelope(#[from] fabric_envelope::EnvelopeError),
    /// The transit-unwrapped data key was not 32 bytes.
    #[error("transit data key wrong size")]
    DataKeySize,
}

/// A machine-readable handling label for a payload (mirrors `fabric_contracts::Label`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelSpec {
    /// Data classification.
    pub classification: Classification,
    /// Compliance regimes attached to the payload.
    #[serde(default)]
    pub compliance_scopes: Vec<ComplianceScope>,
    /// Origin marker.
    pub origin: String,
    /// Permitted operations (empty = unrestricted); an unseal requires `unseal`
    /// to be present when this list is non-empty.
    #[serde(default)]
    pub permitted_ops: Vec<String>,
    /// Permitted routing destinations.
    #[serde(default)]
    pub permitted_destinations: Vec<Route>,
    /// Detected sensitive entities.
    #[serde(default)]
    pub detected_entities: Vec<String>,
}

impl From<LabelSpec> for Label {
    fn from(s: LabelSpec) -> Self {
        Label {
            classification: s.classification,
            compliance_scopes: s.compliance_scopes,
            origin: s.origin,
            permitted_ops: s.permitted_ops,
            permitted_destinations: s.permitted_destinations,
            detected_entities: s.detected_entities,
        }
    }
}

/// A request to seal a payload into an envelope.
pub struct SealRequest {
    /// The trust token authorizing the seal (its id is threaded into provenance).
    pub token: TrustToken,
    /// The plaintext to seal.
    pub plaintext: Vec<u8>,
    /// The handling label to attach.
    pub label: LabelSpec,
    /// Caller-chosen envelope id.
    pub envelope_id: String,
}

/// A request to unseal an envelope.
pub struct UnsealRequest {
    /// The trust token presented for authorization.
    pub token: TrustToken,
    /// The envelope to open.
    pub envelope: Envelope,
}

/// A receipt for one seal-service operation. Metadata only — no plaintext.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealReceipt {
    /// `seal` or `unseal`.
    pub op: String,
    /// The envelope acted on.
    pub envelope_id: String,
    /// The presenting token.
    pub token_id: String,
    /// Owning tenant (from the presenting token) — the ledger's tenant predicate
    /// for authorized receipt queries (AF-007).
    #[serde(default)]
    pub tenant_id: String,
    /// Pseudonymous subject from the token.
    pub subject_hash: String,
    /// `allow` or `deny`.
    pub decision: String,
    /// Operation time (RFC3339).
    pub at: String,
}

/// An in-memory BLAKE3 receipt chain. The W4 ledger service persists these; here
/// the service holds them so an operator (and the live test) can verify the
/// unbroken record and see the denials.
#[derive(Debug, Default)]
pub struct ReceiptChain {
    links: Vec<ChainLink>,
    receipts: Vec<SealReceipt>,
    last_hash: [u8; 32],
}

impl ReceiptChain {
    /// A fresh chain anchored at [`GENESIS_HASH`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            links: Vec::new(),
            receipts: Vec::new(),
            last_hash: GENESIS_HASH,
        }
    }

    fn append(&mut self, receipt: SealReceipt) {
        let value = serde_json::to_value(&receipt).expect("flat receipt serializes");
        let entry_hash = canonical_hash(&value).expect("canonical hash of flat receipt");
        self.links.push(ChainLink {
            previous_hash: self.last_hash,
            entry_hash,
        });
        self.last_hash = chain_link(&self.last_hash, &entry_hash);
        self.receipts.push(receipt);
    }

    /// Hex of the current chain head (the `previous_hash` for the next link).
    #[must_use]
    pub fn head_hex(&self) -> String {
        hex::encode(self.last_hash)
    }

    /// The chain links (for [`fabric_proof::verify_chain`]).
    #[must_use]
    pub fn links(&self) -> &[ChainLink] {
        &self.links
    }

    /// The receipts, in order.
    #[must_use]
    pub fn receipts(&self) -> &[SealReceipt] {
        &self.receipts
    }
}

/// Static configuration for a seal service.
#[derive(Debug, Clone)]
pub struct SealServiceConfig {
    /// OpenBao Transit key that wraps per-envelope data keys.
    pub transit_key: String,
    /// Trust-anchor public key used to verify presented trust tokens.
    pub token_public_key: Vec<u8>,
}

/// The seal service.
pub struct SealService {
    openbao: OpenBaoAuth,
    signer: Arc<dyn Signer>,
    config: SealServiceConfig,
    receipts: Arc<Mutex<ReceiptChain>>,
    revocation: Option<RevocationSnapshot>,
}

impl SealService {
    /// Assemble a seal service from an OpenBao client (Transit custody), the
    /// service's own ML-DSA signer (provenance threads), and config.
    #[must_use]
    pub fn new(openbao: OpenBaoAuth, signer: Arc<dyn Signer>, config: SealServiceConfig) -> Self {
        Self {
            openbao,
            signer,
            config,
            receipts: Arc::new(Mutex::new(ReceiptChain::new())),
            revocation: None,
        }
    }

    /// Builder: consult `snapshot` on every seal/unseal so a revoked token (by id,
    /// subject, signing key, or bundle) is refused (AF-006). The snapshot's own
    /// signature is verified against the trust anchor at token-check time.
    #[must_use]
    pub fn with_revocation(mut self, snapshot: RevocationSnapshot) -> Self {
        self.revocation = Some(snapshot);
        self
    }

    /// The service's provenance-signing public key (verifies envelope threads).
    #[must_use]
    pub fn service_public_key(&self) -> &[u8] {
        self.signer.public_key()
    }

    /// A snapshot of the receipt chain links, for verification / ledger ingest.
    #[must_use]
    pub fn receipt_links(&self) -> Vec<ChainLink> {
        self.receipts
            .lock()
            .expect("receipts lock")
            .links()
            .to_vec()
    }

    /// A snapshot of the receipts, in order.
    #[must_use]
    pub fn receipts_snapshot(&self) -> Vec<SealReceipt> {
        self.receipts
            .lock()
            .expect("receipts lock")
            .receipts()
            .to_vec()
    }

    fn record(
        &self,
        op: &str,
        envelope_id: &str,
        token: &TrustToken,
        decision: &str,
        now: DateTime<Utc>,
    ) {
        self.receipts
            .lock()
            .expect("receipts lock")
            .append(SealReceipt {
                op: op.to_string(),
                envelope_id: envelope_id.to_string(),
                token_id: token.token_id.clone(),
                tenant_id: token.tenant_id.clone(),
                subject_hash: token.subject_hash.clone(),
                decision: decision.to_string(),
                at: now.to_rfc3339(),
            });
    }

    fn verify_token(&self, token: &TrustToken, now: DateTime<Utc>) -> Result<(), SealError> {
        // Context-aware verification (AF-006): signature + expiry + the current
        // signed revocation snapshot (token / subject / signing key / bundle).
        let ctx = fabric_token::VerificationContext {
            verifier: &MlDsa87Verifier,
            public_key: &self.config.token_public_key,
            now,
            revocation: self.revocation.as_ref(),
        };
        fabric_token::verify_in_context(token, &ctx)
            .map_err(|e| SealError::Unauthorized(e.to_string()))
    }

    /// Seal a payload into an envelope (Transit-wrapped data key + provenance
    /// thread). Receipts the operation.
    ///
    /// # Errors
    /// [`SealError::Unauthorized`] if the token fails verification; an OpenBao or
    /// envelope error otherwise. A denial is receipted before returning.
    pub async fn seal(&self, req: SealRequest, now: DateTime<Utc>) -> Result<Envelope, SealError> {
        if let Err(e) = self.verify_token(&req.token, now) {
            self.record("seal", &req.envelope_id, &req.token, "deny", now);
            return Err(e);
        }

        let vault_token = self.openbao.login().await?;
        let mut data_key = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut data_key);
        let data_key_wrapped = self
            .openbao
            .transit_encrypt(&vault_token, &self.config.transit_key, &data_key)
            .await?;

        let previous_hash = self.receipts.lock().expect("receipts lock").head_hex();
        let envelope = seal_envelope(
            req.envelope_id.clone(),
            &req.plaintext,
            &data_key,
            data_key_wrapped,
            req.label.into(),
            ThreadSpec {
                authorizing_token_id: req.token.token_id.clone(),
                previous_hash,
                created_at: now.to_rfc3339(),
                // Bind the envelope to the sealing token's tenant + owner (AF-003):
                // only a same-tenant, same-owner token can later unseal it.
                binding: EnvelopeBinding {
                    tenant_id: req.token.tenant_id.clone(),
                    owner_subject_hash: req.token.subject_hash.clone(),
                    audience: String::new(),
                },
            },
            self.signer.as_ref(),
        )?;

        self.record("seal", &envelope.envelope_id, &req.token, "allow", now);
        Ok(envelope)
    }

    /// Unseal an envelope for a token-authorized op. Verifies the token, checks
    /// clearance against the envelope's classification, `transit/decrypt`s the
    /// data key, verifies provenance, then recovers the plaintext. Receipts the
    /// operation (including denials).
    ///
    /// # Errors
    /// [`SealError::Unauthorized`] if the token fails verification, is expired,
    /// lacks clearance, or the label forbids unseal; an OpenBao or envelope error
    /// otherwise. A denial is receipted before returning.
    pub async fn unseal(
        &self,
        req: UnsealRequest,
        now: DateTime<Utc>,
    ) -> Result<Vec<u8>, SealError> {
        let envelope = &req.envelope;
        if let Err(e) = self.verify_token(&req.token, now) {
            self.record("unseal", &envelope.envelope_id, &req.token, "deny", now);
            return Err(e);
        }
        // Clearance: the token must be cleared to at least the payload's classification.
        if req.token.max_data_classification < envelope.label.classification {
            self.record("unseal", &envelope.envelope_id, &req.token, "deny", now);
            return Err(SealError::Unauthorized(
                "token classification below the envelope's".to_string(),
            ));
        }
        // Handling: an explicit op allowlist must include `unseal`.
        if !envelope.label.permitted_ops.is_empty()
            && !envelope.label.permitted_ops.iter().any(|o| o == "unseal")
        {
            self.record("unseal", &envelope.envelope_id, &req.token, "deny", now);
            return Err(SealError::Unauthorized(
                "label does not permit unseal".to_string(),
            ));
        }

        // Tenant/owner binding (AF-003): the presenting token must own the
        // envelope. An unbound (legacy v1) envelope is refused — no silent v1
        // acceptance (E5). Cross-tenant and cross-owner unseal both fail here,
        // before any Transit decrypt.
        let binding = envelope_binding(envelope);
        if binding.is_unbound() {
            self.record("unseal", &envelope.envelope_id, &req.token, "deny", now);
            return Err(SealError::Unauthorized(
                "envelope is not tenant-bound (legacy v1)".to_string(),
            ));
        }
        if binding.tenant_id != req.token.tenant_id
            || binding.owner_subject_hash != req.token.subject_hash
        {
            self.record("unseal", &envelope.envelope_id, &req.token, "deny", now);
            return Err(SealError::Unauthorized(
                "token does not own the envelope (tenant/owner mismatch)".to_string(),
            ));
        }

        let vault_token = self.openbao.login().await?;
        let key_bytes = self
            .openbao
            .transit_decrypt(
                &vault_token,
                &self.config.transit_key,
                &envelope.seal.data_key_wrapped,
            )
            .await?;
        let data_key: [u8; 32] = key_bytes.try_into().map_err(|_| SealError::DataKeySize)?;

        let plaintext = open_envelope(
            envelope,
            &data_key,
            &MlDsa87Verifier,
            self.signer.public_key(),
        )?;
        self.record("unseal", &envelope.envelope_id, &req.token, "allow", now);
        Ok(plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_proof::verify_chain;

    #[test]
    fn receipt_chain_appends_and_verifies() {
        let mut chain = ReceiptChain::new();
        for i in 0..3 {
            chain.append(SealReceipt {
                op: "seal".to_string(),
                envelope_id: format!("env-{i}"),
                token_id: "tok".to_string(),
                tenant_id: "t".to_string(),
                subject_hash: "h".to_string(),
                decision: "allow".to_string(),
                at: "2026-07-03T00:00:00Z".to_string(),
            });
        }
        assert_eq!(chain.receipts().len(), 3);
        verify_chain(chain.links()).expect("chain verifies");
    }

    #[test]
    fn tampered_receipt_chain_fails() {
        let mut chain = ReceiptChain::new();
        chain.append(SealReceipt {
            op: "seal".to_string(),
            envelope_id: "e".to_string(),
            token_id: "t".to_string(),
            tenant_id: "t".to_string(),
            subject_hash: "h".to_string(),
            decision: "allow".to_string(),
            at: "2026-07-03T00:00:00Z".to_string(),
        });
        chain.append(SealReceipt {
            op: "unseal".to_string(),
            envelope_id: "e".to_string(),
            token_id: "t".to_string(),
            tenant_id: "t".to_string(),
            subject_hash: "h".to_string(),
            decision: "allow".to_string(),
            at: "2026-07-03T00:01:00Z".to_string(),
        });
        // Break the link between the two entries.
        chain.links[1].previous_hash = [9u8; 32];
        assert!(verify_chain(chain.links()).is_err());
    }

    #[test]
    fn label_spec_maps_to_contract_label() {
        let spec = LabelSpec {
            classification: Classification::Restricted,
            compliance_scopes: vec![ComplianceScope::Hipaa],
            origin: "ingest".to_string(),
            permitted_ops: vec!["unseal".to_string()],
            permitted_destinations: vec![Route::LocalOnly],
            detected_entities: vec![],
        };
        let label: Label = spec.into();
        assert_eq!(label.classification, Classification::Restricted);
        assert_eq!(label.permitted_ops, vec!["unseal".to_string()]);
    }
}
