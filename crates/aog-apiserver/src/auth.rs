//! K6 — front-door WSF authentication.
//!
//! Every API request must carry a valid, in-budget, unrevoked WSF trust token,
//! verified **before** the admission chain runs (the K6 gate: unauth /
//! over-budget / revoked is rejected pre-admission). Verification is **local
//! asymmetric crypto** — ML-DSA-87 over the token's canonical payload — so it is
//! sub-millisecond and offline, with no OpenBao round-trip on the hot path.
//! This is doctrine I-3 in force: authority is re-earned by verifying the token
//! on *every* request, never by trusting a prior session, and I-4: any
//! uncertainty (missing, malformed, unverifiable, expired, revoked) fails closed.
//!
//! The token is presented as `x-wsf-token: base64(json(TrustToken))`. The
//! verified [`Principal`] is stashed in request extensions for the handler and
//! the admission chain (its `mutate`/`receipt` stages stamp the token as
//! provenance; K8 attenuates a child from it).

use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use fabric_contracts::{Budget, TrustToken};
use fabric_crypto::providers::MlDsa87Verifier;
use fabric_revocation::{RevocationError, RevocationSnapshot};

use crate::AppState;
use crate::admission::Principal;
use crate::error::ApiError;

/// Header carrying the base64-encoded JSON trust token.
pub const TOKEN_HEADER: &str = "x-wsf-token";

/// The front-door authenticator: the WSF trust-anchor public key every presented
/// token must verify under, plus an optional (already signature-verified)
/// revocation snapshot — the kill switch consulted on every request.
pub struct Authenticator {
    token_public_key: Vec<u8>,
    revocation: Option<RevocationSnapshot>,
}

impl Authenticator {
    /// Build an authenticator anchored on the WSF trust public key.
    #[must_use]
    pub fn new(token_public_key: Vec<u8>) -> Self {
        Self {
            token_public_key,
            revocation: None,
        }
    }

    /// Attach a revocation snapshot (the kill switch). The snapshot's own
    /// signature is verified against the trust anchor here — a snapshot that does
    /// not verify is refused, never silently ignored (fail-closed, doctrine I-4).
    ///
    /// # Errors
    /// [`RevocationError`] if the snapshot signature does not verify.
    pub fn with_revocation(
        mut self,
        snapshot: RevocationSnapshot,
    ) -> Result<Self, RevocationError> {
        fabric_revocation::verify(&snapshot, &MlDsa87Verifier, &self.token_public_key)?;
        self.revocation = Some(snapshot);
        Ok(self)
    }

    /// Verify a presented token and yield the authenticated principal, or refuse.
    /// Every failure resolves toward *less* privilege (doctrine I-4).
    ///
    /// # Errors
    /// [`ApiError::Unauthenticated`] when the token is missing, malformed, fails
    /// signature/expiry, or is revoked; [`ApiError::BudgetExhausted`] when over
    /// budget.
    pub fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, ApiError> {
        let raw = headers
            .get(TOKEN_HEADER)
            .and_then(|v| v.to_str().ok())
            .ok_or(ApiError::Unauthenticated)?;
        let bytes = BASE64
            .decode(raw.trim())
            .map_err(|_| ApiError::Unauthenticated)?;
        let token: TrustToken =
            serde_json::from_slice(&bytes).map_err(|_| ApiError::Unauthenticated)?;

        // Signature + on-token revocation status (local ML-DSA verify).
        fabric_token::verify(&token, &MlDsa87Verifier, &self.token_public_key)
            .map_err(|_| ApiError::Unauthenticated)?;

        // Expiry (the token's own expiry caveat).
        if fabric_token::is_expired(&token, chrono::Utc::now())
            .map_err(|_| ApiError::Unauthenticated)?
        {
            return Err(ApiError::Unauthenticated);
        }

        // Kill switch: a revoked token or subject halts the next call.
        if let Some(snap) = &self.revocation
            && (snap.is_token_revoked(&token.token_id)
                || snap.is_subject_revoked(&token.subject_hash))
        {
            return Err(ApiError::Unauthenticated);
        }

        // Budget pre-flight — reject an exhausted token before it acts.
        if let Some(budget) = &token.budget
            && budget_exhausted(budget)
        {
            return Err(ApiError::BudgetExhausted);
        }

        Ok(Principal::authenticated(token))
    }
}

/// Any budget dimension exhausted (a cap of 0 means that axis is unused).
fn budget_exhausted(b: &Budget) -> bool {
    (b.token_cap > 0 && b.tokens_spent >= b.token_cap)
        || (b.usd_cap_cents > 0 && b.usd_spent_cents >= b.usd_cap_cents)
        || (b.tool_call_cap > 0 && b.tool_calls_spent >= b.tool_call_cap)
}

/// axum middleware: authenticate an API request before its handler runs, and
/// stash the verified [`Principal`] in request extensions for the handler +
/// admission chain. Applied only to `/apis/**` — health probes stay open.
///
/// # Errors
/// Propagates the [`Authenticator::authenticate`] refusal as the response.
pub async fn require_token(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let principal = state.authenticator.authenticate(req.headers())?;
    req.extensions_mut().insert(principal);
    Ok(next.run(req).await)
}
