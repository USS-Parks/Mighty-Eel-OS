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

use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tracing::debug;
use zeroize::Zeroize;

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
///
/// `Debug` is hand-written to redact `subject_hmac_key`: the derived form would
/// print the raw per-tenant key, so a stray `{:?}` in a log line would leak
/// keying material.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Per-tenant subject-pseudonymization HMAC key (hex). When present, the
    /// bridge uses it instead of its config-wide key (W9 tenant provisioning).
    /// Optional for backwards compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_hmac_key: Option<String>,
}

impl std::fmt::Debug for TenantAttributes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantAttributes")
            .field("tenant_id", &self.tenant_id)
            .field("display_name", &self.display_name)
            .field("compliance_scopes", &self.compliance_scopes)
            .field("default_allowed_routes", &self.default_allowed_routes)
            .field("max_data_classification", &self.max_data_classification)
            .field(
                "subject_hmac_key",
                &self.subject_hmac_key.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
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
    /// A requested KV path does not exist.
    #[error("kv path not found: {0}")]
    NotFound(String),
    /// A caller supplied an unsafe or unsupported token-lease request.
    #[error("invalid token lease request: {0}")]
    InvalidLease(String),
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

#[derive(Debug, Serialize)]
struct TokenCreateRequest<'a> {
    display_name: &'a str,
    ttl: String,
    explicit_max_ttl: String,
    renewable: bool,
    no_default_policy: bool,
    policies: &'a [String],
    #[serde(rename = "meta")]
    metadata: &'a BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct TokenCreateResponse {
    auth: TokenAuth,
}

#[derive(Debug, Deserialize)]
struct TokenAuth {
    client_token: String,
    accessor: String,
    lease_duration: u64,
}

#[derive(Debug, Serialize)]
struct AccessorRequest<'a> {
    accessor: &'a str,
}

/// An authority-enforced OpenBao token lease. The token is deliberately
/// redacted from `Debug`; only its accessor is suitable for logs and receipts.
#[derive(Clone)]
pub struct OpenBaoTokenLease {
    client_token: String,
    accessor: String,
    lease_duration: Duration,
}

impl OpenBaoTokenLease {
    /// Consume the lease for handoff to the executor boundary. Any token bytes
    /// left in this value are zeroized by `Drop`.
    #[must_use]
    pub fn into_parts(mut self) -> (String, String, Duration) {
        let client_token = std::mem::take(&mut self.client_token);
        let accessor = std::mem::take(&mut self.accessor);
        (client_token, accessor, self.lease_duration)
    }
}

impl Drop for OpenBaoTokenLease {
    fn drop(&mut self) {
        self.client_token.zeroize();
    }
}

impl std::fmt::Debug for OpenBaoTokenLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenBaoTokenLease")
            .field("client_token", &"<redacted>")
            .field("accessor", &self.accessor)
            .field("lease_duration", &self.lease_duration)
            .finish()
    }
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

    /// Create one non-renewable token from a pre-provisioned token role. Both
    /// `ttl` and `explicit_max_ttl` are set to the requested bound, so OpenBao
    /// itself—not the caller—enforces expiration.
    pub async fn create_token_for_role(
        &self,
        parent_token: &str,
        role: &str,
        display_name: &str,
        ttl: Duration,
        policies: &[String],
        metadata: &BTreeMap<String, String>,
    ) -> Result<OpenBaoTokenLease, OpenBaoError> {
        validate_path_segment(role, "token role")?;
        if display_name.trim().is_empty() || display_name.len() > 128 {
            return Err(OpenBaoError::InvalidLease(
                "display name must contain 1..=128 characters".to_string(),
            ));
        }
        if ttl.is_zero() || ttl > Duration::from_secs(60) {
            return Err(OpenBaoError::InvalidLease(
                "TTL must be within 1..=60 seconds".to_string(),
            ));
        }
        if policies.is_empty() {
            return Err(OpenBaoError::InvalidLease(
                "at least one child token policy is required".to_string(),
            ));
        }
        for policy in policies {
            validate_path_segment(policy, "token policy")?;
        }
        let ttl_wire = format!("{}s", ttl.as_secs());
        let request = TokenCreateRequest {
            display_name,
            ttl: ttl_wire.clone(),
            explicit_max_ttl: ttl_wire,
            renewable: false,
            no_default_policy: true,
            policies,
            metadata,
        };
        let url = format!("{}/v1/auth/token/create/{role}", self.config.address);
        let response = self
            .client
            .post(url)
            .header("X-Vault-Token", parent_token)
            .json(&request)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(OpenBaoError::Protocol(format!(
                "token create {status}: {}",
                body.trim()
            )));
        }
        let response: TokenCreateResponse = response
            .json()
            .await
            .map_err(|error| OpenBaoError::Protocol(format!("token create parse: {error}")))?;
        if response.auth.client_token.is_empty()
            || response.auth.accessor.is_empty()
            || response.auth.lease_duration == 0
            || response.auth.lease_duration > ttl.as_secs()
        {
            return Err(OpenBaoError::Protocol(
                "token create returned an invalid or overlong lease".to_string(),
            ));
        }
        Ok(OpenBaoTokenLease {
            client_token: response.auth.client_token,
            accessor: response.auth.accessor,
            lease_duration: Duration::from_secs(response.auth.lease_duration),
        })
    }

    /// Revoke a token by its non-authorizing accessor. A missing accessor is
    /// treated as already revoked, making durable retry idempotent.
    pub async fn revoke_token_accessor(
        &self,
        parent_token: &str,
        accessor: &str,
    ) -> Result<(), OpenBaoError> {
        validate_path_segment(accessor, "token accessor")?;
        let url = format!("{}/v1/auth/token/revoke-accessor", self.config.address);
        let response = self
            .client
            .post(url)
            .header("X-Vault-Token", parent_token)
            .json(&AccessorRequest { accessor })
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() && status != StatusCode::BAD_REQUEST {
            let body = response.text().await.unwrap_or_default();
            return Err(OpenBaoError::Protocol(format!(
                "token revoke {status}: {}",
                body.trim()
            )));
        }
        Ok(())
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

    /// Read a KV-v2 record's inner `data.data` object at a full API `path`
    /// (e.g. `kv/data/broker/aws-root`). Used by downstream trust-plane services
    /// — the STS broker fetches its custodied cloud root credentials this way.
    ///
    /// # Errors
    /// [`OpenBaoError::NotFound`] on 404, or a transport / protocol error.
    pub async fn get_kv_data(
        &self,
        token: &str,
        path: &str,
    ) -> Result<serde_json::Value, OpenBaoError> {
        let url = format!("{}/v1/{path}", self.config.address);
        let resp = self
            .client
            .get(&url)
            .header("X-Vault-Token", token)
            .send()
            .await?;
        let status = resp.status();
        if status == StatusCode::NOT_FOUND {
            return Err(OpenBaoError::NotFound(path.to_string()));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OpenBaoError::Protocol(format!(
                "kv read {status}: {}",
                body.trim()
            )));
        }
        let body: KvV2Response = resp
            .json()
            .await
            .map_err(|e| OpenBaoError::Protocol(format!("kv parse: {e}")))?;
        Ok(body.data.data)
    }

    /// Write a KV-v2 record at a full API `path` (e.g. `kv/data/tenants/<id>`),
    /// wrapping `data` in the KV-v2 `{ "data": … }` envelope. Admin use (tenant
    /// provisioning).
    ///
    /// # Errors
    /// [`OpenBaoError::Protocol`] on a non-2xx, or a transport error.
    pub async fn put_kv_data(
        &self,
        token: &str,
        path: &str,
        data: serde_json::Value,
    ) -> Result<(), OpenBaoError> {
        let url = format!("{}/v1/{path}", self.config.address);
        let resp = self
            .client
            .post(&url)
            .header("X-Vault-Token", token)
            .json(&serde_json::json!({ "data": data }))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OpenBaoError::Protocol(format!(
                "kv write {status}: {}",
                body.trim()
            )));
        }
        Ok(())
    }

    /// Delete a KV path (e.g. `kv/metadata/tenants/<id>` for a full delete). A
    /// 404 is treated as success (already gone). Admin use (tenant deprovision).
    ///
    /// # Errors
    /// [`OpenBaoError::Protocol`] on a non-2xx (other than 404), or a transport error.
    pub async fn delete_kv(&self, token: &str, path: &str) -> Result<(), OpenBaoError> {
        let url = format!("{}/v1/{path}", self.config.address);
        let resp = self
            .client
            .delete(&url)
            .header("X-Vault-Token", token)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() && status != StatusCode::NOT_FOUND {
            let body = resp.text().await.unwrap_or_default();
            return Err(OpenBaoError::Protocol(format!(
                "kv delete {status}: {}",
                body.trim()
            )));
        }
        Ok(())
    }

    /// Transit-encrypt `plaintext` under `key` (`transit/encrypt/<key>`),
    /// returning the opaque `vault:v1:...` wrapped ciphertext. This is how the
    /// seal service wraps a per-envelope data key (the F4 seal seam) — Transit
    /// does symmetric AEAD even though OSS Transit has no ML-DSA *signing*.
    ///
    /// # Errors
    /// [`OpenBaoError::Protocol`] on a non-2xx or a malformed response, or a
    /// transport error.
    pub async fn transit_encrypt(
        &self,
        token: &str,
        key: &str,
        plaintext: &[u8],
    ) -> Result<String, OpenBaoError> {
        let url = format!("{}/v1/transit/encrypt/{key}", self.config.address);
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, plaintext);
        let resp = self
            .client
            .post(&url)
            .header("X-Vault-Token", token)
            .json(&serde_json::json!({ "plaintext": b64 }))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OpenBaoError::Protocol(format!(
                "transit encrypt {status}: {}",
                body.trim()
            )));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| OpenBaoError::Protocol(format!("transit encrypt parse: {e}")))?;
        v.get("data")
            .and_then(|d| d.get("ciphertext"))
            .and_then(serde_json::Value::as_str)
            .map(String::from)
            .ok_or_else(|| OpenBaoError::Protocol("transit encrypt: missing ciphertext".into()))
    }

    /// Transit-decrypt a `vault:v1:...` ciphertext under `key`, returning the
    /// recovered plaintext bytes.
    ///
    /// # Errors
    /// [`OpenBaoError::Protocol`] on a non-2xx or a malformed response, or a
    /// transport error.
    pub async fn transit_decrypt(
        &self,
        token: &str,
        key: &str,
        ciphertext: &str,
    ) -> Result<Vec<u8>, OpenBaoError> {
        let url = format!("{}/v1/transit/decrypt/{key}", self.config.address);
        let resp = self
            .client
            .post(&url)
            .header("X-Vault-Token", token)
            .json(&serde_json::json!({ "ciphertext": ciphertext }))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OpenBaoError::Protocol(format!(
                "transit decrypt {status}: {}",
                body.trim()
            )));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| OpenBaoError::Protocol(format!("transit decrypt parse: {e}")))?;
        let b64 = v
            .get("data")
            .and_then(|d| d.get("plaintext"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| OpenBaoError::Protocol("transit decrypt: missing plaintext".into()))?;
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
            .map_err(|e| OpenBaoError::Protocol(format!("transit decrypt b64: {e}")))
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

fn validate_path_segment(value: &str, label: &str) -> Result<(), OpenBaoError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(OpenBaoError::InvalidLease(format!(
            "{label} must match [A-Za-z0-9_-]{{1,128}}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod token_lease_tests {
    use super::*;

    #[test]
    fn token_lease_debug_redacts_secret() {
        let lease = OpenBaoTokenLease {
            client_token: "never-print-me".to_string(),
            accessor: "accessor-1".to_string(),
            lease_duration: Duration::from_secs(30),
        };
        let rendered = format!("{lease:?}");
        assert!(!rendered.contains("never-print-me"));
        assert!(rendered.contains("<redacted>"));
        assert!(rendered.contains("accessor-1"));
    }

    #[test]
    fn path_segment_validation_rejects_injection() {
        assert!(validate_path_segment("role-a_1", "role").is_ok());
        assert!(validate_path_segment("../root", "role").is_err());
        assert!(validate_path_segment("role?x=1", "role").is_err());
        assert!(validate_path_segment("", "role").is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::TenantAttributes;

    #[test]
    fn debug_redacts_the_subject_hmac_key() {
        // A stray `{:?}` on TenantAttributes must never render the per-tenant
        // HMAC key bytes.
        let attrs = TenantAttributes {
            tenant_id: "t-a".to_string(),
            display_name: "Tenant A".to_string(),
            compliance_scopes: vec!["hipaa".to_string()],
            default_allowed_routes: vec!["local_only".to_string()],
            max_data_classification: "restricted".to_string(),
            subject_hmac_key: Some("deadbeefkeymaterial".to_string()),
        };
        let dbg = format!("{attrs:?}");
        assert!(!dbg.contains("deadbeefkeymaterial"), "key must not appear");
        assert!(dbg.contains("<redacted>"));
        assert!(dbg.contains("t-a"), "non-secret fields still render");

        // A None key renders as None, never a leak.
        let none = TenantAttributes {
            subject_hmac_key: None,
            ..attrs
        };
        let dbg_none = format!("{none:?}");
        assert!(dbg_none.contains("None"));
        assert!(!dbg_none.contains("<redacted>"));
    }
}
