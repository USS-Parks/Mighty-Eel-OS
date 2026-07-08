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

use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use chrono::Utc;
use fabric_contracts::{Budget, Envelope, TrustToken};
use fabric_crypto::providers::MlDsa87Verifier;
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
    /// Authenticates callers to a server-side [`auth::WsfPrincipal`]; fails closed
    /// by default ([`auth::DenyAllAuthenticator`]).
    pub authenticator: Arc<dyn auth::Authenticator>,
    /// Per-principal issuance rate limiter.
    pub rate_limiter: Arc<auth::RateLimiter>,
}

/// Mount all routes over `state`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/tokens/issue", post(issue))
        .route("/v1/tokens/verify", post(verify))
        .route("/v1/tokens/attenuate", post(attenuate))
        .route("/v1/envelopes/seal", post(seal))
        .route("/v1/envelopes/unseal", post(unseal))
        .route("/v1/credentials/exchange", post(exchange))
        .route("/v1/receipts", get(receipts))
        .route("/openapi.json", get(openapi))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state)
}

// ── request / response DTOs (shared with the SDK) ──────────────────────

/// Issue-token request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueReq {
    /// Tenant.
    pub tenant_id: String,
    /// Cleartext subject (pseudonymized by the bridge).
    pub subject_id: String,
    /// Roles.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Optional budget strand.
    #[serde(default)]
    pub budget: Option<Budget>,
    /// Optional model allowlist (narrows the principal's allowlist).
    #[serde(default)]
    pub allowed_models: Vec<String>,
    /// Issuance kind: `self` | `delegated` (default) | `service`.
    #[serde(default)]
    pub issuance_kind: Option<String>,
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

// ── handlers ───────────────────────────────────────────────────────────

async fn issue(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<IssueReq>,
) -> Result<Json<TokenResp>, ApiError> {
    // 1. Authenticate — fail closed. No principal ⇒ no signing.
    let principal = match s.authenticator.authenticate(&headers) {
        Ok(p) => p,
        Err(e) => {
            receipt_issue_denied(&s, "-", "unauthenticated", &e.message());
            return Err(ApiError::new(e.status(), e.message()));
        }
    };

    // 2. Per-principal issuance rate limit.
    if !s
        .rate_limiter
        .check(&principal.service_identity, Utc::now().timestamp())
    {
        receipt_issue_denied(&s, &principal.service_identity, "rate_limited", "");
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "issuance rate limit exceeded".to_string(),
        ));
    }

    // 3. Derive the authority to sign — the body may only NARROW the principal.
    let kind = match req.issuance_kind.as_deref() {
        None => auth::IssuanceKind::Delegated,
        Some(k) => auth::IssuanceKind::parse(k).ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("unknown issuance_kind '{k}'"),
            )
        })?,
    };
    let ireq = auth::IssuanceRequest {
        kind,
        requested_tenant: (!req.tenant_id.is_empty()).then_some(req.tenant_id.as_str()),
        subject_id: &req.subject_id,
        requested_roles: &req.roles,
        requested_budget: req.budget.as_ref(),
        requested_models: &req.allowed_models,
    };
    let authority = match auth::derive_issue_authority(&principal, &ireq) {
        Ok(a) => a,
        Err(e) => {
            receipt_issue_denied(
                &s,
                &principal.service_identity,
                "authority_denied",
                &e.message(),
            );
            return Err(ApiError::new(e.status(), e.message()));
        }
    };

    // 4. Sign with the SERVER-DERIVED authority — never a caller-authored field.
    let mut ir = IssueTokenRequest::new(
        authority.tenant_id.clone(),
        authority.subject_id.clone(),
        authority.roles.clone(),
    )
    .with_models(authority.allowed_models.clone());
    if let Some(b) = authority.budget.clone() {
        ir = ir.with_budget(b);
    }
    let token = s.bridge.issue_token(&ir).await.map_err(|e| match e {
        wsf_bridge::BridgeError::OpenBao(_) => {
            ApiError::new(StatusCode::BAD_GATEWAY, e.to_string())
        }
        wsf_bridge::BridgeError::Config(_) => {
            ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, e.to_string())
        }
        _ => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    })?;

    // 5. Receipt the allow (metadata only), then the bridge correlation.
    receipt_issue_allowed(&s, &principal, &authority);
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

/// Receipt a denied issuance (metadata only — never token or credential material).
fn receipt_issue_denied(s: &AppState, identity: &str, reason: &str, detail: &str) {
    let value = serde_json::json!({
        "event": "issue_denied",
        "service_identity": identity,
        "reason": reason,
        "detail": detail,
    });
    let _ = s
        .ledger
        .lock()
        .expect("ledger lock")
        .ingest("wsf-api-auth", value);
}

/// Receipt an allowed issuance (server-derived authority; no token material).
fn receipt_issue_allowed(
    s: &AppState,
    principal: &auth::WsfPrincipal,
    authority: &auth::DerivedAuthority,
) {
    let value = serde_json::json!({
        "event": "issue_allowed",
        "service_identity": principal.service_identity,
        "tenant": authority.tenant_id,
        "audience": authority.audience,
        "roles": authority.roles,
    });
    let _ = s
        .ledger
        .lock()
        .expect("ledger lock")
        .ingest("wsf-api-auth", value);
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
