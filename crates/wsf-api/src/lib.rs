//! `wsf-api` — one REST surface over the WSF trust plane, plus a Rust SDK.
//!
//! Mounts the four services behind stable `/v1` routes: token issue / verify /
//! attenuate (`wsf-bridge`), envelope seal / unseal (`wsf-seal`), credential
//! exchange (`wsf-broker`), and receipt query (`wsf-ledger`). Every token issue
//! and every seal/unseal op is receipted into the ledger, so `/v1/receipts`
//! reflects a live, correlated record. The OpenAPI document is served at
//! `/openapi.json`; the [`client`] module is the typed SDK ([`client::WsfClient`])
//! that round-trips every endpoint.
//!
//! Deferred (documented): a TypeScript client for the console lands with Phase C;
//! folding the SDK into `mai-sdk-rs` and a gRPC/tonic-0.14 surface are follow-ons
//! (the axum-0.8 half of the 0.2d pin is exercised here and in W3).

pub mod audit;
pub mod auth;
pub mod client;
pub mod grants;
pub mod policy;
pub mod posture;

use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};

use axum::extract::{Extension, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use fabric_contracts::{
    Audience, Budget, CanonicalResource, Envelope, RequestOperation, TrustToken,
    VerifiedRequestContext, WsfPrincipal,
};

use audit::AuditorStore;
use auth::{WsfAuthenticator, require_principal};
use base64::Engine;
use chrono::Utc;
use fabric_crypto::providers::MlDsa87Verifier;
use grants::GrantStore;
use policy::TenantPolicyStore;
use serde::{Deserialize, Serialize};
use wsf_bridge::{IssueTokenRequest, TrustBridge};
use wsf_broker::AwsStsBroker;
use wsf_ledger::{Ledger, LedgerEntry};
use wsf_seal::{LabelSpec, SealRequest, SealService, UnsealRequest};

/// Explicit revocation posture for privileged handlers. Production constructs
/// only `Required`; bypass is named and limited to the development profile.
#[derive(Clone)]
pub enum RevocationEnforcement {
    Required(Arc<RwLock<fabric_revocation::MonotonicRevocationStore>>),
    DevelopmentDisabled,
}

impl RevocationEnforcement {
    #[must_use]
    pub fn required(store: Arc<RwLock<fabric_revocation::MonotonicRevocationStore>>) -> Self {
        Self::Required(store)
    }

    #[must_use]
    pub fn development_disabled() -> Self {
        Self::DevelopmentDisabled
    }

    fn authorize_token(&self, token: &TrustToken) -> Result<(), String> {
        match self {
            Self::Required(store) => store
                .read()
                .map_err(|_| "revocation store lock poisoned".to_string())?
                .authorize(token, Utc::now())
                .map_err(|e| e.to_string()),
            Self::DevelopmentDisabled => Ok(()),
        }
    }

    fn authorize_principal(&self, principal: &WsfPrincipal) -> Result<(), String> {
        match self {
            Self::Required(store) => store
                .read()
                .map_err(|_| "revocation store lock poisoned".to_string())?
                .authorize_principal(principal, Utc::now())
                .map_err(|e| e.to_string()),
            Self::DevelopmentDisabled => Ok(()),
        }
    }

    #[must_use]
    pub fn required_store(
        &self,
    ) -> Option<Arc<RwLock<fabric_revocation::MonotonicRevocationStore>>> {
        match self {
            Self::Required(store) => Some(Arc::clone(store)),
            Self::DevelopmentDisabled => None,
        }
    }
}

/// Shared attenuation state: authoritative root-lineage spend plus a
/// process-wide child-id registry. Both are mandatory so handler-local signed
/// counters and duplicate requests cannot reset authority.
#[derive(Debug, Default)]
pub struct AttenuationState {
    reservations: fabric_token::spend::ReservationLedger,
    child_ids: Mutex<HashSet<String>>,
}

impl AttenuationState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record completed spend against the immutable root lineage. The same
    /// ledger is folded into every later attenuation decision.
    pub fn commit_spend(
        &self,
        token: &TrustToken,
        usage: fabric_token::spend::Spent,
    ) -> Result<(), fabric_token::spend::ReservationError> {
        let Some(cap) = token.budget.as_ref() else {
            return Err(fabric_token::spend::ReservationError::Exhausted);
        };
        self.reservations
            .reserve(
                fabric_token::spend::ReservationKey::for_token(token, None, None),
                cap,
                usage,
            )?
            .commit()
    }

    fn current_parent(&self, parent: &TrustToken) -> TrustToken {
        let mut current = parent.clone();
        if let Some(budget) = current.budget.as_mut() {
            let spent =
                self.reservations
                    .committed(&fabric_token::spend::ReservationKey::for_token(
                        parent, None, None,
                    ));
            budget.tokens_spent = budget.tokens_spent.max(spent.tokens);
            budget.usd_spent_cents = budget.usd_spent_cents.max(spent.usd_cents);
            budget.tool_calls_spent = budget.tool_calls_spent.max(spent.tool_calls);
        }
        current
    }

    fn claim_child_id(&self, child_id: &str) -> bool {
        self.child_ids
            .lock()
            .expect("attenuation child-id lock")
            .insert(child_id.to_owned())
    }

    fn release_child_id(&self, child_id: &str) {
        self.child_ids
            .lock()
            .expect("attenuation child-id lock")
            .remove(child_id);
    }
}

/// The OpenAPI 3.0 document for the WSF API, served at `/openapi.json`.
pub const OPENAPI_JSON: &str = include_str!("openapi.json");

/// Shared application state — the four trust-plane services + the token anchor.
#[derive(Clone)]
pub struct AppState {
    /// Trust bridge (token issue / attenuate signing).
    pub bridge: Arc<TrustBridge>,
    /// STS credential broker.
    pub broker: Arc<AwsStsBroker>,
    /// Seal service.
    pub seal: Arc<SealService>,
    /// Unified receipt ledger.
    pub ledger: Arc<Mutex<Ledger>>,
    /// Trust-anchor public key for verifying presented tokens.
    pub token_public_key: Arc<Vec<u8>>,
    /// Transport authenticator (plan A2). Establishes the calling principal for
    /// every privileged route before its handler runs.
    pub auth: Arc<dyn WsfAuthenticator>,
    /// Server-side tenant issuance policy (plan A3). Bounds what any principal
    /// may be granted; the caller supplies intent, never authority.
    pub policy: Arc<dyn TenantPolicyStore>,
    /// Server-side cloud-credential grants (plan B1/B2). Resolves a tenant-scoped
    /// grant_id to an approved cloud identity; the caller never submits a raw ARN.
    pub grants: Arc<dyn GrantStore>,
    /// Server-side global-auditor enrollment (plan L2). The only path past
    /// receipt tenant scoping; `StaticAuditors::none()` for non-audit planes.
    pub auditors: Arc<dyn AuditorStore>,
    /// Mandatory current revocation in production; explicit bypass in dev.
    pub revocation: RevocationEnforcement,
    /// Mandatory root-lineage remaining-authority and duplicate-id state.
    pub attenuation: Arc<AttenuationState>,
}

/// Mount all routes over `state`.
///
/// Privileged `/v1/*` routes are wrapped by the [`require_principal`]
/// middleware (plan A2): a missing, malformed, expired, wrong-audience, or
/// wrong-tenant credential is rejected 401/403 before the handler. `/healthz`
/// and `/openapi.json` are intentionally open.
pub fn router(state: AppState) -> Router {
    let privileged = Router::new()
        .route("/v1/tokens/issue", post(issue))
        .route("/v1/tokens/verify", post(verify))
        .route("/v1/tokens/attenuate", post(attenuate))
        .route("/v1/envelopes/seal", post(seal))
        .route("/v1/envelopes/unseal", post(unseal))
        .route("/v1/credentials/exchange", post(exchange))
        .route("/v1/receipts", get(receipts))
        .route("/v1/receipts/export", get(export_receipts))
        .route_layer(axum::middleware::from_fn_with_state(
            state.auth.clone(),
            require_principal,
        ))
        .with_state(state);

    Router::new()
        .route("/openapi.json", get(openapi))
        .route("/healthz", get(|| async { "ok" }))
        .merge(privileged)
}

// ── request / response DTOs (shared with the SDK) ──────────────────────

/// Issue-token request — **bounded intent only** (plan A3).
///
/// The tenant, subject, and effective authority are derived server-side from
/// the authenticated [`WsfPrincipal`] and the tenant's issuance policy. This
/// type carries *requests*, not authority: `deny_unknown_fields` means an
/// attempt to smuggle a `tenant_id` / `subject_id` / `roles` authority field is
/// a 422, not a silent override.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueReq {
    /// Roles the caller requests. Each must be grantable by the tenant policy;
    /// a role outside the policy is refused.
    #[serde(default)]
    pub requested_roles: Vec<String>,
    /// Models the caller requests. Must lie within the policy allowlist when the
    /// tenant restricts models.
    #[serde(default)]
    pub requested_models: Vec<String>,
    /// Requested budget. Every counter must be at or below the policy ceiling;
    /// omitted ⇒ the ceiling is granted (never unlimited).
    #[serde(default)]
    pub budget: Option<Budget>,
}

/// Issue-token response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResp {
    /// The signed token.
    pub token: TrustToken,
}

/// Verify-token request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReq {
    /// The token to verify.
    pub token: TrustToken,
}

/// Verify-token response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResp {
    /// Whether signature + revocation-status + expiry all pass.
    pub valid: bool,
    /// Reason (`ok` or the failure).
    pub reason: String,
}

/// Attenuate request — the presented parent plus **narrowing restrictions
/// only** (plan T2). The child's identity/authority fields are generated
/// server-side from the authenticated parent, so this request exposes no
/// attacker-suppliable child identity or signature field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttenuateReq {
    /// The parent token (authenticated server-side before any child is built).
    pub parent: TrustToken,
    /// How the child narrows the parent (subset/lower/earlier only).
    pub restrictions: fabric_token::TokenRestrictions,
}

/// Seal request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SealReq {
    /// Authorizing token.
    pub token: TrustToken,
    /// Base64 plaintext.
    pub plaintext_b64: String,
    /// Handling label.
    pub label: LabelSpec,
    /// Envelope id.
    pub envelope_id: String,
}

/// Seal response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SealResp {
    /// The sealed envelope.
    pub envelope: Envelope,
}

/// Unseal request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsealReq {
    /// Presenting token.
    pub token: TrustToken,
    /// The envelope to open.
    pub envelope: Envelope,
}

/// Unseal response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsealResp {
    /// Base64 recovered plaintext.
    pub plaintext_b64: String,
}

/// Credential-exchange request (plan B1). Carries a tenant-scoped `grant_id`,
/// never a raw cloud identity: `deny_unknown_fields` means a smuggled `role_arn`
/// is a 422, not honored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeReq {
    /// The verified trust token authorizing the exchange.
    pub token: TrustToken,
    /// The server-side grant to exercise (resolved to a cloud identity by policy).
    pub grant_id: String,
}

/// Credential-exchange response (ephemeral scoped creds).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeResp {
    /// Temporary access key id.
    pub access_key_id: String,
    /// Temporary secret access key.
    pub secret_access_key: String,
    /// Session token.
    pub session_token: String,
    /// Expiry (RFC3339).
    pub expiration: String,
}

/// Receipt-query response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptsResp {
    /// Matching ledger entries.
    pub entries: Vec<LedgerEntry>,
}

/// Receipt query parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct ReceiptsQuery {
    /// Correlation field (e.g. `token_id`).
    pub field: Option<String>,
    /// Correlation value.
    pub value: Option<String>,
}

// ── error mapping ──────────────────────────────────────────────────────

/// An API error carrying an HTTP status.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

fn b64_decode(s: &str) -> Result<Vec<u8>, ApiError> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, format!("bad base64: {e}")))
}

/// A privileged token-carrying call must present a WSF-plane principal whose
/// tenant matches the token's — no cross-tenant use of a token on this plane.
/// The seal, unseal, and exchange handlers all funnel through this so the
/// tenant binding is enforced identically (403 on mismatch).
fn enforce_token_tenant(
    context: &VerifiedRequestContext,
    expected_operation: RequestOperation,
    token: &TrustToken,
) -> Result<(), ApiError> {
    context
        .require_operation(expected_operation)
        .map_err(|e| ApiError::new(StatusCode::FORBIDDEN, e.to_string()))?;
    let principal = context.principal();
    if !principal.is_for(Audience::Wsf) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "principal is not authenticated for the WSF plane",
        ));
    }
    if token.tenant_id != principal.tenant_id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "token tenant does not match the authenticated principal",
        ));
    }
    Ok(())
}

/// Bind a transport-authenticated principal to the operation and final resource
/// selected by the server. Every privileged handler calls this before its sink;
/// payloads cannot deserialize either the principal or the resulting context.
fn require_request_context(
    principal: &WsfPrincipal,
    operation: RequestOperation,
    kind: &str,
    name: &str,
    tenant_id: Option<String>,
) -> Result<VerifiedRequestContext, ApiError> {
    let resource = CanonicalResource::resolved(kind, name, tenant_id)
        .map_err(|e| ApiError::new(StatusCode::FORBIDDEN, e.to_string()))?;
    let context = VerifiedRequestContext::establish(principal.clone(), operation, resource)
        .map_err(|e| ApiError::new(StatusCode::FORBIDDEN, e.to_string()))?;
    context
        .require_operation(operation)
        .map_err(|e| ApiError::new(StatusCode::FORBIDDEN, e.to_string()))?;
    Ok(context)
}

fn require_current_token(
    state: &AppState,
    context: &VerifiedRequestContext,
    token: &TrustToken,
) -> Result<(), ApiError> {
    state.revocation.authorize_token(token).map_err(|reason| {
        receipt_revocation_deny(state, context, Some(&token.token_id), &reason);
        ApiError::new(
            StatusCode::FORBIDDEN,
            "current revocation denied the operation",
        )
    })
}

fn require_current_principal(
    state: &AppState,
    context: &VerifiedRequestContext,
) -> Result<(), ApiError> {
    state
        .revocation
        .authorize_principal(context.principal())
        .map_err(|reason| {
            receipt_revocation_deny(state, context, None, &reason);
            ApiError::new(
                StatusCode::FORBIDDEN,
                "current revocation denied the operation",
            )
        })
}

fn receipt_revocation_deny(
    state: &AppState,
    context: &VerifiedRequestContext,
    token_id: Option<&str>,
    reason: &str,
) {
    let receipt = serde_json::json!({
        "kind": "revocation_decision",
        "decision": "deny",
        "reason": reason,
        "token_id": token_id,
        "tenant_id": context.principal().tenant_id,
        "subject_hash": context.principal().subject_hash,
        "operation": context.operation(),
        "resource": context.resource(),
        "correlation_id": context.principal().correlation_id,
        "recorded_at": Utc::now().to_rfc3339(),
    });
    let _ = state
        .ledger
        .lock()
        .expect("ledger lock")
        .ingest("wsf-revocation", receipt);
}

/// Authorize a requested budget against the tenant ceiling (plan A3): every
/// counter must be at or below the ceiling; an omitted request is granted the
/// ceiling exactly (never unlimited). Spent counters are always reset to zero.
fn authorize_budget(requested: Option<&Budget>, ceiling: &Budget) -> Result<Budget, ApiError> {
    let deny = |counter: &str| {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            format!("requested budget exceeds tenant ceiling: {counter}"),
        ))
    };
    let effective = match requested {
        None => ceiling.clone(),
        Some(r) => {
            if r.token_cap > ceiling.token_cap {
                return deny("token_cap");
            }
            if r.usd_cap_cents > ceiling.usd_cap_cents {
                return deny("usd_cap_cents");
            }
            if r.tool_call_cap > ceiling.tool_call_cap {
                return deny("tool_call_cap");
            }
            r.clone()
        }
    };
    Ok(Budget {
        token_cap: effective.token_cap,
        tokens_spent: 0,
        usd_cap_cents: effective.usd_cap_cents,
        usd_spent_cents: 0,
        tool_call_cap: effective.tool_call_cap,
        tool_calls_spent: 0,
    })
}

// ── handlers ───────────────────────────────────────────────────────────

async fn issue(
    State(s): State<AppState>,
    Extension(principal): Extension<WsfPrincipal>,
    Json(req): Json<IssueReq>,
) -> Result<Json<TokenResp>, ApiError> {
    let context = require_request_context(
        &principal,
        RequestOperation::WsfIssue,
        "tenant",
        &principal.tenant_id,
        Some(principal.tenant_id.clone()),
    )?;
    require_current_principal(&s, &context)?;
    let principal = context.principal();
    // Authority is derived from the authenticated principal + server-side tenant
    // policy — never from the request body (plan A3). Every
    // allow and deny is receipted (plan A4).
    let roles = &req.requested_roles;
    if !principal.is_for(Audience::Wsf) {
        return Err(deny_issuance(
            &s,
            principal,
            "unknown",
            "principal is not authenticated for the WSF plane",
            roles,
        ));
    }
    let Some(policy) = s.policy.policy_for(&principal.tenant_id) else {
        return Err(deny_issuance(
            &s,
            principal,
            "unknown",
            "no issuance policy for this tenant",
            roles,
        ));
    };

    // A4: classify the issuance mode and enforce the permission matrix +
    // delegation-depth gate before any authority is granted.
    let mode = policy.classify(principal.kind, roles);
    if !policy.permits_mode(mode) {
        return Err(deny_issuance(
            &s,
            principal,
            mode.label(),
            "issuance mode is not permitted for this tenant",
            roles,
        ));
    }
    if mode.is_delegation_capable() && policy.max_delegation_depth == 0 {
        return Err(deny_issuance(
            &s,
            principal,
            mode.label(),
            "tenant forbids delegation for this issuance mode",
            roles,
        ));
    }

    // Roles: requested must be a subset of what the tenant may grant.
    for role in roles {
        if !policy.may_grant_role(role) {
            return Err(deny_issuance(
                &s,
                principal,
                mode.label(),
                "requested role is not grantable for this tenant",
                roles,
            ));
        }
    }
    // Models: omission/empty resolves to the restrictive policy allowlist;
    // explicit values must be a subset. An over-broad request is denied.
    let Some(effective_models) = policy.models_for_request(&req.requested_models) else {
        return Err(deny_issuance(
            &s,
            principal,
            mode.label(),
            "requested model is not permitted for this tenant",
            roles,
        ));
    };
    if !policy.allows_service_identity(principal.service_identity.as_deref()) {
        return Err(deny_issuance(
            &s,
            principal,
            mode.label(),
            "authenticated service identity is not permitted for this tenant",
            roles,
        ));
    }
    // Budget: every counter at or below the ceiling; unspecified ⇒ the ceiling.
    let budget = match authorize_budget(req.budget.as_ref(), &policy.max_budget) {
        Ok(b) => b,
        Err(e) => {
            issuance_receipt(
                &s,
                principal,
                mode.label(),
                "deny",
                "requested budget exceeds tenant ceiling",
                roles,
            );
            return Err(e);
        }
    };

    // Subject is the authenticated principal, never a caller-supplied id.
    let subject_source = if principal.subject_hash.is_empty() {
        principal.principal_id.clone()
    } else {
        principal.subject_hash.clone()
    };

    // Empty/omitted requested_models means "use the policy allowlist", never
    // "unrestricted" when the policy is restrictive. Every other authority
    // axis is selected by authenticated server context + tenant policy.
    let ir = IssueTokenRequest::new(principal.tenant_id.clone(), subject_source, roles.clone())
        .with_models(effective_models)
        .with_authority(
            policy.allowed_routes.clone(),
            policy.compliance_scopes.clone(),
            policy.max_classification,
            principal.service_identity.clone(),
        )
        .with_budget(budget);

    let token = s.bridge.issue_token(&ir).await.map_err(|e| match e {
        wsf_bridge::BridgeError::OpenBao(_) => {
            ApiError::new(StatusCode::BAD_GATEWAY, e.to_string())
        }
        wsf_bridge::BridgeError::Config(_) => {
            ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, e.to_string())
        }
        _ => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    })?;

    // Allow receipt (the authorized decision) + the bridge's token correlation.
    issuance_receipt(&s, principal, mode.label(), "allow", "issued", roles);
    let correlation = s.bridge.audit_correlation(&token);
    if let Ok(value) = serde_json::to_value(&correlation) {
        let _ = s
            .ledger
            .lock()
            .expect("ledger lock")
            .ingest("wsf-bridge", value);
    }
    Ok(Json(TokenResp { token }))
}

/// Emit an issuance-decision receipt to the ledger (plan A4). Metadata only —
/// no cleartext subject, no token payload: the principal carries a `subject_hash`
/// and correlation id, matching the ledger's receipt contract.
fn issuance_receipt(
    s: &AppState,
    principal: &WsfPrincipal,
    mode: &str,
    decision: &str,
    reason: &str,
    requested_roles: &[String],
) {
    let receipt = serde_json::json!({
        "kind": "issuance_decision",
        "decision": decision,
        "mode": mode,
        "reason": reason,
        "tenant_id": principal.tenant_id,
        "principal_id": principal.principal_id,
        "subject_hash": principal.subject_hash,
        "auth_strength": principal.auth_strength,
        "correlation_id": principal.correlation_id,
        "requested_roles": requested_roles,
        "issued_at": Utc::now().to_rfc3339(),
    });
    let _ = s
        .ledger
        .lock()
        .expect("ledger lock")
        .ingest("wsf-issuance", receipt);
}

/// Emit a deny receipt and build the matching 403 (plan A4).
fn deny_issuance(
    s: &AppState,
    principal: &WsfPrincipal,
    mode: &str,
    reason: &'static str,
    requested_roles: &[String],
) -> ApiError {
    issuance_receipt(s, principal, mode, "deny", reason, requested_roles);
    ApiError::new(StatusCode::FORBIDDEN, reason)
}

async fn verify(
    State(s): State<AppState>,
    Extension(principal): Extension<WsfPrincipal>,
    Json(req): Json<VerifyReq>,
) -> Result<Json<VerifyResp>, ApiError> {
    let _context = require_request_context(
        &principal,
        RequestOperation::WsfVerify,
        "token",
        &req.token.token_id,
        Some(req.token.tenant_id.clone()),
    )?;
    require_current_token(&s, &_context, &req.token)?;
    if let Err(e) = fabric_token::verify(&req.token, &MlDsa87Verifier, &s.token_public_key) {
        return Ok(Json(VerifyResp {
            valid: false,
            reason: e.to_string(),
        }));
    }
    Ok(match fabric_token::is_expired(&req.token, Utc::now()) {
        Ok(false) => Json(VerifyResp {
            valid: true,
            reason: "ok".to_string(),
        }),
        Ok(true) => Json(VerifyResp {
            valid: false,
            reason: "token expired".to_string(),
        }),
        Err(e) => Json(VerifyResp {
            valid: false,
            reason: e.to_string(),
        }),
    })
}

async fn attenuate(
    State(s): State<AppState>,
    Extension(principal): Extension<WsfPrincipal>,
    Json(req): Json<AttenuateReq>,
) -> Result<Json<TokenResp>, ApiError> {
    let context = require_request_context(
        &principal,
        RequestOperation::WsfAttenuate,
        "token",
        &req.parent.token_id,
        Some(req.parent.tenant_id.clone()),
    )?;
    require_current_token(&s, &context, &req.parent)?;
    let principal = context.principal();
    // T3: authenticate the presented parent under the trust anchor, bound to the
    // caller's tenant, before any child is constructed. The child's identity is
    // copied from the authenticated parent (T2); the request carries narrowing
    // restrictions only.
    let now = Utc::now();
    let ctx = fabric_token::VerificationContext::new(
        &MlDsa87Verifier,
        &s.token_public_key,
        now,
        fabric_token::Operation::Attenuate,
    )
    .expect_tenant(&principal.tenant_id)
    // T6: a legacy (v1) parent — one whose bundle is not the bridge's current
    // version — is refused attenuation (LegacyAttenuationDenied → 422).
    .require_current_bundle(s.bridge.bundle_version());

    let Some(policy) = s.policy.policy_for(&principal.tenant_id) else {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "no attenuation policy for this tenant",
        ));
    };
    fabric_token::verify_in_context(&req.parent, &ctx).map_err(map_attenuate_error)?;
    if ctx.is_legacy(&req.parent) {
        return Err(map_attenuate_error(
            fabric_token::TokenError::LegacyAttenuationDenied(
                req.parent.trust_bundle_version.clone(),
            ),
        ));
    }
    if !s.attenuation.claim_child_id(&req.restrictions.new_token_id) {
        return Err(map_attenuate_error(
            fabric_token::TokenError::InvalidChildId,
        ));
    }
    let current_parent = s.attenuation.current_parent(&req.parent);
    let result = fabric_token::attenuate_preverified(
        &current_parent,
        &req.restrictions,
        now,
        Some(policy.max_delegation_depth),
        s.bridge.signer(),
    );
    let token = match result {
        Ok(token) => token,
        Err(error) => {
            s.attenuation
                .release_child_id(&req.restrictions.new_token_id);
            return Err(map_attenuate_error(error));
        }
    };
    Ok(Json(TokenResp { token }))
}

/// Map a token-attenuation error to an HTTP status (plan T3): parent-
/// authentication failures are 403 (the caller presented a parent it may not
/// use); narrowing / id / depth failures are 422 (a malformed request).
fn map_attenuate_error(e: fabric_token::TokenError) -> ApiError {
    use fabric_token::TokenError as E;
    let status = match e {
        E::Revoked
        | E::InvalidSignature
        | E::MalformedSignature
        | E::Expired
        | E::NotYetValid
        | E::TenantMismatch
        | E::BundleMismatch
        | E::RevocationUnknown
        | E::UnsupportedTokenVersion(_) => StatusCode::FORBIDDEN,
        E::AttenuationWidens { .. }
        | E::InvalidChildId
        | E::DepthExceeded
        | E::LegacyAttenuationDenied(_)
        | E::BadTimestamp(_)
        | E::BudgetExceeded { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        E::Serialize(_) | E::Sign(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    ApiError::new(status, e.to_string())
}

async fn seal(
    State(s): State<AppState>,
    Extension(principal): Extension<WsfPrincipal>,
    Json(req): Json<SealReq>,
) -> Result<Json<SealResp>, ApiError> {
    let context = require_request_context(
        &principal,
        RequestOperation::WsfSeal,
        "envelope",
        &req.envelope_id,
        Some(req.token.tenant_id.clone()),
    )?;
    require_current_token(&s, &context, &req.token)?;
    // Bind the presented token to the authenticated principal's tenant (parity
    // with `exchange`): the seal plane must not let a principal seal under a
    // token minted for a different tenant.
    enforce_token_tenant(&context, RequestOperation::WsfSeal, &req.token)?;
    let plaintext = b64_decode(&req.plaintext_b64)?;
    let before = s.seal.receipts_snapshot().len();
    let result = s
        .seal
        .seal(
            SealRequest {
                token: req.token,
                plaintext,
                label: req.label,
                envelope_id: req.envelope_id,
            },
            Utc::now(),
        )
        .await;
    ingest_new_seal_receipts(&s, before);
    let envelope = result.map_err(map_seal_error)?;
    Ok(Json(SealResp { envelope }))
}

async fn unseal(
    State(s): State<AppState>,
    Extension(principal): Extension<WsfPrincipal>,
    Json(req): Json<UnsealReq>,
) -> Result<Json<UnsealResp>, ApiError> {
    let context = require_request_context(
        &principal,
        RequestOperation::WsfUnseal,
        "envelope",
        &req.envelope.envelope_id,
        Some(req.token.tenant_id.clone()),
    )?;
    require_current_token(&s, &context, &req.token)?;
    // Same tenant binding as `seal`: a principal may only unseal with a token
    // belonging to its own tenant.
    enforce_token_tenant(&context, RequestOperation::WsfUnseal, &req.token)?;
    let before = s.seal.receipts_snapshot().len();
    let result = s
        .seal
        .unseal(
            UnsealRequest {
                token: req.token,
                envelope: req.envelope,
            },
            Utc::now(),
        )
        .await;
    ingest_new_seal_receipts(&s, before);
    let plaintext = result.map_err(map_seal_error)?;
    Ok(Json(UnsealResp {
        plaintext_b64: base64::engine::general_purpose::STANDARD.encode(plaintext),
    }))
}

async fn exchange(
    State(s): State<AppState>,
    Extension(principal): Extension<WsfPrincipal>,
    Json(req): Json<ExchangeReq>,
) -> Result<Json<ExchangeResp>, ApiError> {
    let context = require_request_context(
        &principal,
        RequestOperation::WsfBroker,
        "grant",
        &req.grant_id,
        Some(req.token.tenant_id.clone()),
    )?;
    require_current_token(&s, &context, &req.token)?;
    // B1/B2: the cloud identity is resolved server-side from a tenant-scoped
    // grant — the caller never names a role ARN. A grant is scoped to the
    // authenticated principal's tenant, and the presented token must belong to
    // that same tenant (no cross-tenant brokering) — the same binding seal and
    // unseal enforce.
    enforce_token_tenant(&context, RequestOperation::WsfBroker, &req.token)?;
    let principal = context.principal();
    let grant = s
        .grants
        .grant_for(&principal.tenant_id, &req.grant_id)
        .ok_or_else(|| {
            ApiError::new(StatusCode::FORBIDDEN, "no such cloud grant for this tenant")
        })?;

    // B3: the broker binds the grant's full scope — approved actions (never
    // `Action:"*"`), region, external id, TTL ceiling — not just the role ARN.
    let creds = s
        .broker
        .broker_credentials(
            &req.token,
            &MlDsa87Verifier,
            &s.token_public_key,
            &grant.to_scope(),
            Utc::now(),
        )
        .await
        .map_err(|e| match e {
            wsf_broker::BrokerError::TokenRejected(_)
            | wsf_broker::BrokerError::TokenExpired
            | wsf_broker::BrokerError::Grant(_) => {
                ApiError::new(StatusCode::FORBIDDEN, e.to_string())
            }
            _ => ApiError::new(StatusCode::BAD_GATEWAY, e.to_string()),
        })?;
    Ok(Json(ExchangeResp {
        access_key_id: creds.access_key_id,
        secret_access_key: creds.secret_access_key,
        session_token: creds.session_token,
        expiration: creds.expiration.to_rfc3339(),
    }))
}

/// Maximum receipts returned per query (plan L2 — bounded results).
const RECEIPTS_LIMIT: usize = 500;

async fn receipts(
    State(s): State<AppState>,
    Extension(principal): Extension<WsfPrincipal>,
    Query(q): Query<ReceiptsQuery>,
) -> Result<Json<ReceiptsResp>, ApiError> {
    let context = require_request_context(
        &principal,
        RequestOperation::WsfAuditRead,
        "receipt-ledger",
        "tenant-view",
        Some(principal.tenant_id.clone()),
    )?;
    require_current_principal(&s, &context)?;
    let principal = context.principal();
    // L1/L2: the query is authenticated (principal established by the A2
    // middleware) and **mandatorily tenant-scoped** to the caller's tenant. A
    // receipt is returned only if it carries a `tenant_id` equal to the
    // principal's. Cross-tenant identifier guessing therefore returns no rows
    // and no existence oracle. Receipts without a tenant binding are
    // withheld from tenant-scoped reads (fail closed).
    if !principal.is_for(Audience::Wsf) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "principal is not authenticated for the WSF plane",
        ));
    }
    // L2: a server-enrolled global auditor is the one exception to tenant
    // scoping — enrollment is by authenticated principal_id, never by anything
    // in the request.
    let auditor = s.auditors.is_global_auditor(principal);
    let ledger = s.ledger.lock().expect("ledger lock");

    // Optional typed field filter — always intersected with the tenant scope,
    // so it can never widen beyond the caller's tenant.
    let base: Vec<&LedgerEntry> = match (q.field.as_deref(), q.value.as_deref()) {
        (Some(field), Some(value)) => ledger.query(field, value),
        _ => ledger.entries().iter().collect(),
    };
    let entries: Vec<LedgerEntry> = base
        .into_iter()
        .filter(|e| {
            auditor
                || e.receipt.get("tenant_id").and_then(|v| v.as_str())
                    == Some(principal.tenant_id.as_str())
        })
        .take(RECEIPTS_LIMIT)
        .cloned()
        .collect();
    Ok(Json(ReceiptsResp { entries }))
}

/// L4: export the signed evidence pack over the full ledger. Global auditors
/// only — the pack is cross-tenant by nature (the whole chain), and its ML-DSA
/// signature verifies offline via `wsf_ledger::verify_pack`.
async fn export_receipts(
    State(s): State<AppState>,
    Extension(principal): Extension<WsfPrincipal>,
) -> Result<Json<wsf_ledger::EvidencePack>, ApiError> {
    let context = require_request_context(
        &principal,
        RequestOperation::WsfAuditExport,
        "receipt-ledger",
        "estate-export",
        None,
    )?;
    require_current_principal(&s, &context)?;
    let principal = context.principal();
    if !principal.is_for(Audience::Wsf) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "principal is not authenticated for the WSF plane",
        ));
    }
    if !s.auditors.is_global_auditor(principal) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "receipt export requires global-auditor enrollment",
        ));
    }
    let pack = s
        .ledger
        .lock()
        .expect("ledger lock")
        .export_pack(Utc::now().to_rfc3339())
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(pack))
}

async fn openapi() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], OPENAPI_JSON)
}

fn map_seal_error(e: wsf_seal::SealError) -> ApiError {
    match e {
        wsf_seal::SealError::Unauthorized(_) => ApiError::new(StatusCode::FORBIDDEN, e.to_string()),
        wsf_seal::SealError::OpenBao(_) => ApiError::new(StatusCode::BAD_GATEWAY, e.to_string()),
        _ => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Ingest any seal receipts produced since `before` into the ledger.
fn ingest_new_seal_receipts(s: &AppState, before: usize) {
    let receipts = s.seal.receipts_snapshot();
    let mut ledger = s.ledger.lock().expect("ledger lock");
    for r in receipts.iter().skip(before) {
        if let Ok(value) = serde_json::to_value(r) {
            let _ = ledger.ingest("wsf-seal", value);
        }
    }
}

#[cfg(test)]
mod issue_authz_tests {
    use super::*;

    fn ceiling() -> Budget {
        Budget {
            token_cap: 1000,
            usd_cap_cents: 500,
            tool_call_cap: 10,
            ..Budget::default()
        }
    }

    #[test]
    fn omitted_budget_is_granted_the_ceiling_not_unlimited() {
        let b = authorize_budget(None, &ceiling()).unwrap();
        assert_eq!(b.token_cap, 1000);
        assert_eq!(b.usd_cap_cents, 500);
        assert_eq!(b.tool_call_cap, 10);
        assert_eq!(b.tokens_spent, 0);
    }

    #[test]
    fn within_ceiling_is_granted_with_spent_reset() {
        let req = Budget {
            token_cap: 400,
            tokens_spent: 999, // caller-supplied spent is ignored
            usd_cap_cents: 100,
            tool_call_cap: 3,
            ..Budget::default()
        };
        let b = authorize_budget(Some(&req), &ceiling()).unwrap();
        assert_eq!(b.token_cap, 400);
        assert_eq!(b.tokens_spent, 0, "spent counters always reset");
        assert_eq!(b.tool_call_cap, 3);
    }

    #[test]
    fn each_over_ceiling_counter_is_denied() {
        for over in [
            Budget {
                token_cap: 1001,
                ..ceiling()
            },
            Budget {
                usd_cap_cents: 501,
                ..ceiling()
            },
            Budget {
                tool_call_cap: 11,
                ..ceiling()
            },
        ] {
            let err = authorize_budget(Some(&over), &ceiling()).unwrap_err();
            assert_eq!(err.status, StatusCode::FORBIDDEN);
        }
    }

    #[test]
    fn issue_req_rejects_smuggled_authority_fields() {
        // Bounded intent parses.
        let ok: Result<IssueReq, _> = serde_json::from_str(r#"{"requested_roles":["user"]}"#);
        assert!(ok.is_ok());
        // Any attempt to smuggle authority is a hard parse error (deny_unknown_fields),
        // not a silently-ignored field — the structural half of the fix.
        for smuggle in [
            r#"{"tenant_id":"victim","requested_roles":["user"]}"#,
            r#"{"subject_id":"someone-else"}"#,
            r#"{"roles":["admin"]}"#,
            r#"{"allowed_models":["*"]}"#,
        ] {
            assert!(
                serde_json::from_str::<IssueReq>(smuggle).is_err(),
                "must reject smuggled authority: {smuggle}"
            );
        }
    }
}

#[cfg(test)]
mod tenant_binding_tests {
    use super::*;
    use fabric_contracts::{
        Attenuation, AuthStrength, AuthenticatedFacts, Classification, IdentityKind,
        RevocationStatus, Signature,
    };

    fn principal(tenant: &str) -> WsfPrincipal {
        WsfPrincipal::establish(
            AuthenticatedFacts {
                principal_id: "p".into(),
                kind: IdentityKind::Workload,
                tenant_id: tenant.into(),
                subject_hash: String::new(),
                service_identity: None,
                roles: Vec::new(),
                token_lineage: None,
                auth_strength: AuthStrength::WorkloadToken,
                audience: Audience::Wsf,
            },
            "corr-1".to_string(),
            "2026-07-10T00:00:00Z".to_string(),
        )
    }

    fn token(tenant: &str) -> TrustToken {
        TrustToken {
            token_id: "t".into(),
            issued_at: "2026-07-10T00:00:00Z".into(),
            expires_at: "2099-01-01T00:00:00Z".into(),
            issuer: "wsf-bridge".into(),
            trust_bundle_version: "2026.07".into(),
            tenant_id: tenant.into(),
            subject_id: None,
            subject_hash: "h".into(),
            service_identity: None,
            identity_id: None,
            roles: vec![],
            compliance_scopes: vec![],
            allowed_routes: vec![],
            allowed_models: vec![],
            max_data_classification: Classification::Restricted,
            country: None,
            person_type: None,
            offline_mode: false,
            revocation_status: RevocationStatus::Valid,
            budget: None,
            attenuation: Attenuation::default(),
            signature: Signature {
                alg: String::new(),
                key_id: String::new(),
                value: String::new(),
            },
        }
    }

    #[test]
    fn seal_unseal_refuse_a_cross_tenant_token() {
        // The seal/unseal/exchange handlers all route through
        // `enforce_token_tenant`; a token minted for another tenant is refused
        // 403 at the handler, never sealed/unsealed under the caller's identity.
        let alice = principal("tenant-a");
        let context = require_request_context(
            &alice,
            RequestOperation::WsfSeal,
            "envelope",
            "env-1",
            Some("tenant-a".into()),
        )
        .unwrap();
        // Same tenant: allowed.
        assert!(
            enforce_token_tenant(&context, RequestOperation::WsfSeal, &token("tenant-a")).is_ok()
        );
        // Cross-tenant token: 403.
        let err = enforce_token_tenant(&context, RequestOperation::WsfSeal, &token("tenant-b"))
            .unwrap_err();
        assert_eq!(err.status, StatusCode::FORBIDDEN);
        // A context established for another operation cannot reach this sink.
        let err = enforce_token_tenant(&context, RequestOperation::WsfUnseal, &token("tenant-a"))
            .unwrap_err();
        assert_eq!(err.status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn attenuation_state_uses_root_spend_and_rejects_duplicate_child_ids() {
        let mut parent = token("tenant-a");
        parent.token_id = "root-a".into();
        parent.budget = Some(Budget {
            token_cap: 100,
            usd_cap_cents: 100,
            tool_call_cap: 10,
            ..Budget::default()
        });
        let state = AttenuationState::new();
        state
            .commit_spend(
                &parent,
                fabric_token::spend::Spent {
                    tokens: 40,
                    usd_cents: 25,
                    tool_calls: 2,
                },
            )
            .unwrap();
        let current = state.current_parent(&parent);
        let budget = current.budget.unwrap();
        assert_eq!(budget.tokens_spent, 40);
        assert_eq!(budget.usd_spent_cents, 25);
        assert_eq!(budget.tool_calls_spent, 2);

        assert!(state.claim_child_id("child-a"));
        assert!(!state.claim_child_id("child-a"));
    }

    #[test]
    fn a_non_wsf_principal_is_refused() {
        // A principal authenticated for a different plane (AOG) is refused on
        // the WSF seal/unseal/exchange handlers even with a matching tenant.
        let aog_principal = WsfPrincipal::establish(
            AuthenticatedFacts {
                principal_id: "p".into(),
                kind: IdentityKind::Workload,
                tenant_id: "tenant-a".into(),
                subject_hash: String::new(),
                service_identity: None,
                roles: Vec::new(),
                token_lineage: None,
                auth_strength: AuthStrength::WorkloadToken,
                audience: Audience::Aog,
            },
            "corr-1".to_string(),
            "2026-07-10T00:00:00Z".to_string(),
        );
        let err = require_request_context(
            &aog_principal,
            RequestOperation::WsfSeal,
            "envelope",
            "env-1",
            Some("tenant-a".into()),
        )
        .unwrap_err();
        assert_eq!(err.status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn forged_resource_tenant_is_refused_before_the_sink() {
        let alice = principal("tenant-a");
        let err = require_request_context(
            &alice,
            RequestOperation::WsfUnseal,
            "envelope",
            "env-foreign",
            Some("tenant-b".into()),
        )
        .unwrap_err();
        assert_eq!(err.status, StatusCode::FORBIDDEN);
    }
}
