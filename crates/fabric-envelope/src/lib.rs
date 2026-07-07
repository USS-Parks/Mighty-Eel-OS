//! `fabric-envelope` — the WSF sealed envelope. Regulated data never travels
//! naked; it moves inside three wraps:
//!
//!   * **seal** — AES-256-GCM ciphertext under a per-envelope data key. The
//!     data key is normally wrapped by OpenBao transit (Phase W); the
//!     `data_key_wrapped` field carries that opaque reference.
//!   * **label** — classification + handling rules, readable **without**
//!     unsealing (it is plaintext in the envelope) and **AAD-bound** to the
//!     ciphertext, so altering the label breaks decryption. This is what AOG
//!     reads for DSPM-informed routing.
//!   * **thread** — an ML-DSA signature (via `fabric-crypto`) over the sealed
//!     payload + label + authorizing token + chain link, so provenance is
//!     tamper-evident.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use fabric_contracts::{Envelope, Label, Seal, Signature, Thread};
use fabric_crypto::{Signer, Verifier};

/// Failures from envelope operations.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EnvelopeError {
    /// AEAD cipher initialization failed (bad key length).
    #[error("crypto init failed: {0}")]
    Crypto(String),
    /// Encryption failed.
    #[error("seal (encrypt) failed")]
    SealFailed,
    /// Decryption or authentication failed (wrong key, tampered ciphertext).
    #[error("unseal (decrypt/authentication) failed")]
    UnsealFailed,
    /// The seal's `aad_hash` does not match the label — the label was altered.
    #[error("AAD hash does not match the label")]
    AadMismatch,
    /// An envelope field (nonce, ciphertext, hash) was malformed.
    #[error("malformed envelope field")]
    Malformed,
    /// Canonical serialization failed.
    #[error("canonical serialization failed: {0}")]
    Serialize(String),
    /// The thread signer failed.
    #[error("signing failed: {0}")]
    Sign(String),
    /// The envelope's thread carries no signature.
    #[error("envelope thread has no signature")]
    NoSignature,
    /// The thread signature failed verification.
    #[error("thread signature failed verification")]
    InvalidSignature,
}

fn aad_hash(aad: &[u8]) -> String {
    format!("blake3:{}", hex::encode(blake3::hash(aad).as_bytes()))
}

/// Seal `plaintext` under `data_key` (32 bytes), binding `aad` (the canonical
/// label bytes) into the AEAD so the label cannot be altered without breaking
/// decryption.
///
/// # Errors
/// Returns [`EnvelopeError`] if the key is the wrong length or encryption fails.
pub fn seal(
    plaintext: &[u8],
    data_key: &[u8; 32],
    data_key_wrapped: impl Into<String>,
    aad: &[u8],
) -> Result<Seal, EnvelopeError> {
    let cipher =
        Aes256Gcm::new_from_slice(data_key).map_err(|e| EnvelopeError::Crypto(e.to_string()))?;
    let mut nonce_bytes = [0u8; 12];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| EnvelopeError::SealFailed)?;
    Ok(Seal {
        aead_alg: "AES-256-GCM".to_string(),
        data_key_wrapped: data_key_wrapped.into(),
        nonce: hex::encode(nonce_bytes),
        ciphertext: hex::encode(&ciphertext),
        aad_hash: aad_hash(aad),
    })
}

/// Unseal a [`Seal`] with `data_key` and the same `aad` used to seal it.
///
/// # Errors
/// Returns [`EnvelopeError::AadMismatch`] if the label changed,
/// [`EnvelopeError::Malformed`] on bad encoding, or
/// [`EnvelopeError::UnsealFailed`] on wrong key / tampered ciphertext.
pub fn unseal(seal: &Seal, data_key: &[u8; 32], aad: &[u8]) -> Result<Vec<u8>, EnvelopeError> {
    if seal.aad_hash != aad_hash(aad) {
        return Err(EnvelopeError::AadMismatch);
    }
    let cipher =
        Aes256Gcm::new_from_slice(data_key).map_err(|e| EnvelopeError::Crypto(e.to_string()))?;
    let nonce_bytes = hex::decode(&seal.nonce).map_err(|_| EnvelopeError::Malformed)?;
    let ct = hex::decode(&seal.ciphertext).map_err(|_| EnvelopeError::Malformed)?;
    if nonce_bytes.len() != 12 {
        return Err(EnvelopeError::Malformed);
    }
    let nonce = Nonce::from_slice(&nonce_bytes);
    cipher
        .decrypt(nonce, Payload { msg: &ct, aad })
        .map_err(|_| EnvelopeError::UnsealFailed)
}

/// The tenant / owner / audience an envelope is **bound** to (v2). All-empty
/// means an unbound legacy (v1) envelope; production unseal refuses one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvelopeBinding {
    /// Owning tenant.
    pub tenant_id: String,
    /// Owner subject hash.
    pub owner_subject_hash: String,
    /// Intended audience.
    pub audience: String,
}

impl EnvelopeBinding {
    /// True when no binding field is set (a legacy v1 envelope).
    #[must_use]
    pub fn is_unbound(&self) -> bool {
        self.tenant_id.is_empty() && self.owner_subject_hash.is_empty() && self.audience.is_empty()
    }
}

/// The AEAD AAD: the handling label **and** the tenant/owner/audience binding, so
/// altering either the label or the binding breaks decryption.
fn envelope_aad(label: &Label, binding: &EnvelopeBinding) -> Result<Vec<u8>, EnvelopeError> {
    let v = serde_json::json!({
        "label": label,
        "tenant_id": binding.tenant_id,
        "owner_subject_hash": binding.owner_subject_hash,
        "audience": binding.audience,
    });
    fabric_proof::canonical_bytes(&v).map_err(|e| EnvelopeError::Serialize(e.to_string()))
}

/// The canonical content the thread signs: everything but the signatures.
fn thread_content(
    envelope_id: &str,
    seal: &Seal,
    label: &Label,
    authorizing_token_id: &str,
    previous_hash: &str,
    created_at: &str,
    binding: &EnvelopeBinding,
) -> serde_json::Value {
    serde_json::json!({
        "envelope_id": envelope_id,
        "seal": seal,
        "label": label,
        "authorizing_token_id": authorizing_token_id,
        "previous_hash": previous_hash,
        "created_at": created_at,
        "tenant_id": binding.tenant_id,
        "owner_subject_hash": binding.owner_subject_hash,
        "audience": binding.audience,
    })
}

/// Fields for building an envelope's provenance thread.
pub struct ThreadSpec {
    /// The trust token that authorized creating this envelope.
    pub authorizing_token_id: String,
    /// The prior chain hash this envelope extends, hex-encoded.
    pub previous_hash: String,
    /// Creation time (RFC3339).
    pub created_at: String,
    /// The tenant / owner / audience this envelope is bound to (v2). Leave empty
    /// only for a deliberately unbound envelope.
    pub binding: EnvelopeBinding,
}

/// Seal `plaintext` into a full [`Envelope`]: AEAD-seal the payload (label bound
/// as AAD), attach the `label`, and sign the provenance `thread`.
///
/// # Errors
/// Returns [`EnvelopeError`] on serialization, encryption, or signing failure.
pub fn seal_envelope(
    envelope_id: impl Into<String>,
    plaintext: &[u8],
    data_key: &[u8; 32],
    data_key_wrapped: impl Into<String>,
    label: Label,
    thread: ThreadSpec,
    signer: &dyn Signer,
) -> Result<Envelope, EnvelopeError> {
    let envelope_id = envelope_id.into();
    // AAD binds the label AND the tenant/owner/audience: tampering with either
    // breaks decryption (AF-003).
    let aad = envelope_aad(&label, &thread.binding)?;
    let seal = seal(plaintext, data_key, data_key_wrapped, &aad)?;

    let content = thread_content(
        &envelope_id,
        &seal,
        &label,
        &thread.authorizing_token_id,
        &thread.previous_hash,
        &thread.created_at,
        &thread.binding,
    );
    let hash = fabric_proof::canonical_hash(&content)
        .map_err(|e| EnvelopeError::Serialize(e.to_string()))?;
    let sig = signer
        .sign(&hash)
        .map_err(|e| EnvelopeError::Sign(e.to_string()))?;

    Ok(Envelope {
        envelope_id,
        seal,
        label,
        thread: Thread {
            created_at: thread.created_at,
            authorizing_token_id: thread.authorizing_token_id,
            previous_hash: thread.previous_hash,
            signatures: vec![Signature {
                alg: signer.algorithm().to_string(),
                key_id: signer.key_id().to_string(),
                value: hex::encode(sig),
            }],
            tenant_id: thread.binding.tenant_id,
            owner_subject_hash: thread.binding.owner_subject_hash,
            audience: thread.binding.audience,
        },
    })
}

/// The tenant / owner / audience an [`Envelope`] is bound to (read from its
/// thread), for the unseal-authorization check.
#[must_use]
pub fn envelope_binding(envelope: &Envelope) -> EnvelopeBinding {
    EnvelopeBinding {
        tenant_id: envelope.thread.tenant_id.clone(),
        owner_subject_hash: envelope.thread.owner_subject_hash.clone(),
        audience: envelope.thread.audience.clone(),
    }
}

/// Read an envelope's label **without** unsealing the payload. This is the
/// property AOG relies on for DSPM-informed routing.
#[must_use]
pub fn read_label(envelope: &Envelope) -> &Label {
    &envelope.label
}

/// Verify an envelope's provenance thread signature (binds seal + label +
/// authorizing token + chain link).
///
/// # Errors
/// Returns [`EnvelopeError::NoSignature`], [`EnvelopeError::Malformed`], or
/// [`EnvelopeError::InvalidSignature`].
pub fn verify_thread(
    envelope: &Envelope,
    verifier: &dyn Verifier,
    public_key: &[u8],
) -> Result<(), EnvelopeError> {
    let content = thread_content(
        &envelope.envelope_id,
        &envelope.seal,
        &envelope.label,
        &envelope.thread.authorizing_token_id,
        &envelope.thread.previous_hash,
        &envelope.thread.created_at,
        &envelope_binding(envelope),
    );
    let hash = fabric_proof::canonical_hash(&content)
        .map_err(|e| EnvelopeError::Serialize(e.to_string()))?;
    let sig = envelope
        .thread
        .signatures
        .first()
        .ok_or(EnvelopeError::NoSignature)?;
    let sig_bytes = hex::decode(&sig.value).map_err(|_| EnvelopeError::Malformed)?;
    match verifier.verify(&hash, &sig_bytes, public_key) {
        Ok(true) => Ok(()),
        _ => Err(EnvelopeError::InvalidSignature),
    }
}

/// Verify the thread, then unseal the payload with `data_key`. The whole
/// contract: provenance is checked before any plaintext is recovered.
///
/// # Errors
/// Returns [`EnvelopeError`] if thread verification or unsealing fails.
pub fn open_envelope(
    envelope: &Envelope,
    data_key: &[u8; 32],
    verifier: &dyn Verifier,
    public_key: &[u8],
) -> Result<Vec<u8>, EnvelopeError> {
    verify_thread(envelope, verifier, public_key)?;
    let aad = envelope_aad(&envelope.label, &envelope_binding(envelope))?;
    unseal(&envelope.seal, data_key, &aad)
}
