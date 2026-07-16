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
use fabric_contracts::{
    Budget, Classification, ComplianceScope, RevocationStatus, Route, Signature, TrustToken,
};
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
    /// The token is expired (`now >= expires_at`).
    #[error("token is expired")]
    Expired,
    /// The token is not yet valid (`issued_at > now`).
    #[error("token is not yet valid")]
    NotYetValid,
    /// The token's tenant does not match the required context.
    #[error("tenant mismatch: token is not bound to the required tenant")]
    TenantMismatch,
    /// The token's trust-bundle version does not match the required context.
    #[error("trust-bundle version mismatch")]
    BundleMismatch,
    /// The token's revocation status is `unknown` where a fresh status is required.
    #[error("revocation status is unknown (fail-closed)")]
    RevocationUnknown,
    /// The child token id is empty or collides with the parent (trivial cycle).
    #[error("invalid child token id: empty or equal to the parent")]
    InvalidChildId,
    /// The attenuation would exceed the maximum delegation depth.
    #[error("attenuation exceeds maximum delegation depth")]
    DepthExceeded,
    /// A legacy (v1) token was presented where the current bundle is required
    /// and legacy migration was not permitted (plan T6 deny-by-default).
    #[error("unsupported legacy token version (bundle {0})")]
    UnsupportedTokenVersion(String),
    /// A legacy (v1) token may be verified under a bounded migration flag but is
    /// never a valid attenuation parent (plan T6 — no v1 attenuation).
    #[error("legacy token may not be attenuated (bundle {0})")]
    LegacyAttenuationDenied(String),
}

/// BLAKE3-32 over the canonical payload. Only the signature **bytes**
/// (`signature.value`) are excluded — `signature.alg` and `signature.key_id`
/// are signed, so the signature binds its own declared algorithm and key
/// identity (JWS-style). A swapped `key_id`/`alg` then invalidates the token
/// rather than riding along unsigned.
fn signing_hash(token: &TrustToken) -> Result<[u8; 32], TokenError> {
    let mut v = serde_json::to_value(token).map_err(|e| TokenError::Serialize(e.to_string()))?;
    // Strip only the signature bytes (absent at signing time); keep alg + key_id
    // in the signed payload.
    if let Some(sig) = v.get_mut("signature").and_then(|s| s.as_object_mut()) {
        sig.remove("value");
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

/// The shared budget-metering key for a token's attenuation lineage (plan T5).
///
/// Siblings and nested descendants draw from a *single* shared spend
/// counter, or each could independently spend the root's full remaining
/// budget (a concurrent double-spend). Keying spend by the immediate parent —
/// the anchor all siblings share — makes their combined spend meter against one
/// atomic counter, so it can never exceed the root ceiling. A root token
/// (no parent) keys by its own id, unchanged.
///
/// The first attenuation stamps the lineage root and every deeper descendant
/// copies it unchanged.
#[must_use]
pub fn lineage_key(token: &TrustToken) -> &str {
    token
        .attenuation
        .root_id
        .as_deref()
        .unwrap_or(&token.token_id)
}

/// The privileged operation a verification is guarding (plan T1). Carried for
/// receipts and so a call site declares intent explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Issue,
    Attenuate,
    Verify,
    Seal,
    Unseal,
    Broker,
}

/// Everything required to authenticate a token *in context* (plan T1).
///
/// The signature primitive ([`verify`]) only answers "is this signed by this
/// key". A privileged call site needs more: issuer key, current time (expiry +
/// not-before), tenant binding, revocation, bundle version, and the operation.
/// Bundling them in one required-fields struct makes it impossible to *omit* a
/// check at a call site — you cannot construct the context without the issuer
/// key, clock, and operation, and [`verify_in_context`] runs every check.
pub struct VerificationContext<'a> {
    verifier: &'a dyn Verifier,
    issuer_public_key: &'a [u8],
    now: DateTime<Utc>,
    operation: Operation,
    expected_tenant: Option<&'a str>,
    expected_bundle_version: Option<&'a str>,
    require_fresh_revocation: bool,
    current_bundle: Option<&'a str>,
    permit_legacy_verify: bool,
    revocation: Option<&'a fabric_revocation::RevocationSnapshot>,
}

impl<'a> VerificationContext<'a> {
    /// A context that will verify a token signed by `issuer_public_key` as of
    /// `now`, guarding `operation`. Add tenant/bundle/revocation requirements
    /// with the builder methods.
    #[must_use]
    pub fn new(
        verifier: &'a dyn Verifier,
        issuer_public_key: &'a [u8],
        now: DateTime<Utc>,
        operation: Operation,
    ) -> Self {
        Self {
            verifier,
            issuer_public_key,
            now,
            operation,
            expected_tenant: None,
            expected_bundle_version: None,
            require_fresh_revocation: false,
            current_bundle: None,
            permit_legacy_verify: false,
            revocation: None,
        }
    }

    /// Require the token to be bound to `tenant`.
    #[must_use]
    pub fn expect_tenant(mut self, tenant: &'a str) -> Self {
        self.expected_tenant = Some(tenant);
        self
    }

    /// Require the token's trust-bundle version to equal `bundle`.
    #[must_use]
    pub fn expect_bundle(mut self, bundle: &'a str) -> Self {
        self.expected_bundle_version = Some(bundle);
        self
    }

    /// Fail closed when the token's revocation status is `unknown` (not just
    /// `revoked`). Use where a current revocation snapshot is mandatory.
    #[must_use]
    pub fn require_fresh_revocation(mut self) -> Self {
        self.require_fresh_revocation = true;
        self
    }

    /// Apply the token-version policy (plan T6): tokens whose bundle equals
    /// `current` are v2; any other bundle is legacy (v1). Legacy tokens are
    /// denied by default ([`TokenError::UnsupportedTokenVersion`]) and can never
    /// be an attenuation parent. Pair with [`permit_legacy_verify`] for a bounded
    /// verify-only migration window.
    #[must_use]
    pub fn require_current_bundle(mut self, current: &'a str) -> Self {
        self.current_bundle = Some(current);
        self
    }

    /// Allow a legacy (v1) token to *verify* (not attenuate) under the version
    /// policy — the bounded migration path. No effect unless
    /// [`require_current_bundle`](Self::require_current_bundle) is set.
    #[must_use]
    pub fn permit_legacy_verify(mut self) -> Self {
        self.permit_legacy_verify = true;
        self
    }

    /// Whether `token` is a legacy (v1) token under this context's version
    /// policy. `false` when no policy is set.
    #[must_use]
    pub fn is_legacy(&self, token: &TrustToken) -> bool {
        self.current_bundle
            .is_some_and(|cur| token.trust_bundle_version != cur)
    }

    /// Require the token to pass a **verified, current** signed revocation
    /// snapshot: every consumer that authenticates a token also honors
    /// revocation on every dimension. The caller verifies the snapshot signature
    /// and hands it in; `verify_in_context` then fails closed if the snapshot is
    /// expired or revokes the token. Implies [`require_fresh_revocation`].
    #[must_use]
    pub fn with_revocation(mut self, snapshot: &'a fabric_revocation::RevocationSnapshot) -> Self {
        self.revocation = Some(snapshot);
        self.require_fresh_revocation = true;
        self
    }

    /// The operation this context guards.
    #[must_use]
    pub fn operation(&self) -> Operation {
        self.operation
    }
}

/// Authenticate a token against a full [`VerificationContext`] (plan T1/T3):
/// revocation, signature under the issuer key, expiry, not-before, tenant, and
/// bundle version — every one, in one call.
///
/// # Errors
/// The specific [`TokenError`] for the first failing check.
pub fn verify_in_context(
    token: &TrustToken,
    ctx: &VerificationContext<'_>,
) -> Result<(), TokenError> {
    // Revocation first — a revoked token is never worth verifying further.
    match token.revocation_status {
        RevocationStatus::Revoked => return Err(TokenError::Revoked),
        RevocationStatus::Unknown if ctx.require_fresh_revocation && ctx.revocation.is_none() => {
            return Err(TokenError::RevocationUnknown);
        }
        _ => {}
    }
    // A verified signed snapshot is the fresh revocation state. Fail closed
    // if it is expired, and deny on any revoked dimension.
    if let Some(snapshot) = ctx.revocation {
        let snap_exp = DateTime::parse_from_rfc3339(&snapshot.expires_at)
            .map_err(|_| TokenError::BadTimestamp(snapshot.expires_at.clone()))?
            .with_timezone(&Utc);
        if ctx.now >= snap_exp {
            return Err(TokenError::RevocationUnknown);
        }
        if snapshot.revokes(token).is_some() {
            return Err(TokenError::Revoked);
        }
    }
    // Signature under the trusted issuer key.
    let hash = signing_hash(token)?;
    let sig = hex::decode(&token.signature.value).map_err(|_| TokenError::MalformedSignature)?;
    match ctx.verifier.verify(&hash, &sig, ctx.issuer_public_key) {
        Ok(true) => {}
        _ => return Err(TokenError::InvalidSignature),
    }
    // Time: not expired, and not before its issue instant.
    let exp = DateTime::parse_from_rfc3339(&token.expires_at)
        .map_err(|_| TokenError::BadTimestamp(token.expires_at.clone()))?
        .with_timezone(&Utc);
    if ctx.now >= exp {
        return Err(TokenError::Expired);
    }
    let iat = DateTime::parse_from_rfc3339(&token.issued_at)
        .map_err(|_| TokenError::BadTimestamp(token.issued_at.clone()))?
        .with_timezone(&Utc);
    if iat > ctx.now {
        return Err(TokenError::NotYetValid);
    }
    // Tenant + bundle bindings.
    if let Some(t) = ctx.expected_tenant
        && token.tenant_id != t
    {
        return Err(TokenError::TenantMismatch);
    }
    if let Some(b) = ctx.expected_bundle_version
        && token.trust_bundle_version != b
    {
        return Err(TokenError::BundleMismatch);
    }
    // T6 version policy: a legacy (v1) bundle is denied unless a bounded
    // verify-only migration is explicitly permitted.
    if ctx.is_legacy(token) && !ctx.permit_legacy_verify {
        return Err(TokenError::UnsupportedTokenVersion(
            token.trust_bundle_version.clone(),
        ));
    }
    Ok(())
}

/// Restriction-only attenuation input (plan T2). A child can only be a
/// *narrowing* of an authenticated parent: `None` on an axis inherits the
/// parent unchanged; `Some(..)` must be a subset / lower / earlier value. The
/// child's identity and authority fields (tenant, subject, service identity,
/// issuer, bundle, revocation status) are **generated server-side by copying
/// the authenticated parent** — they are absent here by construction, so an
/// attacker cannot supply them.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenRestrictions {
    /// The child's token id. Must be non-empty and differ from the parent.
    pub new_token_id: String,
    /// Earlier expiry. `None` inherits the parent's.
    #[serde(default)]
    pub expires_at: Option<String>,
    /// Route subset. `None` inherits; `Some` must be ⊆ parent.
    #[serde(default)]
    pub allowed_routes: Option<Vec<Route>>,
    /// Model subset. `None` inherits; `Some` narrows (⊆ parent, or any list when
    /// the parent is unrestricted).
    #[serde(default)]
    pub allowed_models: Option<Vec<String>>,
    /// Role subset. `None` inherits; `Some` must be ⊆ parent.
    #[serde(default)]
    pub roles: Option<Vec<String>>,
    /// Compliance-scope subset. `None` inherits; `Some` must be ⊆ parent.
    #[serde(default)]
    pub compliance_scopes: Option<Vec<ComplianceScope>>,
    /// Lower classification ceiling. `None` inherits; `Some` must be ≤ parent.
    #[serde(default)]
    pub max_data_classification: Option<Classification>,
    /// Tighter budget. `None` inherits; `Some` caps must fit the parent's remaining.
    #[serde(default)]
    pub budget: Option<Budget>,
    /// If true, force the child offline (a tightening). Never turns a parent's
    /// offline mode off.
    #[serde(default)]
    pub set_offline_mode: bool,
}

impl TokenRestrictions {
    /// A no-op narrowing with the given child id (inherits every parent axis).
    #[must_use]
    pub fn new(new_token_id: impl Into<String>) -> Self {
        Self {
            new_token_id: new_token_id.into(),
            ..Self::default()
        }
    }
}

/// Mint a child that narrows an **authenticated** parent (plan T2/T3/T4).
///
/// Unlike the old full-child signature, the caller cannot
/// supply a child's identity or a forged parent:
///
/// 1. **Parent authentication (T3):** the parent is verified with
///    [`verify_in_context`] — signature under the trusted issuer key, not
///    expired, not before its issue instant, not revoked, correct tenant and
///    bundle. An unsigned, wrong-key, expired, revoked, stale-bundle, or
///    wrong-tenant parent fails here, before any child exists.
/// 2. **Server-side identity (T2):** the child starts as a copy of the
///    authenticated parent, so tenant, subject, service identity, issuer,
///    bundle, and revocation status are inherited — never attacker-supplied.
/// 3. **Complete monotonicity (T4):** every restriction may only narrow
///    (routes/models/roles/scopes subset, classification ≤, budget fits
///    remaining, expiry earlier, offline can only turn on). The child id must be
///    non-empty and differ from the parent (no trivial cycle / duplicate).
///
/// `max_child_depth` caps how many further attenuations the *caller* has
/// authorized this hop to allow; `None` means the caller is not tracking depth
/// here (the service layer enforces chain depth against lineage). `0` refuses.
///
/// # Errors
/// The specific [`TokenError`] for the first failing check (parent
/// authentication, widening, id, or depth), or a signing error.
pub fn attenuate(
    parent: &TrustToken,
    restrictions: &TokenRestrictions,
    ctx: &VerificationContext<'_>,
    max_child_depth: Option<u32>,
    signer: &dyn Signer,
) -> Result<TrustToken, TokenError> {
    // 1. Authenticate the parent — the fix. No child is constructed
    //    until the presented parent is proven genuine and current.
    verify_in_context(parent, ctx)?;
    // T6: a legacy token may (under a migration flag) verify, but is never a
    // valid attenuation parent — no v1 attenuation, ever.
    if ctx.is_legacy(parent) {
        return Err(TokenError::LegacyAttenuationDenied(
            parent.trust_bundle_version.clone(),
        ));
    }
    let child = narrow_child(parent, restrictions, ctx.now, max_child_depth)?;
    issue(child, signer)
}

/// Attenuate a parent the caller has **already authenticated at its own trust
/// boundary** (plan T2/T4), skipping only the signature/time/tenant checks that
/// [`attenuate`] performs.
///
/// # Safety-critical contract
/// `parent` MUST have been verified against the correct issuer key before this
/// call (e.g. an admission front door that already ran [`verify`] /
/// [`verify_in_context`] against the trust anchor). Passing an unauthenticated
/// parent reintroduces the vulnerability — the signer will mint a child of a forged token.
/// Prefer [`attenuate`] wherever the issuer key is available at the call site.
///
/// The server-side identity copy and complete monotonicity (T2/T4) are enforced
/// identically to [`attenuate`]; only parent authentication is the caller's
/// responsibility here.
///
/// # Errors
/// [`TokenError`] on any widening, invalid child id, exceeded depth, or signing.
pub fn attenuate_preverified(
    parent: &TrustToken,
    restrictions: &TokenRestrictions,
    now: DateTime<Utc>,
    max_child_depth: Option<u32>,
    signer: &dyn Signer,
) -> Result<TrustToken, TokenError> {
    let child = narrow_child(parent, restrictions, now, max_child_depth)?;
    issue(child, signer)
}

/// Build the narrowed, unsigned child from an (already-authenticated) parent:
/// server-side identity copy (T2) + complete monotonicity (T4). Shared by
/// [`attenuate`] and [`attenuate_preverified`].
fn narrow_child(
    parent: &TrustToken,
    restrictions: &TokenRestrictions,
    now: DateTime<Utc>,
    max_child_depth: Option<u32>,
) -> Result<TrustToken, TokenError> {
    let child_depth = parent.attenuation.depth.saturating_add(1);
    if max_child_depth.is_some_and(|maximum| child_depth > maximum) {
        return Err(TokenError::DepthExceeded);
    }

    // Child id: non-empty and not the parent's (no trivial cycle / duplicate).
    if restrictions.new_token_id.is_empty()
        || restrictions.new_token_id == parent.token_id
        || parent
            .attenuation
            .root_id
            .as_ref()
            .is_some_and(|root| root == &restrictions.new_token_id)
        || parent
            .attenuation
            .ancestor_ids
            .contains(&restrictions.new_token_id)
    {
        return Err(TokenError::InvalidChildId);
    }

    // 2. Child inherits the authenticated parent's identity + authority.
    let mut child = parent.clone();
    child.token_id = restrictions.new_token_id.clone();
    child.issued_at = now.to_rfc3339();
    child.attenuation.root_id = Some(
        parent
            .attenuation
            .root_id
            .clone()
            .unwrap_or_else(|| parent.token_id.clone()),
    );
    child.attenuation.parent_id = Some(parent.token_id.clone());
    child.attenuation.depth = child_depth;
    child.attenuation.ancestor_ids = parent.attenuation.ancestor_ids.clone();
    child.attenuation.ancestor_ids.push(parent.token_id.clone());

    // 3. Apply each restriction, narrowing only.
    if let Some(exp) = &restrictions.expires_at {
        let c_exp = DateTime::parse_from_rfc3339(exp)
            .map_err(|_| TokenError::BadTimestamp(exp.clone()))?
            .with_timezone(&Utc);
        let p_exp = DateTime::parse_from_rfc3339(&parent.expires_at)
            .map_err(|_| TokenError::BadTimestamp(parent.expires_at.clone()))?
            .with_timezone(&Utc);
        if c_exp > p_exp {
            return Err(TokenError::AttenuationWidens { axis: "expires_at" });
        }
        child.expires_at = exp.clone();
    }
    if let Some(routes) = &restrictions.allowed_routes {
        if !routes.iter().all(|r| parent.allowed_routes.contains(r)) {
            return Err(TokenError::AttenuationWidens {
                axis: "allowed_routes",
            });
        }
        child.allowed_routes = routes.clone();
    }
    if let Some(models) = &restrictions.allowed_models {
        // Narrowing when the parent restricts: child ⊆ parent. When the parent is
        // unrestricted (empty), *any* explicit list is a narrowing. But an EMPTY
        // child list means "unrestricted = all models", so against a
        // restricted parent it is a WIDENING — reject it rather than granting the
        // child every model the parent was walled off from.
        if !parent.allowed_models.is_empty()
            && (models.is_empty() || !models.iter().all(|m| parent.allowed_models.contains(m)))
        {
            return Err(TokenError::AttenuationWidens {
                axis: "allowed_models",
            });
        }
        child.allowed_models = models.clone();
    }
    if let Some(roles) = &restrictions.roles {
        if !roles.iter().all(|r| parent.roles.contains(r)) {
            return Err(TokenError::AttenuationWidens { axis: "roles" });
        }
        child.roles = roles.clone();
    }
    if let Some(scopes) = &restrictions.compliance_scopes {
        if !scopes.iter().all(|s| parent.compliance_scopes.contains(s)) {
            return Err(TokenError::AttenuationWidens {
                axis: "compliance_scopes",
            });
        }
        child.compliance_scopes = scopes.clone();
    }
    if let Some(class) = restrictions.max_data_classification {
        if class > parent.max_data_classification {
            return Err(TokenError::AttenuationWidens {
                axis: "max_data_classification",
            });
        }
        child.max_data_classification = class;
    }
    if let Some(cb) = &restrictions.budget {
        if let Some(pb) = &parent.budget
            && (cb.token_cap > pb.token_cap.saturating_sub(pb.tokens_spent)
                || cb.usd_cap_cents > pb.usd_cap_cents.saturating_sub(pb.usd_spent_cents)
                || cb.tool_call_cap > pb.tool_call_cap.saturating_sub(pb.tool_calls_spent))
        {
            return Err(TokenError::AttenuationWidens { axis: "budget" });
        }
        // Fresh child budget: requested caps, spent reset.
        child.budget = Some(Budget {
            token_cap: cb.token_cap,
            tokens_spent: 0,
            usd_cap_cents: cb.usd_cap_cents,
            usd_spent_cents: 0,
            tool_call_cap: cb.tool_call_cap,
            tool_calls_spent: 0,
        });
    }
    // Offline mode can only be tightened on (never turned off).
    if restrictions.set_offline_mode {
        child.offline_mode = true;
    }

    // The signature is rebuilt by issue(); the child carries the parent's stale
    // signature until then, which is fine (issue overwrites it wholesale).
    Ok(child)
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

#[cfg(test)]
mod attenuation_monotonicity_tests {
    use super::{TokenError, TokenRestrictions, narrow_child};
    use chrono::{DateTime, TimeZone, Utc};
    use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature, TrustToken};

    fn parent_with_models(models: Vec<String>) -> TrustToken {
        TrustToken {
            token_id: "parent".into(),
            issued_at: "2026-07-03T00:00:00Z".into(),
            expires_at: "2099-01-01T00:00:00Z".into(),
            issuer: "wsf-bridge".into(),
            trust_bundle_version: "2026.07.v2".into(),
            tenant_id: "baap".into(),
            subject_id: None,
            subject_hash: "hmac:abc".into(),
            service_identity: None,
            identity_id: None,
            roles: vec![],
            compliance_scopes: vec![],
            allowed_routes: vec![],
            allowed_models: models,
            max_data_classification: Classification::Restricted,
            country: None,
            person_type: None,
            offline_mode: false,
            revocation_status: RevocationStatus::Unknown,
            budget: None,
            attenuation: Attenuation::default(),
            signature: Signature {
                alg: String::new(),
                key_id: String::new(),
                value: String::new(),
            },
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 4, 0, 0, 0).unwrap()
    }

    #[test]
    fn empty_child_models_is_a_widening_against_a_restricted_parent() {
        // An empty child list means "all models"; against a restricted
        // parent that is a WIDENING and must be refused.
        let parent = parent_with_models(vec!["gpt-4".into()]);
        let r = TokenRestrictions {
            allowed_models: Some(vec![]),
            ..TokenRestrictions::new("child")
        };
        assert!(matches!(
            narrow_child(&parent, &r, now(), None),
            Err(TokenError::AttenuationWidens {
                axis: "allowed_models"
            })
        ));
    }

    #[test]
    fn subset_child_models_narrows_ok() {
        let parent = parent_with_models(vec!["gpt-4".into(), "gpt-3".into()]);
        let r = TokenRestrictions {
            allowed_models: Some(vec!["gpt-4".into()]),
            ..TokenRestrictions::new("child")
        };
        let child = narrow_child(&parent, &r, now(), None).unwrap();
        assert_eq!(child.allowed_models, vec!["gpt-4".to_owned()]);
    }

    #[test]
    fn empty_child_models_ok_when_parent_unrestricted() {
        // Parent already unrestricted (empty = all): an empty child stays unrestricted.
        let parent = parent_with_models(vec![]);
        let r = TokenRestrictions {
            allowed_models: Some(vec![]),
            ..TokenRestrictions::new("child")
        };
        assert!(narrow_child(&parent, &r, now(), None).is_ok());
    }

    #[test]
    fn tenant_maximum_depth_is_enforced_across_recursive_attenuation() {
        let parent = parent_with_models(vec![]);
        let child =
            narrow_child(&parent, &TokenRestrictions::new("child"), now(), Some(1)).unwrap();
        assert_eq!(child.attenuation.depth, 1);
        assert_eq!(child.attenuation.root_id.as_deref(), Some("parent"));
        assert_eq!(child.attenuation.ancestor_ids, vec!["parent"]);
        assert_eq!(
            narrow_child(
                &child,
                &TokenRestrictions::new("grandchild"),
                now(),
                Some(1),
            )
            .unwrap_err(),
            TokenError::DepthExceeded
        );
    }

    #[test]
    fn recursive_root_or_ancestor_id_reuse_is_rejected() {
        let parent = parent_with_models(vec![]);
        let child =
            narrow_child(&parent, &TokenRestrictions::new("child"), now(), Some(3)).unwrap();
        assert_eq!(
            narrow_child(&child, &TokenRestrictions::new("parent"), now(), Some(3),).unwrap_err(),
            TokenError::InvalidChildId
        );
    }
}
