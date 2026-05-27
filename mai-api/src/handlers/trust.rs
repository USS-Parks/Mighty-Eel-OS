//! Trust Manifold handlers (BF-6).
//!
//! Wires the local trust cache (BF-4) and the in-process token
//! exchange stub onto the public REST surface so the Python SDK
//! `client.trust.*` and `client.auth.exchange_token` calls have a
//! real server to talk to.
//!
//! Every response is metadata only — claims, bundle version,
//! revocation snapshot status, connectivity mode. No prompt,
//! completion, embedding, or regulated payload travels through
//! these endpoints (Trust Manifold hard rule §A.2.4).
//!
//! Endpoint summary:
//!
//! - `GET  /v1/trust/status`            — consolidated trust mode
//! - `GET  /v1/trust/claims`            — list every cached claim
//! - `GET  /v1/trust/bundle_status`     — bundle version + freshness
//! - `GET  /v1/trust/revocation_status` — single-claim lookup
//! - `POST /v1/auth/exchange_token`     — local-dev token stub

use axum::Json;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::auth::check_permission;
use crate::errors::ApiError;
use crate::state::AppState;
use crate::trust_builder::TrustExchangeMode;
use crate::types::ProfileInfo;

use mai_compliance::trust_cache::{LocalTrustCache, RevocationSnapshot, SnapshotStatus};

// ─── Wire types ────────────────────────────────────────────────────

/// One row of `GET /v1/trust/claims`.
#[derive(Debug, Serialize)]
pub struct TrustClaim {
    pub claim_id: String,
    pub status: String,
    pub recorded_at_secs: u64,
}

impl From<RevocationSnapshot> for TrustClaim {
    fn from(snap: RevocationSnapshot) -> Self {
        Self {
            claim_id: snap.claim_id,
            status: snapshot_status_label(snap.status).to_string(),
            recorded_at_secs: snap.recorded_at_secs,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TrustClaimsResponse {
    pub claims: Vec<TrustClaim>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct TrustBundleStatus {
    pub bundle_version: Option<String>,
    pub last_refresh_secs: Option<u64>,
    pub age_secs: Option<u64>,
    pub connectivity: String,
    pub is_emergency_only: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevocationQuery {
    pub claim_id: String,
}

#[derive(Debug, Serialize)]
pub struct RevocationStatusResponse {
    pub claim_id: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct TrustStatusResponse {
    pub mode: String,
    pub bundle_version: Option<String>,
    pub last_refresh_secs: Option<u64>,
    pub age_secs: Option<u64>,
    pub claim_count: usize,
    pub airgap: AirGapView,
    pub offline_backlog: usize,
}

#[derive(Debug, Serialize)]
pub struct AirGapView {
    pub connectivity: String,
    pub permits_cloud_route: bool,
    pub requires_local_only: bool,
    pub is_air_gapped: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeTokenRequest {
    pub subject_id: String,
    #[serde(default)]
    pub tenant_id: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ExchangeTokenResponse {
    pub token: String,
    pub token_type: String,
    pub subject_id: String,
    pub tenant_id: String,
    pub scopes: Vec<String>,
    pub issued_at_secs: u64,
    pub expires_at_secs: u64,
    pub mode: String,
}

// ─── Handlers ──────────────────────────────────────────────────────

/// `GET /v1/trust/status`
///
/// Consolidated trust mode for the dashboard overview. Combines the
/// cache freshness ladder with the canonical air-gap state.
pub async fn get_trust_status(
    State(state): State<AppState>,
    _profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    let cache = state.trust_cache.read().await;
    let now = LocalTrustCache::now_secs();
    let connectivity = cache.evaluate(state.airgap_policy.state(), true, now);
    let response = TrustStatusResponse {
        mode: connectivity.label().to_string(),
        bundle_version: cache.bundle_version().map(str::to_string),
        last_refresh_secs: cache.last_refresh_secs(),
        age_secs: cache.age_secs(now),
        claim_count: cache.claims().len(),
        airgap: AirGapView {
            connectivity: state.airgap_policy.state().label().to_string(),
            permits_cloud_route: state.airgap_policy.state().permits_cloud_route(),
            requires_local_only: state.airgap_policy.state().requires_local_only(),
            is_air_gapped: state.airgap_policy.state().is_air_gapped(),
        },
        offline_backlog: cache.offline_audit_backlog(),
    };
    Ok(Json(response))
}

/// `GET /v1/trust/claims`
///
/// Lists every claim currently held in the local trust cache. Admin-
/// only (`view_audit` permission) — claim ids leak tenant + subject
/// metadata that non-admin profiles must not observe.
pub async fn list_claims(
    State(state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "view_audit")?;
    let cache = state.trust_cache.read().await;
    let claims: Vec<TrustClaim> = cache.claims().into_iter().map(TrustClaim::from).collect();
    let total = claims.len();
    Ok(Json(TrustClaimsResponse { claims, total }))
}

/// `GET /v1/trust/bundle_status`
///
/// Bundle version and freshness for the dashboard. Mirrors the inputs
/// that the policy runtime sees on every decision.
pub async fn bundle_status(
    State(state): State<AppState>,
    _profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    let cache = state.trust_cache.read().await;
    let now = LocalTrustCache::now_secs();
    let connectivity = cache.evaluate(state.airgap_policy.state(), true, now);
    let response = TrustBundleStatus {
        bundle_version: cache.bundle_version().map(str::to_string),
        last_refresh_secs: cache.last_refresh_secs(),
        age_secs: cache.age_secs(now),
        connectivity: connectivity.label().to_string(),
        is_emergency_only: cache.is_emergency_only(state.airgap_policy.state(), true, now),
    };
    Ok(Json(response))
}

/// `GET /v1/trust/revocation_status?claim_id=...`
///
/// Single-claim lookup. Returns `unknown` for any claim id the cache
/// has never seen — pessimistic behaviour matching the policy runtime.
pub async fn revocation_status(
    State(state): State<AppState>,
    _profile: ProfileInfo,
    Query(q): Query<RevocationQuery>,
) -> Result<impl IntoResponse, ApiError> {
    if q.claim_id.trim().is_empty() {
        return Err(ApiError::ValidationFailed(
            "claim_id query parameter is required".to_string(),
        ));
    }
    let cache = state.trust_cache.read().await;
    let status = cache.revocation_status(&q.claim_id);
    Ok(Json(RevocationStatusResponse {
        claim_id: q.claim_id,
        status: snapshot_status_label(status).to_string(),
    }))
}

/// `POST /v1/auth/exchange_token`
///
/// Profile-aware token exchange (SHIP-07 Slice B). Behaviour switches
/// on [`AppState::trust_exchange_mode`], itself populated by
/// [`crate::trust_builder::build_trust_components`] when the server
/// boots with a ship profile:
///
/// | Mode                                     | Behaviour                                                        |
/// |------------------------------------------|------------------------------------------------------------------|
/// | [`TrustExchangeMode::LocalDevSynthetic`] | Mint the legacy synthetic local-dev token (back-compat default). |
/// | [`TrustExchangeMode::OpenBaoBridge`]     | Return 503 until the live bridge client lands (SHIP-08+).        |
/// | [`TrustExchangeMode::Disabled`]          | Return 410 Gone with [`ApiError::EndpointDisabled`].             |
///
/// The synthetic token is *not* cryptographic — it is a stable
/// identifier the policy runtime correlates with audit records. The
/// OpenBao bridge will replace the synthetic branch without changing
/// the route shape.
pub async fn exchange_token(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Json(req): Json<ExchangeTokenRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if req.subject_id.trim().is_empty() {
        return Err(ApiError::ValidationFailed(
            "subject_id is required".to_string(),
        ));
    }

    match state.trust_exchange_mode {
        TrustExchangeMode::LocalDevSynthetic => mint_local_dev_synthetic(profile, req),
        TrustExchangeMode::OpenBaoBridge => {
            let bridge = state.openbao_bridge.as_ref().ok_or_else(|| {
                tracing::error!(
                    "exchange_token: OpenBaoBridge mode selected but no bridge client wired"
                );
                ApiError::ServiceUnavailable
            })?;

            let tenant_id = req.tenant_id.as_deref().unwrap_or("tribal-health-demo");

            let roles = if req.scopes.is_empty() {
                vec!["clinician".to_string()]
            } else {
                req.scopes.clone()
            };

            match bridge.issue_claim(&req.subject_id, tenant_id, roles).await {
                Ok(claim) => {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs());
                    Ok(Json(ExchangeTokenResponse {
                        token: claim.claim_id.clone(),
                        token_type: "Bearer".to_string(),
                        subject_id: claim.subject_id,
                        tenant_id: claim.tenant_id,
                        scopes: claim.roles,
                        issued_at_secs: now,
                        expires_at_secs: now.saturating_add(900),
                        mode: "openbao-bridge".to_string(),
                    }))
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        subject = %req.subject_id,
                        "exchange_token: OpenBao bridge claim issuance failed"
                    );
                    Err(ApiError::ServiceUnavailable)
                }
            }
        }
        TrustExchangeMode::Disabled => Err(ApiError::EndpointDisabled(
            "token exchange disabled by active ship profile".to_string(),
        )),
    }
}

/// Build the legacy synthetic local-dev token. Pulled out of the
/// handler body so the SHIP-07 mode switch above stays readable.
fn mint_local_dev_synthetic(
    profile: ProfileInfo,
    req: ExchangeTokenRequest,
) -> Result<Json<ExchangeTokenResponse>, ApiError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    // Local-dev TTL: 15 minutes. Production reads this from policy.
    let expires_at = now.saturating_add(15 * 60);
    let tenant = req
        .tenant_id
        .unwrap_or_else(|| "local-dev-tenant".to_string());
    let scopes = if req.scopes.is_empty() {
        vec!["local_only".to_string()]
    } else {
        req.scopes
    };
    // Token shape: `local-dev.<profile>.<subject>.<issued_at>` — opaque
    // to consumers but easy to correlate in dashboards and audit rows.
    let token = format!(
        "local-dev.{profile_id}.{subject}.{issued_at}",
        profile_id = profile.profile_id,
        subject = req.subject_id,
        issued_at = now,
    );
    Ok(Json(ExchangeTokenResponse {
        token,
        token_type: "Bearer".to_string(),
        subject_id: req.subject_id,
        tenant_id: tenant,
        scopes,
        issued_at_secs: now,
        expires_at_secs: expires_at,
        mode: "local-dev".to_string(),
    }))
}

// ─── Helpers ───────────────────────────────────────────────────────

fn snapshot_status_label(status: SnapshotStatus) -> &'static str {
    match status {
        SnapshotStatus::Valid => "valid",
        SnapshotStatus::Revoked => "revoked",
        SnapshotStatus::Unknown => "unknown",
    }
}

// ─── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_status_labels() {
        assert_eq!(snapshot_status_label(SnapshotStatus::Valid), "valid");
        assert_eq!(snapshot_status_label(SnapshotStatus::Revoked), "revoked");
        assert_eq!(snapshot_status_label(SnapshotStatus::Unknown), "unknown");
    }

    #[test]
    fn test_trust_claim_from_snapshot() {
        let snap = RevocationSnapshot {
            claim_id: "claim-1".to_string(),
            status: SnapshotStatus::Valid,
            recorded_at_secs: 12345,
        };
        let claim: TrustClaim = snap.into();
        assert_eq!(claim.claim_id, "claim-1");
        assert_eq!(claim.status, "valid");
        assert_eq!(claim.recorded_at_secs, 12345);
    }
}
