//! GCP credential broker — exchange a verified trust token for a short-lived,
//! scoped GCP access token via the IAM Credentials API
//! (`serviceAccounts/{sa}:generateAccessToken`).
//!
//! Same shape as the AWS broker: verify the token (fail closed), read the
//! broker's Google bearer from OpenBao (custodied, never exposed — refreshed
//! out-of-band by a sidecar via the SA-JWT → OAuth2 exchange, which is
//! deployment plumbing, not in this hot path), then mint a downstream token with
//! the requested OAuth scopes and a lifetime clamped to the trust token's TTL.
//!
//! There is no free GCP IAM-Credentials emulator (unlike Moto for AWS), so the
//! endpoint is configurable: the live test points it at a local mock of the
//! `generateAccessToken` contract; a real-GCP run is owner-gated.

use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use fabric_revocation::MonotonicRevocationStore;
use wsf_bridge::OpenBaoAuth;

use crate::error::BrokerError;
use crate::{GcpGrantScope, strict_duration, verify_token};

/// Static configuration for the GCP broker.
#[derive(Debug, Clone)]
pub struct GcpBrokerConfig {
    /// IAM Credentials base URL — real GCP is
    /// `https://iamcredentials.googleapis.com`; the live test points here at a
    /// local mock.
    pub endpoint: String,
    /// OpenBao API path holding the broker's Google bearer, e.g.
    /// `kv/data/broker/gcp-bearer` (record shape: `{ "bearer": "ya29...." }`).
    pub bearer_kv_path: String,
    /// Access-token lifetime floor (seconds).
    pub min_lifetime_secs: i64,
    /// Access-token lifetime ceiling — GCP caps generateAccessToken at 3600s.
    pub max_lifetime_secs: i64,
}

impl GcpBrokerConfig {
    /// Config with the GCP default lifetime window (up to 3600s).
    #[must_use]
    pub fn new(endpoint: impl Into<String>, bearer_kv_path: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            bearer_kv_path: bearer_kv_path.into(),
            min_lifetime_secs: 300,
            max_lifetime_secs: 3600,
        }
    }
}

/// A short-lived GCP access token minted for a trust token.
///
/// `Debug` redacts the bearer token (plan B5 — parity with the AWS broker):
/// a stray `{:?}` in a log line must never leak a live credential.
#[derive(Clone)]
pub struct GcpCredentials {
    /// The OAuth2 access token.
    pub access_token: String,
    /// Expiry (from GCP).
    pub expire_time: DateTime<Utc>,
}

impl std::fmt::Debug for GcpCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcpCredentials")
            .field("access_token", &"<redacted>")
            .field("expire_time", &self.expire_time)
            .finish()
    }
}

/// The GCP credential broker.
pub struct GcpBroker {
    openbao: OpenBaoAuth,
    http: reqwest::Client,
    config: GcpBrokerConfig,
    revocation: Option<Arc<RwLock<MonotonicRevocationStore>>>,
}

/// Build the `generateAccessToken` request body: the OAuth `scope` list and a
/// `lifetime` duration string (`"<secs>s"`). Pure, so it is unit-testable.
#[must_use]
pub fn access_token_body(scopes: &[String], lifetime_secs: i64) -> serde_json::Value {
    serde_json::json!({
        "scope": scopes,
        "lifetime": format!("{lifetime_secs}s"),
    })
}

impl GcpBroker {
    /// Assemble a GCP broker.
    #[must_use]
    pub fn new(openbao: OpenBaoAuth, http: reqwest::Client, config: GcpBrokerConfig) -> Self {
        Self {
            openbao,
            http,
            config,
            revocation: None,
        }
    }

    /// Wire a revocation store (plan R consumer wiring) — fail closed, same
    /// semantics as the AWS broker.
    #[must_use]
    pub fn with_revocation_store(mut self, store: Arc<RwLock<MonotonicRevocationStore>>) -> Self {
        self.revocation = Some(store);
        self
    }

    /// Exchange a verified trust token for a scoped GCP access token.
    ///
    /// `grant` is the **server-resolved** scope (parity with the AWS broker's
    /// [`GrantScope`](crate::GrantScope)): the target service account and the
    /// downstream OAuth scopes are server-side truth resolved from a
    /// tenant-scoped `grant_id`, never named by the caller. The lifetime tracks
    /// the trust token's remaining TTL.
    ///
    /// # Errors
    /// [`BrokerError::TokenRejected`] / [`BrokerError::TokenExpired`] (before any
    /// GCP call), [`BrokerError::OpenBao`] / [`BrokerError::RootCredential`] if
    /// the bearer cannot be fetched, or an STS/parse error from GCP.
    pub async fn generate_access_token(
        &self,
        token: &fabric_contracts::TrustToken,
        verifier: &dyn fabric_crypto::Verifier,
        public_key: &[u8],
        grant: &GcpGrantScope,
        now: DateTime<Utc>,
    ) -> Result<GcpCredentials, BrokerError> {
        // 1. Fail closed on trust.
        verify_token(token, verifier, public_key, self.revocation.as_deref(), now)?;

        // 2. Refuse before custody when the token/revocation authority cannot
        // satisfy the provider lifetime floor.
        let revocation_expires_at = self.revocation.as_ref().and_then(|store| {
            store
                .read()
                .expect("revocation store lock")
                .current()
                .map(|snapshot| snapshot.expires_at.clone())
        });
        let lifetime = strict_duration(
            self.config.min_lifetime_secs,
            self.config.max_lifetime_secs,
            &token.expires_at,
            revocation_expires_at.as_deref(),
            now,
        )?;

        // 3. Broker's Google bearer from OpenBao (never exposed downstream).
        let vault_token = self.openbao.login().await?;
        let data = self
            .openbao
            .get_kv_data(&vault_token, &self.config.bearer_kv_path)
            .await?;
        let bearer = data
            .get("bearer")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| BrokerError::RootCredential("missing gcp bearer".into()))?;

        // 4. generateAccessToken.
        let url = format!(
            "{}/v1/projects/-/serviceAccounts/{}:generateAccessToken",
            self.config.endpoint.trim_end_matches('/'),
            grant.service_account
        );
        let resp = self
            .http
            .post(&url)
            .bearer_auth(bearer)
            .json(&access_token_body(&grant.scopes, lifetime))
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(BrokerError::Sts(format!(
                "gcp generateAccessToken {status}: {}",
                text.trim()
            )));
        }

        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| BrokerError::Parse(format!("gcp response: {e}")))?;
        let access_token = v
            .get("accessToken")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| BrokerError::Parse("missing accessToken".into()))?
            .to_string();
        let expire = v
            .get("expireTime")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| BrokerError::Parse("missing expireTime".into()))?;
        let expire_time = DateTime::parse_from_rfc3339(expire)
            .map_err(|e| BrokerError::Parse(format!("bad expireTime: {e}")))?
            .with_timezone(&Utc);

        Ok(GcpCredentials {
            access_token,
            expire_time,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_carries_scopes_and_lifetime_string() {
        let body = access_token_body(
            &[
                "https://www.googleapis.com/auth/cloud-platform".to_string(),
                "https://www.googleapis.com/auth/devstorage.read_only".to_string(),
            ],
            900,
        );
        assert_eq!(body["lifetime"], "900s");
        assert_eq!(
            body["scope"][0],
            "https://www.googleapis.com/auth/cloud-platform"
        );
        assert_eq!(body["scope"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn empty_scopes_still_valid_shape() {
        let body = access_token_body(&[], 300);
        assert_eq!(body["lifetime"], "300s");
        assert!(body["scope"].as_array().unwrap().is_empty());
    }

    #[test]
    fn debug_output_redacts_the_access_token() {
        let creds = GcpCredentials {
            access_token: "ya29.gcp-bearer-material".to_string(),
            expire_time: Utc::now(),
        };
        let d = format!("{creds:?}");
        assert!(!d.contains("ya29.gcp-bearer-material"));
        assert!(d.contains("<redacted>"));
    }
}
