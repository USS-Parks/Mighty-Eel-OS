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

/// The shared budget-metering key for a token's attenuation lineage (plan T5).
///
/// Sibling children of one parent must draw from a *single* shared spend
/// counter, or each could independently spend the parent's full remaining
/// budget (a concurrent double-spend). Keying spend by the immediate parent —
/// the anchor all siblings share — makes their combined spend meter against one
/// atomic counter, so it can never exceed the parent ceiling. A root token
/// (no parent) keys by its own id, unchanged.
///
/// This binds one level of siblings to their parent's pool. Accounting a full
/// deep subtree against the lineage *root* additionally needs the chain, which
/// lives in the receipt ledger (Phase L) — documented, not silently assumed.
#[must_use]
pub fn lineage_key(token: &TrustToken) -> &str {
    token
        .attenuation
        .parent_id
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
    /// `revoked`). Use where a current revocation snapshot is mandatory (R3).
    #[must_use]
    pub fn require_fresh_revocation(mut self) -> Self {
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
        RevocationStatus::Unknown if ctx.require_fresh_revocation => {
            return Err(TokenError::RevocationUnknown);
        }
        _ => {}
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
/// This closes AF-001. Unlike the old full-child signature, the caller cannot
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
    // 1. Authenticate the parent — the AF-001 fix. No child is constructed
    //    until the presented parent is proven genuine and current.
    verify_in_context(parent, ctx)?;
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
/// parent reintroduces AF-001 — the signer will mint a child of a forged token.
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
    // Depth budget for this hop.
    if matches!(max_child_depth, Some(0)) {
        return Err(TokenError::DepthExceeded);
    }

    // Child id: non-empty and not the parent's (no trivial cycle / duplicate).
    if restrictions.new_token_id.is_empty() || restrictions.new_token_id == parent.token_id {
        return Err(TokenError::InvalidChildId);
    }

    // 2. Child inherits the authenticated parent's identity + authority.
    let mut child = parent.clone();
    child.token_id = restrictions.new_token_id.clone();
    child.issued_at = now.to_rfc3339();
    child.attenuation.parent_id = Some(parent.token_id.clone());

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
        // Narrowing when the parent restricts: child ⊆ parent. When the parent
        // is unrestricted (empty), *any* explicit list is a narrowing.
        if !parent.allowed_models.is_empty()
            && !models.iter().all(|m| parent.allowed_models.contains(m))
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
