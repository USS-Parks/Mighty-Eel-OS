//! OpenBao Trust Bridge HTTP client.
//!
//! Implements the Ring-1 ↔ Ring-3 bridge protocol defined in
//! `mai/docs/OPENBAO-INTEGRATION.md` §4 (Claim Issuance Flow):
//!
//! 1. Authenticate to OpenBao via AppRole
//! 2. Look up tenant attributes from KV
//! 3. Compose a Lamprey claim and sign it via Transit
//! 4. Return the signed claim to the caller
//!
//! The client is the bridge between the local MAI appliance and the
//! cloud OpenBao Trust Core. It moves ONLY identity metadata — never
//! prompts, completions, embeddings, or regulated payloads (§2.2).

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// OpenBao bridge client configuration.
#[derive(Debug, Clone)]
pub struct OpenBaoBridgeConfig {
    /// OpenBao server address (e.g. `http://localhost:8200`).
    pub address: String,
    /// AppRole role_id for the appliance.
    pub role_id: String,
    /// AppRole secret_id (pre-unwrapped).
    pub secret_id: Option<String>,
    /// Wrapped secret_id token. If present, the client unwraps it first.
    pub wrapped_secret_id: Option<String>,
    /// Request timeout.
    pub timeout: Duration,
}

impl OpenBaoBridgeConfig {
    /// Staging config. Reads secrets from environment variables — never
    /// hardcoded in source.
    ///
    /// Environment variables:
    /// - `MAI_OPENBAO_ADDR` — OpenBao address (default: `http://localhost:8200`)
    /// - `MAI_OPENBAO_ROLE_ID` — AppRole role_id (default: staging role)
    /// - `MAI_OPENBAO_SECRET_ID` — plain AppRole secret_id (preferred)
    /// - `MAI_OPENBAO_WRAPPED_SECRET_ID` — response-wrapped secret_id token
    ///
    /// At least one of `MAI_OPENBAO_SECRET_ID` or
    /// `MAI_OPENBAO_WRAPPED_SECRET_ID` must be set. Without a
    /// credential the client fails closed on first use.
    #[must_use]
    pub fn staging() -> Self {
        let address =
            std::env::var("MAI_OPENBAO_ADDR").unwrap_or_else(|_| "http://localhost:8200".into());
        let role_id = std::env::var("MAI_OPENBAO_ROLE_ID")
            .unwrap_or_else(|_| "8053c291-8f60-381f-e283-5e645e5907f4".into());
        let secret_id = std::env::var("MAI_OPENBAO_SECRET_ID").ok();
        let wrapped_secret_id = std::env::var("MAI_OPENBAO_WRAPPED_SECRET_ID").ok();

        if secret_id.is_none() && wrapped_secret_id.is_none() {
            warn!(
                "MAI_OPENBAO_SECRET_ID and MAI_OPENBAO_WRAPPED_SECRET_ID are both unset; \
                 bridge client will fail closed on first use"
            );
        }

        Self {
            address,
            role_id,
            secret_id,
            wrapped_secret_id,
            timeout: Duration::from_secs(10),
        }
    }
}

// ─── OpenBao API wire types ────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AppRoleLoginResponse {
    auth: AppRoleAuth,
}

#[derive(Debug, Deserialize)]
struct AppRoleAuth {
    client_token: String,
    lease_duration: u64,
}

#[derive(Debug, Deserialize)]
struct UnwrapResponse {
    data: UnwrapData,
}

#[derive(Debug, Deserialize)]
struct UnwrapData {
    secret_id: String,
}

#[derive(Debug, Serialize)]
struct AppRoleLoginRequest {
    role_id: String,
    secret_id: String,
}

#[derive(Debug, Deserialize)]
struct KvV2Response {
    data: KvV2Data,
}

#[derive(Debug, Deserialize)]
struct KvV2Data {
    data: serde_json::Value,
    metadata: Option<KvV2Metadata>,
}

#[derive(Debug, Deserialize)]
struct KvV2Metadata {
    created_time: Option<String>,
}

#[derive(Debug, Serialize)]
struct TransitSignRequest {
    input: String,
}

#[derive(Debug, Deserialize)]
struct TransitSignResponse {
    data: TransitSignData,
}

#[derive(Debug, Deserialize)]
struct TransitSignData {
    signature: String,
    key_version: u32,
}

// ─── Bridge output types ───────────────────────────────────────────

/// A Lamprey claim, signed by the OpenBao Transit engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedLampreyClaim {
    pub claim_id: String,
    pub issued_at: String,
    pub expires_at: String,
    pub issuer: String,
    pub trust_bundle_version: String,
    pub tenant_id: String,
    pub subject_id: String,
    pub subject_hash: String,
    pub roles: Vec<String>,
    pub compliance_scopes: Vec<String>,
    pub allowed_routes: Vec<String>,
    pub allowed_models: Vec<String>,
    pub max_data_classification: String,
    pub country: String,
    pub person_type: String,
    pub offline_mode: bool,
    pub revocation_status: String,
    pub signature: ClaimSignature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimSignature {
    pub alg: String,
    pub key_id: String,
    pub value: String,
}

/// Tenant attributes read from OpenBao KV.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantAttributes {
    pub tenant_id: String,
    pub display_name: String,
    pub compliance_scopes: Vec<String>,
    pub default_allowed_routes: Vec<String>,
    pub max_data_classification: String,
}

/// Result of an OpenBao bridge health probe.
#[derive(Debug, Clone, Serialize)]
pub struct OpenbaoHealth {
    pub reachable: bool,
    pub sealed: bool,
    pub kv_mounted: bool,
    pub transit_mounted: bool,
    pub pki_mounted: bool,
    pub approle_enabled: bool,
    pub demo_tenant_exists: bool,
    pub claim_signer_key_exists: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
}

// ─── Bridge client ─────────────────────────────────────────────────

/// OpenBao HTTP bridge client.
///
/// Wraps a `reqwest::Client` with the appliance's AppRole credentials.
/// Thread-safe, cheap to clone (single `Arc` inside `reqwest::Client`).
#[derive(Debug, Clone)]
pub struct OpenBaoBridgeClient {
    client: Client,
    config: Arc<OpenBaoBridgeConfig>,
}

impl OpenBaoBridgeClient {
    pub fn new(config: OpenBaoBridgeConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("reqwest client construction is infallible with valid config");
        Self {
            client,
            config: Arc::new(config),
        }
    }

    /// Resolve the AppRole secret_id: unwrap a wrapped token, or use
    /// the plain secret_id if already provisioned.
    async fn resolve_secret_id(&self) -> Result<String, OpenBaoBridgeError> {
        if let Some(ref plain) = self.config.secret_id {
            return Ok(plain.clone());
        }
        if let Some(ref wrapped) = self.config.wrapped_secret_id {
            return self.unwrap_secret_id(wrapped).await;
        }
        Err(OpenBaoBridgeError::Config(
            "neither secret_id nor wrapped_secret_id configured".into(),
        ))
    }

    /// Unwrap a response-wrapped secret_id token.
    async fn unwrap_secret_id(&self, wrapping_token: &str) -> Result<String, OpenBaoBridgeError> {
        let url = format!("{}/v1/sys/wrapping/unwrap", self.config.address);
        let resp = self
            .client
            .post(&url)
            .header("X-Vault-Token", wrapping_token)
            .send()
            .await?;
        let body: UnwrapResponse = resp
            .json()
            .await
            .map_err(|e| OpenBaoBridgeError::Protocol(format!("unwrap response parse: {e}")))?;
        debug!("Unwrapped secret_id");
        Ok(body.data.secret_id)
    }

    /// Authenticate to OpenBao via AppRole and obtain a short-lived token.
    async fn login(&self) -> Result<String, OpenBaoBridgeError> {
        let secret_id = self.resolve_secret_id().await?;
        let url = format!("{}/v1/auth/approle/login", self.config.address);
        let req = AppRoleLoginRequest {
            role_id: self.config.role_id.clone(),
            secret_id,
        };
        let resp = self.client.post(&url).json(&req).send().await?;
        let body: AppRoleLoginResponse = resp
            .json()
            .await
            .map_err(|e| OpenBaoBridgeError::Protocol(format!("login response parse: {e}")))?;
        info!(
            lease_duration = body.auth.lease_duration,
            "Authenticated to OpenBao via AppRole"
        );
        Ok(body.auth.client_token)
    }

    /// Read tenant attributes from OpenBao KV.
    pub async fn get_tenant(
        &self,
        token: &str,
        tenant_id: &str,
    ) -> Result<TenantAttributes, OpenBaoBridgeError> {
        let url = format!("{}/v1/kv/data/tenants/{tenant_id}", self.config.address);
        let resp = self
            .client
            .get(&url)
            .header("X-Vault-Token", token)
            .send()
            .await?;
        let body: KvV2Response = resp
            .json()
            .await
            .map_err(|e| OpenBaoBridgeError::Protocol(format!("kv read parse: {e}")))?;
        // KV v2 returns { data: { data: { attributes: "{json string}" }, metadata: {...} } }
        // The `attributes` value may be a JSON string (bao kv put stores values
        // serialized) or an already-parsed object. Handle both.
        let attrs_val = body
            .data
            .data
            .get("attributes")
            .cloned()
            .unwrap_or(body.data.data.clone());
        let attrs: TenantAttributes = if let serde_json::Value::String(s) = &attrs_val {
            serde_json::from_str(s).map_err(|e| {
                OpenBaoBridgeError::Protocol(format!("tenant attr parse from string: {e}"))
            })?
        } else {
            serde_json::from_value(attrs_val).map_err(|e| {
                OpenBaoBridgeError::Protocol(format!("tenant attr parse from object: {e}"))
            })?
        };
        debug!(tenant = %attrs.tenant_id, scopes = ?attrs.compliance_scopes, "Tenant loaded");
        Ok(attrs)
    }

    /// Sign a claim payload with the Lamprey claim-signer transit key.
    pub async fn sign_claim(
        &self,
        token: &str,
        claim_json: &str,
    ) -> Result<String, OpenBaoBridgeError> {
        let url = format!(
            "{}/v1/transit/sign/lamprey-claim-signer",
            self.config.address
        );
        let b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            claim_json.as_bytes(),
        );
        let req = TransitSignRequest { input: b64 };
        let resp = self
            .client
            .post(&url)
            .header("X-Vault-Token", token)
            .json(&req)
            .send()
            .await?;
        let body: TransitSignResponse = resp
            .json()
            .await
            .map_err(|e| OpenBaoBridgeError::Protocol(format!("transit sign parse: {e}")))?;
        debug!(
            key_version = body.data.key_version,
            "Claim signed via OpenBao transit"
        );
        Ok(body.data.signature)
    }

    /// Full claim issuance flow: auth → tenant lookup → claim build → sign.
    ///
    /// Returns a fully signed Lamprey claim suitable for injection into
    /// the local trust cache and for presentation to the policy runtime.
    pub async fn issue_claim(
        &self,
        subject_id: &str,
        tenant_id: &str,
        roles: Vec<String>,
    ) -> Result<SignedLampreyClaim, OpenBaoBridgeError> {
        let token = self.login().await?;
        let tenant = self.get_tenant(&token, tenant_id).await?;

        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expire_secs = now_secs + 900;

        let claim_id = format!("clm_{}_{:04x}", now_secs, rand::random::<u16>());

        let issued = chrono::Utc::now();
        let expires = issued + chrono::TimeDelta::seconds(900);

        let claim_json = serde_json::json!({
            "claim_id": claim_id,
            "issued_at": issued.to_rfc3339(),
            "expires_at": expires.to_rfc3339(),
            "issuer": "lamprey-trust-bridge",
            "trust_bundle_version": "2026.05.26.001",
            "tenant_id": tenant_id,
            "subject_id": subject_id,
            "subject_hash": format!("hmac:{}", sha256_hex(subject_id)),
            "roles": roles.clone(),
            "compliance_scopes": tenant.compliance_scopes.clone(),
            "allowed_routes": tenant.default_allowed_routes.clone(),
            "allowed_models": [],
            "max_data_classification": tenant.max_data_classification.clone(),
            "country": "US",
            "person_type": "us_person",
            "offline_mode": false,
            "revocation_status": "valid",
        });

        let claim_str = serde_json::to_string(&claim_json)
            .map_err(|e| OpenBaoBridgeError::Protocol(format!("claim serialize: {e}")))?;
        let sig_value = self.sign_claim(&token, &claim_str).await?;

        let envelope = serde_json::json!({
            "claim_id": claim_id,
            "issued_at": issued.to_rfc3339(),
            "expires_at": expires.to_rfc3339(),
            "issuer": "lamprey-trust-bridge",
            "trust_bundle_version": "2026.05.26.001",
            "tenant_id": tenant_id,
            "subject_id": subject_id,
            "subject_hash": format!("hmac:{}", sha256_hex(subject_id)),
            "roles": roles,
            "compliance_scopes": tenant.compliance_scopes,
            "allowed_routes": tenant.default_allowed_routes,
            "allowed_models": [],
            "max_data_classification": tenant.max_data_classification,
            "country": "US",
            "person_type": "us_person",
            "offline_mode": false,
            "revocation_status": "valid",
            "signature": {
                "alg": "ed25519",
                "key_id": "lamprey-claim-signer",
                "value": sig_value,
            },
        });

        let signed: SignedLampreyClaim = serde_json::from_value(envelope)
            .map_err(|e| OpenBaoBridgeError::Protocol(format!("claim envelope: {e}")))?;

        info!(
            claim_id = %signed.claim_id,
            tenant = %signed.tenant_id,
            subject = %signed.subject_id,
            "Claim issued via OpenBao bridge"
        );

        Ok(signed)
    }

    /// Rotate the bridge client's credential in-place by atomically
    /// swapping the inner `Arc<RwLock<Option<...>>>`. The caller must
    /// have already obtained the new secret_id from OpenBao (e.g. via
    /// `ir-respond.ps1 rotate-appliance` or an admin API call).
    pub async fn rotate_credential(
        &self,
        bridge_lock: &Arc<RwLock<Option<OpenBaoBridgeClient>>>,
        new_secret_id: &str,
    ) -> Result<(), OpenBaoBridgeError> {
        let new_config = OpenBaoBridgeConfig {
            secret_id: Some(new_secret_id.to_string()),
            ..self.config.as_ref().clone()
        };
        let new_bridge = OpenBaoBridgeClient::new(new_config);
        *bridge_lock.write().await = Some(new_bridge);
        info!("OpenBao bridge credential rotated");
        Ok(())
    }

    /// Probe OpenBao health and return a structured status report.
    /// Used by `GET /v1/trust/openbao_health` for operator dashboards.
    pub async fn health_check(&self) -> OpenbaoHealth {
        let start = std::time::Instant::now();
        let client = self.client.clone();
        let addr = self.config.address.clone();

        let (reachable, sealed) = match client
            .get(format!("{addr}/v1/sys/seal-status"))
            .send()
            .await
        {
            Ok(resp) => {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    let sealed = body.get("sealed").and_then(|v| v.as_bool()).unwrap_or(true);
                    (true, sealed)
                } else {
                    (true, true)
                }
            }
            Err(e) => {
                let latency = start.elapsed().as_millis() as u64;
                return OpenbaoHealth {
                    reachable: false,
                    sealed: true,
                    kv_mounted: false,
                    transit_mounted: false,
                    pki_mounted: false,
                    approle_enabled: false,
                    demo_tenant_exists: false,
                    claim_signer_key_exists: false,
                    latency_ms: latency,
                    error: Some(format!("{e}")),
                };
            }
        };

        let probe = |path: &str| {
            let c = client.clone();
            let url = format!("{addr}{path}");
            async move {
                c.get(&url)
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success())
            }
        };

        let kv_mounted = probe("/v1/sys/mounts/kv").await;
        let transit_mounted = probe("/v1/sys/mounts/transit").await;
        let pki_mounted = probe("/v1/sys/mounts/pki").await;
        let approle_enabled = probe("/v1/sys/auth/approle").await;
        let demo_tenant_exists = probe("/v1/kv/data/tenants/tribal-health-demo").await;
        let claim_signer_key_exists = probe("/v1/transit/keys/lamprey-claim-signer").await;
        let latency = start.elapsed().as_millis() as u64;

        OpenbaoHealth {
            reachable,
            sealed,
            kv_mounted,
            transit_mounted,
            pki_mounted,
            approle_enabled,
            demo_tenant_exists,
            claim_signer_key_exists,
            latency_ms: latency,
            error: None,
        }
    }
}

fn sha256_hex(input: &str) -> String {
    use sha3::Digest;
    hex::encode(sha3::Sha3_256::digest(input.as_bytes()))
}

// ─── Errors ────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum OpenBaoBridgeError {
    #[error("openbao bridge config: {0}")]
    Config(String),
    #[error("openbao bridge HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("openbao bridge protocol: {0}")]
    Protocol(String),
}
