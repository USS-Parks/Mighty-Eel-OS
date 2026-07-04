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
use fabric_contracts::{Attenuation, Budget, Classification, Seal, TrustToken};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use fabric_token::TokenError;

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

/// Control-plane key material for the mutate stage: the field-seal data key and
/// the signer that mints attenuated child tokens.
pub struct Sealer {
    data_key: [u8; 32],
    signer: RustCryptoMlDsa87,
}

impl Sealer {
    /// Build a sealer from an explicit data key + signer.
    #[must_use]
    pub fn new(data_key: [u8; 32], signer: RustCryptoMlDsa87) -> Self {
        Self { data_key, signer }
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
        })
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
        scoped_child_token(parent, ceiling, &format!("child:{action}"), &self.signer)
            .map_err(|e| ApiError::Store(e.to_string()))
    }
}

fn stash(annotations: &mut BTreeMap<String, String>, field: &str, seal: &Seal) {
    if let Ok(json) = serde_json::to_string(seal) {
        annotations.insert(format!("{SEALED_ANNOTATION_PREFIX}{field}"), json);
    }
}

/// Build a child token narrowing `parent` to `ceiling` (and no more), then
/// attenuate + sign it under `signer`. Pure; the seam the K8 tests inspect.
///
/// # Errors
/// [`TokenError`] if the (built-to-be-subset) child fails attenuation.
pub fn scoped_child_token(
    parent: &TrustToken,
    ceiling: Option<Classification>,
    child_id: &str,
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
    fabric_token::attenuate(parent, child, signer)
}
