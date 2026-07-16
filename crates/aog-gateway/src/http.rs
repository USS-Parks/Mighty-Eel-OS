//! HTTP surface for the AOG gateway (axum 0.8).
//!
//! G1 lights up the **skeleton**: a caller presents a virtual key as an
//! `Authorization: Bearer <key>` header (exactly how OpenAI/Anthropic clients
//! present an API key), and the gateway resolves it to a verified, in-budget
//! [`crate::ResolvedContext`]. `GET /healthz` is an unauthenticated liveness
//! probe; `POST /v1/preflight` runs the full resolve-and-check and reports the
//! tenant + the authorization decision. An unknown / unverifiable key returns
//! `401`; an over-budget token returns `402` **before any model is touched**.
//!
//! The inference routes (`/v1/chat/completions` in G3, `/v1/messages` in G4)
//! layer onto this same router + [`Gateway`] state — G1 owns the auth seam they
//! all funnel through.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    routing::{get, post},
};
use chrono::Utc;
use serde::Serialize;

use crate::{Gateway, GatewayError};

/// Build the gateway router over a shared [`Gateway`].
pub fn router(gateway: Arc<Gateway>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/preflight", post(preflight_handler))
        .with_state(gateway)
}

async fn healthz() -> &'static str {
    "ok"
}

/// The pre-flight decision returned to an authorized caller.
#[derive(Debug, Serialize)]
pub struct PreflightResponse {
    /// The tenant the virtual key belongs to.
    pub tenant_id: String,
    /// Always `true` on a `200` — the key resolved, verified off-host, and had
    /// budget room. A refusal never reaches this body (it is an HTTP error).
    pub authorized: bool,
}

/// Pull the bearer virtual key out of the `Authorization` header.
pub(crate) fn bearer_key(headers: &HeaderMap) -> Result<&str, (StatusCode, String)> {
    let raw = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "missing authorization header".to_string(),
        ))?;
    let key = raw
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "authorization must be `Bearer <virtual-key>`".to_string(),
        ))?;
    crate::validate_virtual_key(key).map_err(|error| to_http(&error))?;
    Ok(key)
}

/// Map a [`GatewayError`] to an HTTP status + message.
///
/// `402 Payment Required` is the honest code for an exhausted budget: the key is
/// valid, the caller has simply spent its allocation and must renew/top up.
pub(crate) fn to_http(err: &GatewayError) -> (StatusCode, String) {
    let msg = err.to_string();
    match err {
        GatewayError::UnknownKey | GatewayError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, msg),
        GatewayError::BudgetExhausted => (StatusCode::PAYMENT_REQUIRED, msg),
        GatewayError::Revoked => (StatusCode::FORBIDDEN, msg),
        GatewayError::Malformed(_) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        GatewayError::OpenBao(_) => (StatusCode::BAD_GATEWAY, msg),
        GatewayError::AdmissionLimited => (StatusCode::TOO_MANY_REQUESTS, msg),
    }
}

async fn preflight_handler(
    State(gw): State<Arc<Gateway>>,
    headers: HeaderMap,
) -> Result<Json<PreflightResponse>, (StatusCode, String)> {
    let key = bearer_key(&headers)?;
    let ctx = gw
        .resolve_and_check(key, Utc::now())
        .await
        .map_err(|e| to_http(&e))?;
    Ok(Json(PreflightResponse {
        tenant_id: ctx.tenant_id,
        authorized: true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_extraction() {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, "Bearer vk_abc123".parse().unwrap());
        assert_eq!(bearer_key(&h).unwrap(), "vk_abc123");
    }

    #[test]
    fn bearer_missing_or_malformed_rejected() {
        // No header at all.
        assert_eq!(
            bearer_key(&HeaderMap::new()).unwrap_err().0,
            StatusCode::UNAUTHORIZED
        );
        // Wrong scheme.
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, "Basic Zm9v".parse().unwrap());
        assert_eq!(bearer_key(&h).unwrap_err().0, StatusCode::UNAUTHORIZED);
        // Empty key after the scheme.
        let mut h2 = HeaderMap::new();
        h2.insert(AUTHORIZATION, "Bearer   ".parse().unwrap());
        assert_eq!(bearer_key(&h2).unwrap_err().0, StatusCode::UNAUTHORIZED);
        for malformed in [
            "Bearer vk_bad key",
            "Bearer vk_bad/key",
            "Bearer vk_bad=key",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(AUTHORIZATION, malformed.parse().unwrap());
            assert_eq!(
                bearer_key(&headers).unwrap_err().0,
                StatusCode::UNAUTHORIZED
            );
        }
        let mut oversized = HeaderMap::new();
        oversized.insert(
            AUTHORIZATION,
            format!("Bearer {}", "a".repeat(129)).parse().unwrap(),
        );
        assert_eq!(
            bearer_key(&oversized).unwrap_err().0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn error_status_mapping() {
        assert_eq!(
            to_http(&GatewayError::UnknownKey).0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            to_http(&GatewayError::Unauthorized("expired".into())).0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            to_http(&GatewayError::BudgetExhausted).0,
            StatusCode::PAYMENT_REQUIRED
        );
        assert_eq!(to_http(&GatewayError::Revoked).0, StatusCode::FORBIDDEN);
        assert_eq!(
            to_http(&GatewayError::AdmissionLimited).0,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            to_http(&GatewayError::Malformed("bad".into())).0,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
