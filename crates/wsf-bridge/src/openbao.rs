//! Focused OpenBao client for the WSF Trust Bridge.
//!
//! Adapted (not depended-on) from `mai-api/src/openbao_client.rs`: the bridge
//! needs only two OpenBao operations on the token-issuance path — AppRole login
//! (the "auth event") and a KV-v2 tenant-attribute read. It deliberately does
//! **not** use OpenBao Transit for signing: WSF trust tokens are signed with
//! pure-Rust ML-DSA-87 (`fabric-crypto`) so they verify off-host and in an
//! air-gap, where a networked custody backend is unreachable. Transit stays a
//! future custody seam (OSS OpenBao has no GA post-quantum Transit).
//!
//! Depending on the whole `mai-api` crate here would drag in its axum 0.7/0.8
//! `Handler` conflict (KNOWN-ISSUES #7, deferred by 0.2d); this ~150-line client
//! keeps `wsf-bridge` tonic/axum-free.

use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Configuration for the focused OpenBao client.
#[derive(Debug, Clone)]
pub struct OpenBaoConfig {
    /// OpenBao address, e.g. `http://localhost:8200`.
    pub address: String,
    /// AppRole role_id for the bridge workload.
    pub role_id: String,
    /// AppRole secret_id (pre-provisioned).
    pub secret_id: String,
    /// KV-v2 data-path prefix for tenant records, e.g. `kv/data/tenants`.
    pub tenant_data_path: String,
    /// Per-request timeout.
    pub timeout: Duration,
}

impl OpenBaoConfig {
    /// Construct with the conventional tenant path (`kv/data/tenants`) and a
    /// 10-second timeout.
    #[must_use]
    pub fn new(
        address: impl Into<String>,
        role_id: impl Into<String>,
        secret_id: impl Into<String>,
    ) -> Self {
        Self {
            address: address.into(),
            role_id: role_id.into(),
            secret_id: secret_id.into(),
            tenant_data_path: "kv/data/tenants".to_string(),
            timeout: Duration::from_secs(10),
        }
    }

    /// Builder: override the tenant KV-v2 data-path prefix.
    #[must_use]
    pub fn with_tenant_data_path(mut self, path: impl Into<String>) -> Self {
        self.tenant_data_path = path.into();
        self
    }

    /// Builder: override the per-request timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Tenant attributes read from OpenBao KV — the authorization envelope a token
/// is bounded by (compliance scopes, route ceiling, classification ceiling).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantAttributes {
    /// Stable tenant id.
    pub tenant_id: String,
    /// Human-readable name.
    #[serde(default)]
    pub display_name: String,
    /// Licensed compliance regimes (wire names, e.g. `hipaa`, `ocap`).
    #[serde(default)]
    pub compliance_scopes: Vec<String>,
    /// Default route ceiling (wire names, e.g. `local_only`).
    #[serde(default)]
    pub default_allowed_routes: Vec<String>,
    /// Maximum data classification (wire name, e.g. `restricted`).
    pub max_data_classification: String,
}

/// Failures from the focused OpenBao client.
#[derive(Debug, thiserror::Error)]
pub enum OpenBaoError {
    /// Network / transport failure.
    #[error("openbao transport: {0}")]
    Http(#[from] reqwest::Error),
    /// AppRole authentication was rejected.
    #[error("openbao auth failed: {0}")]
    Auth(String),
    /// A response could not be parsed, or a non-auth call returned an error.
    #[error("openbao protocol: {0}")]
    Protocol(String),
    /// The requested tenant does not exist.
    #[error("tenant not found: {0}")]
    TenantNotFound(String),
}

// ── OpenBao wire types ─────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct AppRoleLoginRequest<'a> {
    role_id: &'a str,
    secret_id: &'a str,
}

#[derive(Debug, Deserialize)]
struct AppRoleLoginResponse {
    auth: AppRoleAuth,
}

#[derive(Debug, Deserialize)]
struct AppRoleAuth {
    client_token: String,
    #[serde(default)]
    lease_duration: u64,
}

#[derive(Debug, Deserialize)]
struct KvV2Response {
    data: KvV2Data,
}

#[derive(Debug, Deserialize)]
struct KvV2Data {
    data: serde_json::Value,
}

/// The bridge's OpenBao client: AppRole login + KV-v2 tenant read. Cheap to
/// clone (an `Arc` inside `reqwest::Client`); stateless between calls (HA-ready).
#[derive(Debug, Clone)]
pub struct OpenBaoAuth {
    client: Client,
    config: OpenBaoConfig,
}

impl OpenBaoAuth {
    /// Build a client.
    ///
    /// # Errors
    /// Returns [`OpenBaoError::Http`] if the reqwest/TLS backend cannot be built.
    pub fn new(config: OpenBaoConfig) -> Result<Self, OpenBaoError> {
        let client = Client::builder().timeout(config.timeout).build()?;
        Ok(Self { client, config })
    }

    /// The configured OpenBao address.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.config.address
    }

    /// AppRole login — the OpenBao "auth event". Returns a short-lived client
    /// token scoped by the role's policies.
    ///
    /// # Errors
    /// [`OpenBaoError::Auth`] if the credential is rejected, or a transport error.
    pub async fn login(&self) -> Result<String, OpenBaoError> {
        let url = format!("{}/v1/auth/approle/login", self.config.address);
        let req = AppRoleLoginRequest {
            role_id: &self.config.role_id,
            secret_id: &self.config.secret_id,
        };
        let resp = self.client.post(&url).json(&req).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OpenBaoError::Auth(format!(
                "approle login {status}: {}",
                body.trim()
            )));
        }
        let body: AppRoleLoginResponse = resp
            .json()
            .await
            .map_err(|e| OpenBaoError::Protocol(format!("login response parse: {e}")))?;
        debug!(
            lease = body.auth.lease_duration,
            "authenticated to OpenBao via AppRole"
        );
        Ok(body.auth.client_token)
    }

    /// Read tenant attributes from KV v2 at `<tenant_data_path>/<tenant_id>`.
    ///
    /// Accepts both storage shapes: the `start-openbao.ps1` convention of
    /// `{ attributes: "<json string>" }`, and a raw attribute object.
    ///
    /// # Errors
    /// [`OpenBaoError::TenantNotFound`] on 404, or a transport/protocol error.
    pub async fn get_tenant(
        &self,
        token: &str,
        tenant_id: &str,
    ) -> Result<TenantAttributes, OpenBaoError> {
        let url = format!(
            "{}/v1/{}/{tenant_id}",
            self.config.address, self.config.tenant_data_path
        );
        let resp = self
            .client
            .get(&url)
            .header("X-Vault-Token", token)
            .send()
            .await?;
        let status = resp.status();
        if status == StatusCode::NOT_FOUND {
            return Err(OpenBaoError::TenantNotFound(tenant_id.to_string()));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OpenBaoError::Protocol(format!(
                "tenant read {status}: {}",
                body.trim()
            )));
        }
        let body: KvV2Response = resp
            .json()
            .await
            .map_err(|e| OpenBaoError::Protocol(format!("tenant kv parse: {e}")))?;
        let attrs_val = body
            .data
            .data
            .get("attributes")
            .cloned()
            .unwrap_or(body.data.data);
        let attrs: TenantAttributes = if let serde_json::Value::String(s) = &attrs_val {
            serde_json::from_str(s)
                .map_err(|e| OpenBaoError::Protocol(format!("tenant attrs from string: {e}")))?
        } else {
            serde_json::from_value(attrs_val)
                .map_err(|e| OpenBaoError::Protocol(format!("tenant attrs from object: {e}")))?
        };
        debug!(tenant = %attrs.tenant_id, "tenant attributes loaded");
        Ok(attrs)
    }

    /// Probe reachability + unseal status (`sys/seal-status`). `true` only when
    /// OpenBao is reachable and unsealed.
    pub async fn health(&self) -> bool {
        let url = format!("{}/v1/sys/seal-status", self.config.address);
        match self.client.get(&url).send().await {
            Ok(r) => r
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("sealed").and_then(serde_json::Value::as_bool))
                .is_some_and(|sealed| !sealed),
            Err(_) => false,
        }
    }
}
