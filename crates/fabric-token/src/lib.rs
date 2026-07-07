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
//!
//! [`spend`] (X1) extends the metering contract across replicas: the ledger
//! trait, the single-process ledger, and the lease-based shared ledger that
//! keeps a budget true under horizontal scale.

pub mod spend;

use chrono::{DateTime, Utc};
use fabric_contracts::{RevocationStatus, Signature, TrustToken};
use fabric_crypto::{Signer, Verifier};
use fabric_proof::canonical_hash;
use fabric_revocation::RevocationSnapshot;

/// Maximum attenuation lineage depth. A chain deeper than this fails closed: it
/// is almost certainly a defect or an attempt to build an unbounded lineage.
pub const MAX_ATTENUATION_DEPTH: u32 = 16;

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
    /// The token is expired (context-aware verification).
    #[error("token is expired")]
    Expired,
    /// A timestamp field was not valid RFC3339.
    #[error("timestamp is not valid RFC3339: {0}")]
    BadTimestamp(String),
    /// The parent token presented for attenuation did not verify under the trust
    /// anchor — unsigned, malformed, or signed by the wrong key (signer oracle).
    #[error("parent token failed verification: {0}")]
    ParentUnverified(String),
    /// The parent token was expired at attenuation time.
    #[error("parent token is expired")]
    ParentExpired,
    /// The parent token is revoked (on-token status or the current snapshot).
    #[error("parent token is revoked")]
    ParentRevoked,
    /// A supplied revocation snapshot did not verify under the trust anchor.
    #[error("revocation snapshot failed verification")]
    RevocationSnapshotInvalid,
    /// The token is revoked by the current snapshot on `dimension`.
    #[error("token revoked by snapshot on {dimension}")]
    RevokedBySnapshot {
        /// The snapshot dimension (token / subject / signing_key / bundle_version).
        dimension: &'static str,
    },
    /// Attenuation would exceed [`MAX_ATTENUATION_DEPTH`].
    #[error("attenuation depth exceeds the maximum of {max}")]
    DepthExceeded {
        /// The configured maximum lineage depth.
        max: u32,
    },
    /// The child reuses the parent's token id (a lineage cycle).
    #[error("child token id must differ from the parent's")]
    LineageCycle,
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

/// The trusted inputs a privileged consumer checks a token against, beyond the
/// bare signature: the anchor key, trusted time, and (optionally) the current
/// signed revocation snapshot. Signature-only [`verify`] remains a low-level
/// primitive; privileged paths should use [`verify_in_context`] so a required
/// check (expiry, revocation) cannot be silently omitted at a call site.
pub struct VerificationContext<'a> {
    /// Verifier for the token's signature algorithm.
    pub verifier: &'a dyn Verifier,
    /// Trust-anchor public key the token — and any snapshot — must verify under.
    pub public_key: &'a [u8],
    /// Trusted current time, for expiry.
    pub now: DateTime<Utc>,
    /// Current revocation snapshot, if one is in force. Its own signature is
    /// verified under `public_key`; a snapshot that does not verify fails closed.
    pub revocation: Option<&'a RevocationSnapshot>,
}

impl<'a> VerificationContext<'a> {
    /// A context with no revocation snapshot (signature + expiry only).
    #[must_use]
    pub fn new(verifier: &'a dyn Verifier, public_key: &'a [u8], now: DateTime<Utc>) -> Self {
        Self {
            verifier,
            public_key,
            now,
            revocation: None,
        }
    }

    /// Builder: attach the current revocation snapshot.
    #[must_use]
    pub fn with_revocation(mut self, snapshot: &'a RevocationSnapshot) -> Self {
        self.revocation = Some(snapshot);
        self
    }
}

/// Check `token` against a full [`VerificationContext`]: signature, on-token
/// revocation status, expiry, and — when a snapshot is supplied — signed
/// revocation by token id, subject hash, signing key, or bundle version. Fails
/// closed on the first check that does not pass.
///
/// # Errors
/// [`TokenError`] naming the failed check.
pub fn verify_in_context(
    token: &TrustToken,
    ctx: &VerificationContext<'_>,
) -> Result<(), TokenError> {
    verify(token, ctx.verifier, ctx.public_key)?;
    if is_expired(token, ctx.now)? {
        return Err(TokenError::Expired);
    }
    if let Some(snap) = ctx.revocation {
        check_revocation_snapshot(token, snap, ctx)?;
    }
    Ok(())
}

/// Verify a snapshot under the anchor (a substituted / forged snapshot fails
/// closed) and reject the token if the snapshot revokes it on any dimension.
fn check_revocation_snapshot(
    token: &TrustToken,
    snap: &RevocationSnapshot,
    ctx: &VerificationContext<'_>,
) -> Result<(), TokenError> {
    fabric_revocation::verify(snap, ctx.verifier, ctx.public_key)
        .map_err(|_| TokenError::RevocationSnapshotInvalid)?;
    if snap.is_token_revoked(&token.token_id) {
        return Err(TokenError::RevokedBySnapshot { dimension: "token" });
    }
    if snap.is_subject_revoked(&token.subject_hash) {
        return Err(TokenError::RevokedBySnapshot {
            dimension: "subject",
        });
    }
    if snap.is_key_revoked(&token.signature.key_id) {
        return Err(TokenError::RevokedBySnapshot {
            dimension: "signing_key",
        });
    }
    if snap.is_bundle_revoked(&token.trust_bundle_version) {
        return Err(TokenError::RevokedBySnapshot {
            dimension: "bundle_version",
        });
    }
    Ok(())
}

/// Mint a child that narrows `parent` on **every** authority axis, bind it to the
/// parent (`attenuation.parent_id`, `depth`), and sign it under `signer`.
///
/// Fails closed unless all of the following hold:
///
/// * the **parent authenticates** under `ctx` — a valid anchor signature, not
///   expired, and not revoked (on-token status or the supplied snapshot). This
///   closes the signer-oracle: a fabricated, wrong-key, expired, or revoked
///   parent can no longer mint a signed child;
/// * every **identity axis is unchanged** — tenant, issuer, bundle version,
///   subject, service identity, opaque identity id, and locale;
/// * every **set axis is a subset** — roles, compliance scopes, routes, models;
/// * every **scalar ceiling narrows** — classification, budget caps (each fits
///   the parent's remaining), expiry (the child cannot outlive the parent), and
///   `offline_mode` (an offline-only parent cannot mint an online child); and
/// * the **lineage is bounded** — the child id differs from the parent's and the
///   depth stays within [`MAX_ATTENUATION_DEPTH`].
///
/// `ctx.public_key` is the key the **parent** verifies under; `signer` signs the
/// **child** (they may differ — e.g. a kernel re-anchors a WSF-issued parent).
///
/// # Errors
/// [`TokenError::ParentUnverified`] / [`ParentExpired`](TokenError::ParentExpired)
/// / [`ParentRevoked`](TokenError::ParentRevoked) if the parent fails to
/// authenticate; [`TokenError::AttenuationWidens`] on any widening;
/// [`TokenError::DepthExceeded`] / [`LineageCycle`](TokenError::LineageCycle) on
/// a bad lineage; or a signing error.
pub fn attenuate(
    parent: &TrustToken,
    mut child: TrustToken,
    ctx: &VerificationContext<'_>,
    signer: &dyn Signer,
) -> Result<TrustToken, TokenError> {
    // 1. Authenticate the parent before it can mint anything.
    if parent.revocation_status == RevocationStatus::Revoked {
        return Err(TokenError::ParentRevoked);
    }
    verify(parent, ctx.verifier, ctx.public_key)
        .map_err(|e| TokenError::ParentUnverified(e.to_string()))?;
    if is_expired(parent, ctx.now)? {
        return Err(TokenError::ParentExpired);
    }
    if let Some(snap) = ctx.revocation {
        fabric_revocation::verify(snap, ctx.verifier, ctx.public_key)
            .map_err(|_| TokenError::RevocationSnapshotInvalid)?;
        if snap.is_token_revoked(&parent.token_id)
            || snap.is_subject_revoked(&parent.subject_hash)
            || snap.is_key_revoked(&parent.signature.key_id)
            || snap.is_bundle_revoked(&parent.trust_bundle_version)
        {
            return Err(TokenError::ParentRevoked);
        }
    }

    // 2. Every authority axis must narrow (or stay equal); never widen.
    check_monotonic(parent, &child)?;
    let p_exp = DateTime::parse_from_rfc3339(&parent.expires_at)
        .map_err(|_| TokenError::BadTimestamp(parent.expires_at.clone()))?;
    let c_exp = DateTime::parse_from_rfc3339(&child.expires_at)
        .map_err(|_| TokenError::BadTimestamp(child.expires_at.clone()))?;
    if c_exp > p_exp {
        return Err(TokenError::AttenuationWidens { axis: "expires_at" });
    }

    // 3. Bound the lineage, bind to the parent, and sign the child.
    if child.token_id == parent.token_id {
        return Err(TokenError::LineageCycle);
    }
    let depth = parent.attenuation.depth.saturating_add(1);
    if depth > MAX_ATTENUATION_DEPTH {
        return Err(TokenError::DepthExceeded {
            max: MAX_ATTENUATION_DEPTH,
        });
    }
    child.attenuation.parent_id = Some(parent.token_id.clone());
    child.attenuation.depth = depth;
    issue(child, signer)
}

/// True if every element of `child` is present in `parent`.
fn is_subset<T: PartialEq>(child: &[T], parent: &[T]) -> bool {
    child.iter().all(|c| parent.contains(c))
}

/// Reject `child` if it widens `parent` on any authority axis other than expiry
/// (checked by the caller, which needs the parsed timestamps).
fn check_monotonic(parent: &TrustToken, child: &TrustToken) -> Result<(), TokenError> {
    // Identity axes — equality; a child may not change who/what it speaks for.
    let identity: [(&'static str, bool); 9] = [
        ("tenant_id", parent.tenant_id == child.tenant_id),
        ("issuer", parent.issuer == child.issuer),
        (
            "trust_bundle_version",
            parent.trust_bundle_version == child.trust_bundle_version,
        ),
        ("subject_hash", parent.subject_hash == child.subject_hash),
        ("subject_id", parent.subject_id == child.subject_id),
        (
            "service_identity",
            parent.service_identity == child.service_identity,
        ),
        ("identity_id", parent.identity_id == child.identity_id),
        ("country", parent.country == child.country),
        ("person_type", parent.person_type == child.person_type),
    ];
    for (axis, unchanged) in identity {
        if !unchanged {
            return Err(TokenError::AttenuationWidens { axis });
        }
    }
    // Set axes — the child must be a subset of the parent.
    if !is_subset(&child.roles, &parent.roles) {
        return Err(TokenError::AttenuationWidens { axis: "roles" });
    }
    if !is_subset(&child.compliance_scopes, &parent.compliance_scopes) {
        return Err(TokenError::AttenuationWidens {
            axis: "compliance_scopes",
        });
    }
    if !is_subset(&child.allowed_routes, &parent.allowed_routes) {
        return Err(TokenError::AttenuationWidens {
            axis: "allowed_routes",
        });
    }
    // An empty parent model list means "unrestricted at this layer"; a non-empty
    // list is a ceiling the child must stay within.
    if !parent.allowed_models.is_empty()
        && !is_subset(&child.allowed_models, &parent.allowed_models)
    {
        return Err(TokenError::AttenuationWidens {
            axis: "allowed_models",
        });
    }
    // Scalar ceilings.
    if child.max_data_classification > parent.max_data_classification {
        return Err(TokenError::AttenuationWidens {
            axis: "max_data_classification",
        });
    }
    // offline_mode is a restriction: an offline-only parent may not mint an
    // online-capable child.
    if parent.offline_mode && !child.offline_mode {
        return Err(TokenError::AttenuationWidens {
            axis: "offline_mode",
        });
    }
    // Budget: each cap must fit the parent's remaining headroom.
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
    Ok(())
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
