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

use axum::extract::{Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router, middleware};
use base64::Engine;
use chrono::Utc;
use fabric_contracts::{Budget, Envelope, TrustToken, WsfPrincipal};
use fabric_crypto::providers::MlDsa87Verifier;
use serde::{Deserialize, Serialize};
use wsf_bridge::{IssueTokenRequest, TrustBridge};

use crate::auth::{AuthError, WsfAuthenticator};
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
    /// Front-door authenticator for privileged issuance (AF-002). Fail-closed:
    /// a request without a verified `WsfPrincipal` cannot mint a token.
    pub authenticator: Arc<dyn WsfAuthenticator>,
}

/// Mount all routes over `state`. `/v1/tokens/issue` is gated by the issuance
/// authenticator — the token's tenant/subject/roles come from the verified
/// principal, never the request body (AF-002).
pub fn router(state: AppState) -> Router {
    let issue_route = post(issue).route_layer(middleware::from_fn_with_state(
        state.clone(),
        require_principal,
    ));
    Router::new()
        .route("/v1/tokens/issue", issue_route)
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

/// Authenticate a privileged issuance request into a `WsfPrincipal` before its
/// handler runs, stashing it in request extensions. Refuses (401/403) when
/// identity is missing, unverifiable, expired, or not permitted.
async fn require_principal(
    State(s): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let principal = s
        .authenticator
        .authenticate(req.headers(), Utc::now())
        .map_err(|e| match e {
            AuthError::Unauthenticated => {
                ApiError::new(StatusCode::UNAUTHORIZED, "unauthenticated")
            }
            AuthError::Forbidden => ApiError::new(StatusCode::FORBIDDEN, "forbidden"),
        })?;
    req.extensions_mut().insert(principal);
    Ok(next.run(req).await)
}

// ── request / response DTOs (shared with the SDK) ──────────────────────

/// Issue-token request. Identity (tenant / subject / roles) is **not** accepted
/// here — it is copied from the authenticated `WsfPrincipal` (AF-002). The body
/// may only *narrow*: an optional model allowlist and a budget below the tenant
/// ceiling.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssueReq {
    /// Optional budget strand (narrowing intent; capped by tenant policy).
    #[serde(default)]
    pub budget: Option<Budget>,
    /// Optional model allowlist (narrowing intent).
    #[serde(default)]
    pub allowed_models: Vec<String>,
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
    Extension(principal): Extension<WsfPrincipal>,
    Json(req): Json<IssueReq>,
) -> Result<Json<TokenResp>, ApiError> {
    // Identity (tenant / subject / roles) comes from the authenticated principal,
    // never the request body (AF-002). The body may only narrow (models, budget).
    let ir = IssueTokenRequest::new(principal.tenant_id, principal.subject_id, principal.roles)
        .with_models(req.allowed_models);
    let ir = if let Some(b) = req.budget {
        ir.with_budget(b)
    } else {
        ir
    };
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
    // The parent must authenticate under the trust anchor before it can mint a
    // child (AF-001): a fabricated / wrong-key / expired / revoked parent, or a
    // child that widens any authority axis, is refused here — never signed.
    let ctx = fabric_token::VerificationContext::new(
        &MlDsa87Verifier,
        s.token_public_key.as_slice(),
        Utc::now(),
    );
    let token = fabric_token::attenuate(&req.parent, req.child, &ctx, s.bridge.signer())
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
