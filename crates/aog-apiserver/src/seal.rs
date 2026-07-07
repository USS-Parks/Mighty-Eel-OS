//! K8 — the mutate stage's seal + attenuate.
//!
//! Two things admission does to a mutation before it lands, both needing
//! control-plane key material:
//!
//!  * **Envelope-seal flagged spec fields** (F4/F6). A designated sensitive field
//!    (`TrustRing.transit_key`, `ToolGrant.credential_ref`) is AES-256-GCM sealed
//!    via `fabric-envelope`; the plaintext is replaced by a placeholder and the
//!    sealed blob stashed in a metadata annotation. The control-plane truth store
//!    never holds the plaintext (addendum A1.3.8 / doctrine I-2).
//!  * **Attenuate a child token** scoped to exactly this action
//!    (`fabric-token::attenuate`). The child narrows the caller's token — never
//!    widens (attenuate fails closed) — so the object is authorized by a
//!    capability scoped to its own creation, not the broad parent (I-1/I-3).
//!
//! The seal data key and signer are the kernel's local placeholders; a
//! production estate custodies both in OpenBao (transit-wrapped data keys,
//! Phase W). Stable across restart so sealed state stays openable.

use std::collections::BTreeMap;

use aog_estate::ResourceObject;
use chrono::Utc;
use fabric_contracts::{Attenuation, Budget, Classification, Seal, TrustToken};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_token::{TokenError, VerificationContext};

use crate::error::ApiError;

/// Placeholder left in a sealed field; the ciphertext lives in the annotation.
pub const SEALED_PLACEHOLDER: &str = "sealed:wsf-envelope";
/// Wrapped-data-key reference (OpenBao transit wraps the real key in Phase W).
const DATA_KEY_REF: &str = "local:kernel-seal";
/// Annotation-key prefix under which a sealed field's envelope is stored.
const SEALED_ANNOTATION_PREFIX: &str = "wsf.io/sealed.";
/// Kernel placeholder data key. NOT a production secret — Phase W replaces it
/// with an OpenBao-transit-wrapped key; it is fixed so sealed state survives a
/// restart.
const KERNEL_DATA_KEY: [u8; 32] = [0x5a; 32];

/// Control-plane key material for the mutate stage: the field-seal data key, the
/// signer that mints attenuated child tokens, and the WSF trust-anchor public key
/// that inbound parent tokens verify under (wired at assembly).
pub struct Sealer {
    data_key: [u8; 32],
    signer: RustCryptoMlDsa87,
    anchor_public_key: Option<Vec<u8>>,
}

impl Sealer {
    /// Build a sealer from an explicit data key + signer. The trust anchor is
    /// wired separately via [`set_anchor_public_key`](Sealer::set_anchor_public_key).
    #[must_use]
    pub fn new(data_key: [u8; 32], signer: RustCryptoMlDsa87) -> Self {
        Self {
            data_key,
            signer,
            anchor_public_key: None,
        }
    }

    /// The kernel default: the fixed placeholder data key + a fresh signer.
    ///
    /// # Errors
    /// [`ApiError::Store`] if signer generation fails.
    pub fn generate() -> Result<Self, ApiError> {
        let signer = RustCryptoMlDsa87::generate("aog-apiserver-cp")
            .map_err(|e| ApiError::Store(e.to_string()))?;
        Ok(Self {
            data_key: KERNEL_DATA_KEY,
            signer,
            anchor_public_key: None,
        })
    }

    /// Wire the WSF trust-anchor public key that inbound parent tokens verify
    /// under, so [`mint_child`](Sealer::mint_child) authenticates the parent
    /// before attenuating it. The API server sets this at assembly
    /// (`AppState::from_raft`) from the front-door authenticator's anchor.
    pub fn set_anchor_public_key(&mut self, anchor_public_key: Vec<u8>) {
        self.anchor_public_key = Some(anchor_public_key);
    }

    /// The public key that verifies the child tokens this sealer mints.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        self.signer.public_key()
    }

    /// Envelope-seal a resource's flagged sensitive fields in place. Idempotent:
    /// an already-sealed field (placeholder present) is left alone.
    ///
    /// # Errors
    /// [`ApiError::Store`] if AEAD sealing fails.
    pub fn seal_fields(&self, object: &mut ResourceObject) -> Result<(), ApiError> {
        match object {
            ResourceObject::TrustRing(r) if r.spec.transit_key != SEALED_PLACEHOLDER => {
                let seal = self.seal_value("transit_key", r.spec.transit_key.as_bytes())?;
                stash(&mut r.metadata.annotations, "transit_key", &seal);
                SEALED_PLACEHOLDER.clone_into(&mut r.spec.transit_key);
            }
            ResourceObject::ToolGrant(r) => {
                if let Some(cred) = r.spec.credential_ref.clone()
                    && cred != SEALED_PLACEHOLDER
                {
                    let seal = self.seal_value("credential_ref", cred.as_bytes())?;
                    stash(&mut r.metadata.annotations, "credential_ref", &seal);
                    r.spec.credential_ref = Some(SEALED_PLACEHOLDER.to_owned());
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn seal_value(&self, field: &str, plaintext: &[u8]) -> Result<Seal, ApiError> {
        // aad binds the ciphertext to the field identity.
        fabric_envelope::seal(plaintext, &self.data_key, DATA_KEY_REF, field.as_bytes())
            .map_err(|e| ApiError::Store(e.to_string()))
    }

    /// Mint a child token that narrows `parent` to this action's scope (its
    /// classification `ceiling`), bound to the parent by attenuation. A strict
    /// subset — `fabric-token::attenuate` fails closed on any widen.
    ///
    /// # Errors
    /// [`ApiError::Store`] if attenuation or signing fails.
    pub fn mint_child(
        &self,
        parent: &TrustToken,
        ceiling: Option<Classification>,
        action: &str,
    ) -> Result<TrustToken, ApiError> {
        // The parent must authenticate under the WSF anchor before it mints a
        // child (AF-001). Fail closed if the anchor was never wired.
        let anchor = self
            .anchor_public_key
            .as_deref()
            .ok_or_else(|| ApiError::Store("attenuation anchor key not configured".to_string()))?;
        scoped_child_token(
            parent,
            ceiling,
            &format!("child:{action}"),
            anchor,
            &self.signer,
        )
        .map_err(|e| ApiError::Store(e.to_string()))
    }
}

fn stash(annotations: &mut BTreeMap<String, String>, field: &str, seal: &Seal) {
    if let Ok(json) = serde_json::to_string(seal) {
        annotations.insert(format!("{SEALED_ANNOTATION_PREFIX}{field}"), json);
    }
}

/// Build a child token narrowing `parent` to `ceiling` (and no more), then
/// attenuate + sign it under `signer`. `parent_public_key` is the trust anchor
/// the parent must verify under — `fabric_token::attenuate` authenticates the
/// parent (rejecting a forged / wrong-key / expired / revoked one) before it
/// signs any child. The seam the K8 tests inspect.
///
/// # Errors
/// [`TokenError`] if the parent fails to authenticate or the (built-to-be-subset)
/// child fails attenuation.
pub fn scoped_child_token(
    parent: &TrustToken,
    ceiling: Option<Classification>,
    child_id: &str,
    parent_public_key: &[u8],
    signer: &dyn Signer,
) -> Result<TrustToken, TokenError> {
    let mut child = parent.clone();
    child_id.clone_into(&mut child.token_id);
    child.attenuation = Attenuation::default();
    if let Some(c) = ceiling {
        child.max_data_classification = c.min(parent.max_data_classification);
    }
    if let Some(pb) = &parent.budget {
        child.budget = Some(Budget {
            token_cap: pb.token_cap.saturating_sub(pb.tokens_spent),
            usd_cap_cents: pb.usd_cap_cents.saturating_sub(pb.usd_spent_cents),
            tool_call_cap: pb.tool_call_cap.saturating_sub(pb.tool_calls_spent),
            tokens_spent: 0,
            usd_spent_cents: 0,
            tool_calls_spent: 0,
        });
    }
    let ctx = VerificationContext::new(&MlDsa87Verifier, parent_public_key, Utc::now());
    fabric_token::attenuate(parent, child, &ctx, signer)
}
