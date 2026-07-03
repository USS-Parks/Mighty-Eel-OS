//! `fabric-token` — the WSF trust-token primitive.
//!
//! A trust token (`fabric_contracts::TrustToken`) is signed over its canonical
//! payload — every field **except** `signature` — via `fabric-crypto`, and
//! chained to a parent by attenuation. This crate owns the four operations:
//!
//!   * [`issue`] — sign a token.
//!   * [`verify`] — check signature + revocation (expiry via [`is_expired`]).
//!   * [`attenuate`] — mint a child that narrows the parent on every axis;
//!     fails closed if it widens any.
//!   * [`try_spend`] — atomically meter the budget strand.

use chrono::{DateTime, Utc};
use fabric_contracts::{RevocationStatus, Signature, TrustToken};
use fabric_crypto::{Signer, Verifier};
use fabric_proof::canonical_hash;

/// Failures from the trust-token operations.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TokenError {
    /// The token could not be serialized for hashing.
    #[error("canonical serialization failed: {0}")]
    Serialize(String),
    /// The signer failed.
    #[error("signing failed: {0}")]
    Sign(String),
    /// The signature string was not valid hex.
    #[error("signature is not valid hex")]
    MalformedSignature,
    /// The signature did not verify against the public key.
    #[error("signature failed verification")]
    InvalidSignature,
    /// The token's revocation status is `revoked`.
    #[error("token is revoked")]
    Revoked,
    /// A timestamp field was not valid RFC3339.
    #[error("timestamp is not valid RFC3339: {0}")]
    BadTimestamp(String),
    /// A child attenuation widened the parent on `axis`.
    #[error("attenuation widens the parent on {axis}")]
    AttenuationWidens {
        /// The axis (routes / models / classification / budget / expiry) widened.
        axis: &'static str,
    },
    /// A spend would exceed the budget `counter`.
    #[error("budget exceeded on {counter}")]
    BudgetExceeded {
        /// The counter (tokens / usd / tool_calls) that would overflow its cap.
        counter: &'static str,
    },
}

/// BLAKE3-32 over the canonical payload (signature field removed).
fn signing_hash(token: &TrustToken) -> Result<[u8; 32], TokenError> {
    let mut v = serde_json::to_value(token).map_err(|e| TokenError::Serialize(e.to_string()))?;
    if let Some(obj) = v.as_object_mut() {
        obj.remove("signature");
    }
    canonical_hash(&v).map_err(|e| TokenError::Serialize(e.to_string()))
}

/// Sign `token` over its canonical payload, returning the signed token.
///
/// # Errors
/// Returns [`TokenError`] if serialization or signing fails.
pub fn issue(mut token: TrustToken, signer: &dyn Signer) -> Result<TrustToken, TokenError> {
    token.signature = Signature {
        alg: signer.algorithm().to_string(),
        key_id: signer.key_id().to_string(),
        value: String::new(),
    };
    let hash = signing_hash(&token)?;
    let sig = signer
        .sign(&hash)
        .map_err(|e| TokenError::Sign(e.to_string()))?;
    token.signature.value = hex::encode(sig);
    Ok(token)
}

/// Verify a token's signature and revocation status. Expiry is checked
/// separately with [`is_expired`] (it needs the caller's clock).
///
/// # Errors
/// Returns [`TokenError::Revoked`], [`TokenError::MalformedSignature`], or
/// [`TokenError::InvalidSignature`].
pub fn verify(
    token: &TrustToken,
    verifier: &dyn Verifier,
    public_key: &[u8],
) -> Result<(), TokenError> {
    if token.revocation_status == RevocationStatus::Revoked {
        return Err(TokenError::Revoked);
    }
    let hash = signing_hash(token)?;
    let sig = hex::decode(&token.signature.value).map_err(|_| TokenError::MalformedSignature)?;
    match verifier.verify(&hash, &sig, public_key) {
        Ok(true) => Ok(()),
        _ => Err(TokenError::InvalidSignature),
    }
}

/// True if `token.expires_at` is at or before `now`.
///
/// # Errors
/// Returns [`TokenError::BadTimestamp`] if `expires_at` is not RFC3339.
pub fn is_expired(token: &TrustToken, now: DateTime<Utc>) -> Result<bool, TokenError> {
    let exp = DateTime::parse_from_rfc3339(&token.expires_at)
        .map_err(|_| TokenError::BadTimestamp(token.expires_at.clone()))?
        .with_timezone(&Utc);
    Ok(exp <= now)
}

/// Validate that `child` narrows `parent` on every axis, bind it to the parent
/// (`attenuation.parent_id`), and sign it. Fails closed if the child widens any
/// axis: routes/models must be subsets, classification must not exceed the
/// parent ceiling, each budget cap must fit the parent's remaining, and the
/// child must not outlive the parent.
///
/// # Errors
/// Returns [`TokenError::AttenuationWidens`] on any widening, or a signing error.
pub fn attenuate(
    parent: &TrustToken,
    mut child: TrustToken,
    signer: &dyn Signer,
) -> Result<TrustToken, TokenError> {
    if !child
        .allowed_routes
        .iter()
        .all(|r| parent.allowed_routes.contains(r))
    {
        return Err(TokenError::AttenuationWidens {
            axis: "allowed_routes",
        });
    }
    if !parent.allowed_models.is_empty()
        && !child
            .allowed_models
            .iter()
            .all(|m| parent.allowed_models.contains(m))
    {
        return Err(TokenError::AttenuationWidens {
            axis: "allowed_models",
        });
    }
    if child.max_data_classification > parent.max_data_classification {
        return Err(TokenError::AttenuationWidens {
            axis: "max_data_classification",
        });
    }
    if let Some(pb) = &parent.budget {
        let cb = child
            .budget
            .as_ref()
            .ok_or(TokenError::AttenuationWidens { axis: "budget" })?;
        if cb.token_cap > pb.token_cap.saturating_sub(pb.tokens_spent)
            || cb.usd_cap_cents > pb.usd_cap_cents.saturating_sub(pb.usd_spent_cents)
            || cb.tool_call_cap > pb.tool_call_cap.saturating_sub(pb.tool_calls_spent)
        {
            return Err(TokenError::AttenuationWidens { axis: "budget" });
        }
    }
    let p_exp = DateTime::parse_from_rfc3339(&parent.expires_at)
        .map_err(|_| TokenError::BadTimestamp(parent.expires_at.clone()))?;
    let c_exp = DateTime::parse_from_rfc3339(&child.expires_at)
        .map_err(|_| TokenError::BadTimestamp(child.expires_at.clone()))?;
    if c_exp > p_exp {
        return Err(TokenError::AttenuationWidens { axis: "expires_at" });
    }
    child.attenuation.parent_id = Some(parent.token_id.clone());
    issue(child, signer)
}

/// Atomically meter the budget strand. No-op (always `Ok`) when the token has no
/// budget (legacy-claim compatibility). Otherwise checks every counter against
/// its cap and, only if all fit, commits the spend.
///
/// # Errors
/// Returns [`TokenError::BudgetExceeded`] naming the first counter that would
/// exceed its cap; the token is left unchanged.
pub fn try_spend(
    token: &mut TrustToken,
    tokens: u64,
    usd_cents: u64,
    tool_calls: u32,
) -> Result<(), TokenError> {
    let Some(b) = token.budget.as_mut() else {
        return Ok(());
    };
    let new_tokens = b
        .tokens_spent
        .checked_add(tokens)
        .filter(|t| *t <= b.token_cap)
        .ok_or(TokenError::BudgetExceeded { counter: "tokens" })?;
    let new_usd = b
        .usd_spent_cents
        .checked_add(usd_cents)
        .filter(|u| *u <= b.usd_cap_cents)
        .ok_or(TokenError::BudgetExceeded { counter: "usd" })?;
    let new_calls = b
        .tool_calls_spent
        .checked_add(tool_calls)
        .filter(|c| *c <= b.tool_call_cap)
        .ok_or(TokenError::BudgetExceeded {
            counter: "tool_calls",
        })?;
    b.tokens_spent = new_tokens;
    b.usd_spent_cents = new_usd;
    b.tool_calls_spent = new_calls;
    Ok(())
}
