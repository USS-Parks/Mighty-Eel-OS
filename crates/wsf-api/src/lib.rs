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

pub mod auth;
pub mod client;
pub mod policy;

use std::sync::{Arc, Mutex};

use axum::extract::{Extension, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use fabric_contracts::{Audience, Budget, Envelope, TrustToken, WsfPrincipal};

use auth::{WsfAuthenticator, require_principal};
use base64::Engine;
use chrono::Utc;
use fabric_crypto::providers::MlDsa87Verifier;
use policy::TenantPolicyStore;
use serde::{Deserialize, Serialize};
use wsf_bridge::{IssueTokenRequest, TrustBridge};
use wsf_broker::AwsStsBroker;
use wsf_ledger::{Ledger, LedgerEntry};
use wsf_seal::{LabelSpec, SealRequest, SealService, UnsealRequest};

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
/// a 422, not a silent override (closes AF-002).
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

/// Attenuate request (mint a narrower child of `parent`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttenuateReq {
    /// The parent token.
    pub parent: TrustToken,
    /// The desired child (must narrow the parent on every axis).
    pub child: TrustToken,
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

/// Credential-exchange request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeReq {
    /// The verified trust token.
    pub token: TrustToken,
    /// The cloud role ARN to assume.
    pub role_arn: String,
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
    // Authority is derived from the authenticated principal + server-side tenant
    // policy — never from the request body (plan A3, closes AF-002).
    if !principal.is_for(Audience::Wsf) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "principal is not authenticated for the WSF plane",
        ));
    }
    let policy = s.policy.policy_for(&principal.tenant_id).ok_or_else(|| {
        ApiError::new(StatusCode::FORBIDDEN, "no issuance policy for this tenant")
    })?;

    // Roles: requested must be a subset of what the tenant may grant.
    for role in &req.requested_roles {
        if !policy.may_grant_role(role) {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "requested role is not grantable for this tenant",
            ));
        }
    }
    // Models: requested must lie within the allowlist when the tenant restricts.
    for model in &req.requested_models {
        if !policy.allows_model(model) {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "requested model is not permitted for this tenant",
            ));
        }
    }
    // Budget: every counter at or below the ceiling; unspecified ⇒ the ceiling.
    let budget = authorize_budget(req.budget.as_ref(), &policy.max_budget)?;

    // Subject is the authenticated principal, never a caller-supplied id.
    let subject_source = if principal.subject_hash.is_empty() {
        principal.principal_id.clone()
    } else {
        principal.subject_hash.clone()
    };

    let ir = IssueTokenRequest::new(
        principal.tenant_id.clone(),
        subject_source,
        req.requested_roles.clone(),
    )
    .with_models(req.requested_models.clone())
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

async fn verify(State(s): State<AppState>, Json(req): Json<VerifyReq>) -> Json<VerifyResp> {
    if let Err(e) = fabric_token::verify(&req.token, &MlDsa87Verifier, &s.token_public_key) {
        return Json(VerifyResp {
            valid: false,
            reason: e.to_string(),
        });
    }
    match fabric_token::is_expired(&req.token, Utc::now()) {
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
    }
}

async fn attenuate(
    State(s): State<AppState>,
    Json(req): Json<AttenuateReq>,
) -> Result<Json<TokenResp>, ApiError> {
    let token = fabric_token::attenuate(&req.parent, req.child, s.bridge.signer())
        .map_err(|e| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    Ok(Json(TokenResp { token }))
}

async fn seal(
    State(s): State<AppState>,
    Json(req): Json<SealReq>,
) -> Result<Json<SealResp>, ApiError> {
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
    Json(req): Json<UnsealReq>,
) -> Result<Json<UnsealResp>, ApiError> {
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
    Json(req): Json<ExchangeReq>,
) -> Result<Json<ExchangeResp>, ApiError> {
    let creds = s
        .broker
        .broker_credentials(
            &req.token,
            &MlDsa87Verifier,
            &s.token_public_key,
            &req.role_arn,
            Utc::now(),
        )
        .await
        .map_err(|e| match e {
            wsf_broker::BrokerError::TokenRejected(_) | wsf_broker::BrokerError::TokenExpired => {
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

async fn receipts(State(s): State<AppState>, Query(q): Query<ReceiptsQuery>) -> Json<ReceiptsResp> {
    let ledger = s.ledger.lock().expect("ledger lock");
    let entries = match (q.field, q.value) {
        (Some(field), Some(value)) => ledger.query(&field, &value).into_iter().cloned().collect(),
        _ => ledger.entries().to_vec(),
    };
    Json(ReceiptsResp { entries })
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
        // not a silently-ignored field — the structural half of the AF-002 fix.
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
