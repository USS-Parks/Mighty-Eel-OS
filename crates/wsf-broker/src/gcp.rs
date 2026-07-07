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

use chrono::{DateTime, Utc};
use wsf_bridge::OpenBaoAuth;

use crate::error::BrokerError;
use crate::{clamp_duration, verify_token};

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
#[derive(Debug, Clone)]
pub struct GcpCredentials {
    /// The OAuth2 access token.
    pub access_token: String,
    /// Expiry (from GCP).
    pub expire_time: DateTime<Utc>,
}

/// The GCP credential broker.
pub struct GcpBroker {
    openbao: OpenBaoAuth,
    http: reqwest::Client,
    config: GcpBrokerConfig,
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
        }
    }

    /// Exchange a verified trust token for a scoped GCP access token on
    /// `service_account`. `scopes` are the OAuth scopes the downstream token is
    /// granted; the lifetime tracks the trust token's remaining TTL.
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
        service_account: &str,
        scopes: &[String],
        now: DateTime<Utc>,
    ) -> Result<GcpCredentials, BrokerError> {
        // 1. Fail closed on trust. GCP snapshot-revocation wiring lands with the
        //    B4 GCP/Azure parity prompt; on-token revocation + expiry apply now.
        verify_token(token, verifier, public_key, None, now)?;

        // 2. Broker's Google bearer from OpenBao (never exposed downstream).
        let vault_token = self.openbao.login().await?;
        let data = self
            .openbao
            .get_kv_data(&vault_token, &self.config.bearer_kv_path)
            .await?;
        let bearer = data
            .get("bearer")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| BrokerError::RootCredential("missing gcp bearer".into()))?;

        // 3. Lifetime tracks the token TTL, clamped to the GCP window.
        let lifetime = clamp_duration(
            self.config.min_lifetime_secs,
            self.config.max_lifetime_secs,
            &token.expires_at,
            now,
        )?;

        // 4. generateAccessToken.
        let url = format!(
            "{}/v1/projects/-/serviceAccounts/{service_account}:generateAccessToken",
            self.config.endpoint.trim_end_matches('/')
        );
        let resp = self
            .http
            .post(&url)
            .bearer_auth(bearer)
            .json(&access_token_body(scopes, lifetime))
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
}
