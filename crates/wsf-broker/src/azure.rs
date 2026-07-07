//! Azure credential broker — exchange a verified trust token for a scoped Azure
//! AD access token (client-credentials / federated workload identity), capping
//! the credential's effective lifetime to the trust token's TTL.
//!
//! Same shape as the AWS/GCP brokers. Azure AD sets its own token lifetime
//! (~60–90 min, not request-shortenable), so **TTL enforced** here means the
//! broker returns an effective expiry of `min(azure_expiry, token_ttl)` — the
//! brokered credential never outlives the trust token that authorized it.
//!
//! No free Azure AD emulator exists, so the endpoint is configurable: the live
//! test points it at a local mock of the token endpoint; a real-Azure run is
//! owner-gated.

use chrono::{DateTime, Utc};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use wsf_bridge::OpenBaoAuth;

use crate::error::BrokerError;
use crate::verify_token;

const FORM: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

fn enc(s: &str) -> String {
    utf8_percent_encode(s, FORM).to_string()
}

/// Static configuration for the Azure broker.
#[derive(Debug, Clone)]
pub struct AzureBrokerConfig {
    /// Azure AD base URL — real Azure is `https://login.microsoftonline.com`;
    /// the live test points here at a local mock.
    pub endpoint: String,
    /// Azure AD tenant id (directory).
    pub tenant_id: String,
    /// OpenBao API path holding the broker's app credentials, e.g.
    /// `kv/data/broker/azure` (record shape: `{ client_id, client_secret }`).
    pub creds_kv_path: String,
}

impl AzureBrokerConfig {
    /// Build a config.
    #[must_use]
    pub fn new(
        endpoint: impl Into<String>,
        tenant_id: impl Into<String>,
        creds_kv_path: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            tenant_id: tenant_id.into(),
            creds_kv_path: creds_kv_path.into(),
        }
    }
}

/// A scoped Azure AD access token minted for a trust token.
///
/// `Debug` redacts the bearer token (plan B5 — parity with the AWS broker):
/// a stray `{:?}` in a log line must never leak a live credential.
#[derive(Clone)]
pub struct AzureCredentials {
    /// The Azure AD access token.
    pub access_token: String,
    /// Effective expiry = `min(Azure token expiry, trust-token TTL)`.
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for AzureCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureCredentials")
            .field("access_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// The Azure credential broker.
pub struct AzureBroker {
    openbao: OpenBaoAuth,
    http: reqwest::Client,
    config: AzureBrokerConfig,
}

/// Build the OAuth2 client-credentials form body. Pure, so it is unit-testable.
#[must_use]
pub fn token_form_body(client_id: &str, client_secret: &str, scope: &str) -> String {
    format!(
        "grant_type=client_credentials&client_id={}&client_secret={}&scope={}",
        enc(client_id),
        enc(client_secret),
        enc(scope),
    )
}

impl AzureBroker {
    /// Assemble an Azure broker.
    #[must_use]
    pub fn new(openbao: OpenBaoAuth, http: reqwest::Client, config: AzureBrokerConfig) -> Self {
        Self {
            openbao,
            http,
            config,
        }
    }

    /// Exchange a verified trust token for a scoped Azure AD access token. The
    /// returned `expires_at` never outlives the trust token.
    ///
    /// # Errors
    /// [`BrokerError::TokenRejected`] / [`BrokerError::TokenExpired`] (before any
    /// Azure call), [`BrokerError::OpenBao`] / [`BrokerError::RootCredential`] if
    /// the app creds cannot be fetched, or an STS/parse error from Azure.
    pub async fn acquire_token(
        &self,
        token: &fabric_contracts::TrustToken,
        verifier: &dyn fabric_crypto::Verifier,
        public_key: &[u8],
        scope: &str,
        now: DateTime<Utc>,
    ) -> Result<AzureCredentials, BrokerError> {
        // 1. Fail closed on trust.
        verify_token(token, verifier, public_key, now)?;

        // 2. Broker's app credentials from OpenBao (never exposed downstream).
        let vault_token = self.openbao.login().await?;
        let data = self
            .openbao
            .get_kv_data(&vault_token, &self.config.creds_kv_path)
            .await?;
        let client_id = data
            .get("client_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| BrokerError::RootCredential("missing client_id".into()))?;
        let client_secret = data
            .get("client_secret")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| BrokerError::RootCredential("missing client_secret".into()))?;

        // 3. Client-credentials grant.
        let url = format!(
            "{}/{}/oauth2/v2.0/token",
            self.config.endpoint.trim_end_matches('/'),
            self.config.tenant_id
        );
        let resp = self
            .http
            .post(&url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(token_form_body(client_id, client_secret, scope))
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(BrokerError::Sts(format!(
                "azure token {status}: {}",
                text.trim()
            )));
        }

        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| BrokerError::Parse(format!("azure response: {e}")))?;
        let access_token = v
            .get("access_token")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| BrokerError::Parse("missing access_token".into()))?
            .to_string();
        let expires_in = v
            .get("expires_in")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| BrokerError::Parse("missing expires_in".into()))?;

        // 4. Cap the effective expiry to the trust token's TTL.
        let azure_exp = now + chrono::Duration::seconds(expires_in);
        let token_exp = DateTime::parse_from_rfc3339(&token.expires_at)
            .map_err(|e| BrokerError::TokenRejected(format!("bad expires_at: {e}")))?
            .with_timezone(&Utc);

        Ok(AzureCredentials {
            access_token,
            expires_at: azure_exp.min(token_exp),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_body_encodes_fields() {
        let body = token_form_body(
            "client-guid",
            "s3cr3t/+=",
            "https://storage.azure.com/.default",
        );
        assert!(body.contains("grant_type=client_credentials"));
        assert!(body.contains("client_id=client-guid"));
        assert!(body.contains("scope=https%3A%2F%2Fstorage.azure.com%2F.default"));
        assert!(body.contains("client_secret=s3cr3t%2F%2B%3D"));
    }

    #[test]
    fn debug_output_redacts_the_access_token() {
        let creds = AzureCredentials {
            access_token: "eyJ.azure-bearer-material".to_string(),
            expires_at: Utc::now(),
        };
        let d = format!("{creds:?}");
        assert!(!d.contains("eyJ.azure-bearer-material"));
        assert!(d.contains("<redacted>"));
    }
}
