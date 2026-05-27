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
//! TLM-3 adds bundle operations:
//!
//! 5. Fetch signed policy bundles from KV
//! 6. Sign bundle payloads via Transit (bundle-signer key)
//!
//! The client is the bridge between the local MAI appliance and the
//! cloud OpenBao Trust Core. It moves ONLY identity metadata — never
//! prompts, completions, embeddings, or regulated payloads (§2.2).

use crate::ship_profile::{KvPathConfig, OpenbaoConfig, PkiRoleConfig, TransitKeysConfig};
use mai_compliance::bundle::SignedPolicyBundle;
use mai_compliance::trust_cache::{LocalTrustCache, RevocationSnapshot, SnapshotStatus};
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
    /// Transit key names from ship profile.
    pub transit_keys: TransitKeysConfig,
    /// KV paths from ship profile.
    pub kv_paths: KvPathConfig,
    /// PKI role from ship profile.
    pub pki_role: PkiRoleConfig,
    /// Whether mTLS is enabled (requires HTTPS address).
    pub use_mtls: bool,
}

impl OpenBaoBridgeConfig {
    /// Build from a ship profile `[openbao]` section. Secrets are
    /// NOT in the profile — they come from environment variables.
    pub fn from_profile(profile: &OpenbaoConfig) -> Self {
        let secret_id = std::env::var("MAI_OPENBAO_SECRET_ID").ok();
        let wrapped_secret_id = std::env::var("MAI_OPENBAO_WRAPPED_SECRET_ID").ok();

        if secret_id.is_none() && wrapped_secret_id.is_none() {
            warn!(
                "MAI_OPENBAO_SECRET_ID and MAI_OPENBAO_WRAPPED_SECRET_ID are both unset; \
                 bridge client will fail closed on first use"
            );
        }

        Self {
            address: profile.address.clone(),
            role_id: profile.role_id.clone(),
            secret_id,
            wrapped_secret_id,
            timeout: Duration::from_secs(profile.timeout_secs),
            transit_keys: profile.transit.clone(),
            kv_paths: profile.kv.clone(),
            pki_role: profile.pki.clone(),
            use_mtls: false,
        }
    }

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
            transit_keys: TransitKeysConfig::default(),
            kv_paths: KvPathConfig::default(),
            pki_role: PkiRoleConfig::default(),
            use_mtls: false,
        }
    }

    /// Builder: override the AppRole secret_id (for hot-reload).
    #[must_use]
    pub fn with_secret_id(mut self, id: String) -> Self {
        self.secret_id = Some(id);
        self
    }

    /// Builder: override the wrapped secret_id token.
    #[must_use]
    pub fn with_wrapped_secret_id(mut self, token: String) -> Self {
        self.wrapped_secret_id = Some(token);
        self
    }

    /// Builder: enable mTLS for this config.
    #[must_use]
    pub fn with_mtls(mut self, enabled: bool) -> Self {
        self.use_mtls = enabled;
        self
    }

    /// Builder: set the PKI role name.
    #[must_use]
    pub fn with_pki_role(mut self, role: String) -> Self {
        self.pki_role = PkiRoleConfig { role };
        self
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

#[derive(Debug, Serialize)]
struct PkiIssueRequest {
    common_name: String,
    ttl: String,
}

#[derive(Debug, Deserialize)]
struct PkiIssueResponse {
    data: PkiIssueData,
}

#[derive(Debug, Deserialize)]
struct PkiIssueData {
    certificate: String,
    issuing_ca: String,
    ca_chain: Vec<String>,
    private_key: String,
    private_key_type: String,
    serial_number: String,
    expiration: i64,
}

#[derive(Debug, Deserialize)]
struct KvV2BundleResponse {
    data: KvV2BundleData,
}

#[derive(Debug, Deserialize)]
struct KvV2BundleData {
    data: serde_json::Value,
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

/// A certificate issued by the OpenBao PKI engine for mTLS.
#[derive(Debug, Clone)]
pub struct IssuedCertificate {
    pub certificate: String,
    pub private_key: String,
    pub ca_chain: Vec<String>,
    pub serial_number: String,
    pub expiration: i64,
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

    /// Build a client with mTLS enabled using PEM-encoded certificates.
    /// The client presents `client_cert_pem` + `client_key_pem` and
    /// trusts `ca_cert_pem` as the root of trust.
    pub fn new_with_mtls(
        config: OpenBaoBridgeConfig,
        ca_cert_pem: &str,
        client_cert_pem: &str,
        client_key_pem: &str,
    ) -> Result<Self, OpenBaoBridgeError> {
        let identity_pem = format!("{client_cert_pem}\n{client_key_pem}");
        let identity = reqwest::Identity::from_pem(identity_pem.as_bytes())
            .map_err(|e| OpenBaoBridgeError::Config(format!("client identity PEM parse: {e}")))?;
        let ca = reqwest::Certificate::from_pem(ca_cert_pem.as_bytes())
            .map_err(|e| OpenBaoBridgeError::Config(format!("CA certificate PEM parse: {e}")))?;
        let client = Client::builder()
            .identity(identity)
            .add_root_certificate(ca)
            .timeout(config.timeout)
            .build()
            .map_err(|e| OpenBaoBridgeError::Config(format!("mTLS client build: {e}")))?;
        Ok(Self {
            client,
            config: Arc::new(config),
        })
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
        let url = format!(
            "{}/v1/{}/{tenant_id}",
            self.config.address, self.config.kv_paths.tenant_path
        );
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

    /// Fetch revocation snapshots for a tenant from OpenBao KV.
    /// Returns an empty vec when the path does not exist or contains no
    /// snapshots (graceful first-boot / pre-provisioned state).
    pub async fn fetch_revocation_snapshots(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<RevocationSnapshot>, OpenBaoBridgeError> {
        let token = self.login().await?;
        let url = format!(
            "{}/v1/{}/{tenant_id}",
            self.config.address, self.config.kv_paths.revocation_path
        );
        let resp = self
            .client
            .get(&url)
            .header("X-Vault-Token", &token)
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            debug!(%tenant_id, "revocation path not found, returning empty");
            return Ok(Vec::new());
        }

        let body: KvV2Response = resp
            .json()
            .await
            .map_err(|e| OpenBaoBridgeError::Protocol(format!("revocation kv read parse: {e}")))?;

        // KV v2 stores snapshots under data.data, potentially as a
        // string-serialized array (bao kv put convention) or a raw array.
        let snapshots_val = body
            .data
            .data
            .get("snapshots")
            .cloned()
            .unwrap_or(body.data.data.clone());
        let snapshots: Vec<RevocationSnapshot> = if let serde_json::Value::String(s) =
            &snapshots_val
        {
            serde_json::from_str(s).map_err(|e| {
                OpenBaoBridgeError::Protocol(format!("revocation snapshots parse from string: {e}"))
            })?
        } else {
            serde_json::from_value(snapshots_val).map_err(|e| {
                OpenBaoBridgeError::Protocol(format!("revocation snapshots parse from object: {e}"))
            })?
        };
        debug!(
            count = snapshots.len(),
            %tenant_id,
            "Revocation snapshots fetched"
        );
        Ok(snapshots)
    }

    /// Sign a claim payload with the Lamprey claim-signer transit key.
    pub async fn sign_claim(
        &self,
        token: &str,
        claim_json: &str,
    ) -> Result<String, OpenBaoBridgeError> {
        let url = format!(
            "{}/v1/transit/sign/{}",
            self.config.address, self.config.transit_keys.claim_signer_key
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

    /// Issue a client certificate from the OpenBao PKI engine for mTLS.
    /// Returns the PEM-encoded certificate, private key, and CA chain.
    pub async fn issue_appliance_cert(
        &self,
        client_token: &str,
        common_name: &str,
        ttl: &str,
    ) -> Result<IssuedCertificate, OpenBaoBridgeError> {
        let url = format!(
            "{}/v1/pki/issue/{}",
            self.config.address, self.config.pki_role.role
        );
        let req = PkiIssueRequest {
            common_name: common_name.into(),
            ttl: ttl.into(),
        };
        let resp = self
            .client
            .post(&url)
            .header("X-Vault-Token", client_token)
            .json(&req)
            .send()
            .await?;
        let body: PkiIssueResponse = resp
            .json()
            .await
            .map_err(|e| OpenBaoBridgeError::Protocol(format!("pki issue parse: {e}")))?;
        info!(
            cn = %common_name,
            serial = %body.data.serial_number,
            expiration = body.data.expiration,
            "PKI certificate issued"
        );
        Ok(IssuedCertificate {
            certificate: body.data.certificate,
            private_key: body.data.private_key,
            ca_chain: body.data.ca_chain,
            serial_number: body.data.serial_number,
            expiration: body.data.expiration,
        })
    }

    /// Full claim issuance flow: auth → tenant lookup → claim build → sign.
    ///
    /// Returns a fully signed Lamprey claim suitable for injection into
    /// the local trust cache and for presentation to the policy runtime.
    ///
    /// When `trust_cache` is provided, the claim's `revocation_status`
    /// reflects the last-known snapshot (Valid, Revoked, or Unknown)
    /// instead of being hardcoded to `"valid"`.
    pub async fn issue_claim(
        &self,
        subject_id: &str,
        tenant_id: &str,
        roles: Vec<String>,
        trust_cache: Option<&Arc<RwLock<LocalTrustCache>>>,
    ) -> Result<SignedLampreyClaim, OpenBaoBridgeError> {
        let token = self.login().await?;
        let tenant = self.get_tenant(&token, tenant_id).await?;

        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expire_secs = now_secs + 900;

        let claim_id = format!("clm_{}_{:04x}", now_secs, rand::random::<u16>());

        let revocation_status = if let Some(cache) = trust_cache {
            let status = cache.read().await.revocation_status(&claim_id);
            match status {
                SnapshotStatus::Valid => "valid",
                SnapshotStatus::Revoked => "revoked",
                SnapshotStatus::Unknown => "unknown",
            }
        } else {
            "valid"
        };

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
            "revocation_status": revocation_status,
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
            "revocation_status": revocation_status,
            "signature": {
                "alg": "ed25519",
                "key_id": self.config.transit_keys.claim_signer_key,
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

    /// Fetch a signed policy bundle from OpenBao KV.
    /// Reads from `kv/data/bundles/{tenant_id}` and deserializes into a
    /// [`SignedPolicyBundle`]. Returns `None` when the path does not
    /// exist (graceful first-boot state).
    pub async fn fetch_signed_bundle(
        &self,
        tenant_id: &str,
    ) -> Result<Option<SignedPolicyBundle>, OpenBaoBridgeError> {
        let token = self.login().await?;
        let url = format!("{}/v1/kv/data/bundles/{tenant_id}", self.config.address);
        let resp = self
            .client
            .get(&url)
            .header("X-Vault-Token", &token)
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            debug!(%tenant_id, "bundle path not found in KV");
            return Ok(None);
        }

        let body: KvV2BundleResponse = resp
            .json()
            .await
            .map_err(|e| OpenBaoBridgeError::Protocol(format!("bundle kv read parse: {e}")))?;
        let bundle: SignedPolicyBundle = serde_json::from_value(body.data.data)
            .map_err(|e| OpenBaoBridgeError::Protocol(format!("bundle deserialize: {e}")))?;
        debug!(version = %bundle.metadata.version, %tenant_id, "Bundle fetched");
        Ok(Some(bundle))
    }

    /// Sign a policy bundle payload with the `lamprey-bundle-signer`
    /// Transit key. Returns the base64-encoded Ed25519 signature.
    pub async fn sign_bundle_payload(
        &self,
        token: &str,
        payload_json: &str,
    ) -> Result<String, OpenBaoBridgeError> {
        let url = format!(
            "{}/v1/transit/sign/lamprey-bundle-signer",
            self.config.address
        );
        let b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            payload_json.as_bytes(),
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
            .map_err(|e| OpenBaoBridgeError::Protocol(format!("bundle transit sign parse: {e}")))?;
        debug!(
            key_version = body.data.key_version,
            "Bundle payload signed via OpenBao transit"
        );
        Ok(body.data.signature)
    }

    /// Fetch a signed bundle from OpenBao KV and apply it to the trust
    /// cache via [`LocalTrustCache::record_refresh`] (bare-data path).
    ///
    /// Uses the bare-data path because the bundle is signed by
    /// OpenBao's Ed25519 Transit engine, whereas the ML-DSA verifier
    /// only accepts ML-DSA-87 signatures. Trust in the bundle is
    /// rooted in the authenticated AppRole connection to OpenBao
    /// rather than an offline signature check.
    ///
    /// Returns the bundle version on success, or `None` when no
    /// bundle exists at the KV path.
    pub async fn refresh_bundle_from_openbao(
        &self,
        tenant_id: &str,
        cache: &Arc<RwLock<LocalTrustCache>>,
    ) -> Result<Option<String>, OpenBaoBridgeError> {
        let bundle = match self.fetch_signed_bundle(tenant_id).await? {
            Some(b) => b,
            None => return Ok(None),
        };

        let now_secs = LocalTrustCache::now_secs();
        let version = bundle.metadata.version.clone();
        let snapshots = bundle.payload.revocations.clone();
        let refresh_secs = bundle.metadata.issued_at_secs;

        let mut lock = cache.write().await;
        if let Err(e) = lock.record_refresh(version.clone(), snapshots, refresh_secs, now_secs) {
            warn!(
                error = %e,
                version = %version,
                "Bundle refresh rejected by trust cache"
            );
            return Ok(None);
        }
        drop(lock);

        info!(
            version = %version,
            tenant = %tenant_id,
            "Bundle applied to trust cache"
        );
        Ok(Some(version))
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
