//! HTTP surface for the seal service (axum 0.8). `POST /seal` and
//! `POST /unseal`; binary fields travel base64. An unauthorized unseal returns
//! `403 Forbidden` (and the service has already receipted the denial).

use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use base64::Engine;
use chrono::Utc;
use fabric_contracts::{Envelope, TrustToken};
use serde::{Deserialize, Serialize};

use crate::{LabelSpec, SealError, SealRequest, SealService, UnsealRequest};

/// Build the seal-service router over a shared [`SealService`].
pub fn router(service: Arc<SealService>) -> Router {
    Router::new()
        .route("/seal", post(seal_handler))
        .route("/unseal", post(unseal_handler))
        .with_state(service)
}

#[derive(Deserialize)]
struct SealHttpRequest {
    token: TrustToken,
    plaintext_b64: String,
    label: LabelSpec,
    envelope_id: String,
}

#[derive(Serialize)]
struct SealHttpResponse {
    envelope: Envelope,
}

#[derive(Deserialize)]
struct UnsealHttpRequest {
    token: TrustToken,
    envelope: Envelope,
}

#[derive(Serialize)]
struct UnsealHttpResponse {
    plaintext_b64: String,
}

fn b64_decode(s: &str) -> Result<Vec<u8>, (StatusCode, String)> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad base64: {e}")))
}

fn to_http(err: SealError) -> (StatusCode, String) {
    match err {
        SealError::Unauthorized(m) => (StatusCode::FORBIDDEN, m),
        other => (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
    }
}

async fn seal_handler(
    State(svc): State<Arc<SealService>>,
    Json(req): Json<SealHttpRequest>,
) -> Result<Json<SealHttpResponse>, (StatusCode, String)> {
    let plaintext = b64_decode(&req.plaintext_b64)?;
    let envelope = svc
        .seal(
            SealRequest {
                token: req.token,
                plaintext,
                label: req.label,
                envelope_id: req.envelope_id,
            },
            Utc::now(),
        )
        .await
        .map_err(to_http)?;
    Ok(Json(SealHttpResponse { envelope }))
}

async fn unseal_handler(
    State(svc): State<Arc<SealService>>,
    Json(req): Json<UnsealHttpRequest>,
) -> Result<Json<UnsealHttpResponse>, (StatusCode, String)> {
    let plaintext = svc
        .unseal(
            UnsealRequest {
                token: req.token,
                envelope: req.envelope,
            },
            Utc::now(),
        )
        .await
        .map_err(to_http)?;
    Ok(Json(UnsealHttpResponse {
        plaintext_b64: base64::engine::general_purpose::STANDARD.encode(plaintext),
    }))
}
