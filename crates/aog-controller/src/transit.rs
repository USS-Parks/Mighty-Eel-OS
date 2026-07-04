//! R4 — minimal OpenBao Transit key administration for the TrustRing
//! controller: ensure a per-ring key exists, read its version, and disable it
//! (the "ring dark" switch). Data-path transit use (encrypt/decrypt) stays on
//! `wsf_bridge::OpenBaoAuth`; this adds only the key-lifecycle calls the M1
//! client never needed. Auth rides the same AppRole login.

use reqwest::{Client, StatusCode};
use serde_json::json;
use wsf_bridge::OpenBaoAuth;

use crate::runtime::ReconcileError;

/// The Transit key name for a trust ring.
#[must_use]
pub fn ring_key_name(ring: u8) -> String {
    format!("loom-ring-{ring}")
}

/// Transit key administration over an AppRole-authenticated OpenBao.
pub struct TransitAdmin {
    openbao: OpenBaoAuth,
    http: Client,
}

impl TransitAdmin {
    /// # Errors
    /// [`ReconcileError`] if the HTTP client cannot be built.
    pub fn new(openbao: OpenBaoAuth) -> Result<Self, ReconcileError> {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| ReconcileError(e.to_string()))?;
        Ok(Self { openbao, http })
    }

    async fn login(&self) -> Result<String, ReconcileError> {
        self.openbao
            .login()
            .await
            .map_err(|e| ReconcileError(e.to_string()))
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<reqwest::Response, ReconcileError> {
        let token = self.login().await?;
        let url = format!("{}/v1/{path}", self.openbao.address());
        let mut rb = self
            .http
            .request(method, url)
            .header("X-Vault-Token", token);
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        rb.send().await.map_err(|e| ReconcileError(e.to_string()))
    }

    /// Read a key's latest version; `Ok(None)` when the key does not exist.
    ///
    /// # Errors
    /// [`ReconcileError`] on transport failure or an unexpected status.
    pub async fn key_version(&self, name: &str) -> Result<Option<u32>, ReconcileError> {
        let resp = self
            .call(reqwest::Method::GET, &format!("transit/keys/{name}"), None)
            .await?;
        match resp.status() {
            StatusCode::NOT_FOUND => Ok(None),
            s if s.is_success() => {
                let v: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| ReconcileError(e.to_string()))?;
                let version = v["data"]["latest_version"].as_u64().unwrap_or(0);
                Ok(Some(u32::try_from(version).unwrap_or(u32::MAX)))
            }
            s => Err(ReconcileError(format!("transit key read: {s}"))),
        }
    }

    /// Ensure the key exists; returns its latest version.
    ///
    /// # Errors
    /// [`ReconcileError`] if the key can be neither read nor created.
    pub async fn ensure_key(&self, name: &str) -> Result<u32, ReconcileError> {
        if let Some(version) = self.key_version(name).await? {
            return Ok(version);
        }
        let resp = self
            .call(
                reqwest::Method::POST,
                &format!("transit/keys/{name}"),
                Some(json!({})),
            )
            .await?;
        if !resp.status().is_success() {
            return Err(ReconcileError(format!(
                "transit key create: {}",
                resp.status()
            )));
        }
        self.key_version(name)
            .await?
            .ok_or_else(|| ReconcileError("transit key vanished after create".to_owned()))
    }

    /// Disable a ring key — the "ring dark" switch. The key is deleted from
    /// Transit, so every data key it wrapped (and every envelope under those)
    /// becomes unrecoverable until a deliberate operator restore. Idempotent:
    /// an already-absent key is already dark.
    ///
    /// # Errors
    /// [`ReconcileError`] on transport failure or a refusal other than 404.
    pub async fn disable_key(&self, name: &str) -> Result<(), ReconcileError> {
        // Already absent = already dark (and Transit answers 400, not 404, to
        // a config write on a deleted key — so check, don't probe).
        if self.key_version(name).await?.is_none() {
            return Ok(());
        }
        let config = self
            .call(
                reqwest::Method::POST,
                &format!("transit/keys/{name}/config"),
                Some(json!({ "deletion_allowed": true })),
            )
            .await?;
        if !(config.status().is_success() || config.status() == StatusCode::NOT_FOUND) {
            return Err(ReconcileError(format!(
                "transit key config: {}",
                config.status()
            )));
        }
        let del = self
            .call(
                reqwest::Method::DELETE,
                &format!("transit/keys/{name}"),
                None,
            )
            .await?;
        if del.status().is_success() || del.status() == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(ReconcileError(format!(
                "transit key delete: {}",
                del.status()
            )))
        }
    }
}
