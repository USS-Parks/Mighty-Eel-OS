//! `wsf-broker` — exchange a verified WSF trust token for ephemeral, scoped
//! cloud credentials (the net-new "sovereign STS broker").
//!
//! AWS first: verify the token → resolve the request's **tenant-scoped named
//! grant** (the caller never names a raw role ARN — AF-004) → read the broker's
//! root credentials from OpenBao (never exposed) → STS `AssumeRole` on the grant's
//! approved role with an **inline session policy** of the grant's actions on its
//! resources, further narrowed by the token's `ResourcePrefix` caveats → return
//! temporary credentials whose duration tracks the token TTL, capped by the
//! grant's max TTL. GCP (W7) and Azure (W8) follow the same shape.
//!
//! Fail-closed on trust: a token that does not verify, is revoked, or has expired
//! is refused **before** any AWS call; an unknown or cross-tenant grant is denied;
//! and a grant/caveat scope that resolves to nothing brokers a deny-all policy.

pub mod azure;
pub mod error;
pub mod gcp;
mod sigv4;
mod sts;

pub use azure::{AzureBroker, AzureBrokerConfig, AzureCredentials};
pub use error::BrokerError;
pub use gcp::{GcpBroker, GcpBrokerConfig, GcpCredentials};
pub use sts::{RootCredentials, TemporaryCredentials};

use chrono::{DateTime, Utc};
use fabric_contracts::{CaveatType, TrustToken};
use fabric_crypto::Verifier;
use wsf_bridge::OpenBaoAuth;

/// Static configuration for the AWS STS broker.
#[derive(Debug, Clone)]
pub struct BrokerConfig {
    /// AWS region, e.g. `us-east-1`.
    pub region: String,
    /// STS endpoint — LocalStack `http://localhost:4566` or
    /// `https://sts.<region>.amazonaws.com`.
    pub sts_endpoint: String,
    /// Full OpenBao API path holding the broker root creds, e.g.
    /// `kv/data/broker/aws-root`.
    pub root_cred_kv_path: String,
    /// STS duration floor (STS minimum is 900s).
    pub min_duration_secs: i64,
    /// STS duration ceiling (role max session duration, typically 3600s).
    pub max_duration_secs: i64,
}

impl BrokerConfig {
    /// Config with the STS default duration window (900s–3600s).
    #[must_use]
    pub fn new(
        region: impl Into<String>,
        sts_endpoint: impl Into<String>,
        root_cred_kv_path: impl Into<String>,
    ) -> Self {
        Self {
            region: region.into(),
            sts_endpoint: sts_endpoint.into(),
            root_cred_kv_path: root_cred_kv_path.into(),
            min_duration_secs: 900,
            max_duration_secs: 3600,
        }
    }
}

/// A tenant-scoped, server-side mapping from a named grant to the cloud identity
/// it authorizes. The public API selects a grant by id; it never names a raw role
/// ARN (AF-004). Loaded from signed / OpenBao-custodied policy in production.
#[derive(Debug, Clone)]
pub struct GrantMapping {
    /// Tenant that owns this grant.
    pub tenant_id: String,
    /// The approved role ARN to assume — server-side, never caller-supplied.
    pub role_arn: String,
    /// Allowed actions (e.g. `s3:GetObject`); empty ⇒ deny-all.
    pub actions: Vec<String>,
    /// Allowed resource ARNs / prefixes; empty ⇒ deny-all.
    pub resource_prefixes: Vec<String>,
    /// Optional region constraint (else the broker default).
    pub region: Option<String>,
    /// Maximum session TTL (seconds); the effective duration never exceeds it.
    pub max_ttl_secs: i64,
}

/// The server-side grant policy: the named grants a broker will honor. Empty by
/// default — an unknown grant is denied (fail closed).
#[derive(Debug, Clone, Default)]
pub struct GrantPolicy {
    grants: std::collections::HashMap<String, GrantMapping>,
}

impl GrantPolicy {
    /// An empty policy (every grant denied until one is registered).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: register `mapping` under `grant_id`.
    #[must_use]
    pub fn with_grant(mut self, grant_id: impl Into<String>, mapping: GrantMapping) -> Self {
        self.grants.insert(grant_id.into(), mapping);
        self
    }

    /// Resolve a grant for the token's tenant, or deny. The grant must exist and
    /// be owned by that tenant — a token cannot borrow another tenant's grant.
    ///
    /// # Errors
    /// [`BrokerError::GrantDenied`] if the grant is unknown or cross-tenant.
    pub fn resolve(&self, grant_id: &str, tenant_id: &str) -> Result<&GrantMapping, BrokerError> {
        let mapping = self
            .grants
            .get(grant_id)
            .ok_or_else(|| BrokerError::GrantDenied(format!("unknown grant '{grant_id}'")))?;
        if mapping.tenant_id != tenant_id {
            return Err(BrokerError::GrantDenied(
                "grant is not owned by the token's tenant".to_string(),
            ));
        }
        Ok(mapping)
    }
}

/// The AWS STS credential broker.
pub struct AwsStsBroker {
    openbao: OpenBaoAuth,
    http: reqwest::Client,
    config: BrokerConfig,
    grant_policy: GrantPolicy,
}

impl AwsStsBroker {
    /// Assemble a broker from an OpenBao client (root-cred custody), an HTTP
    /// client, and config. The grant policy starts empty (every grant denied);
    /// install one with [`with_grants`](AwsStsBroker::with_grants).
    #[must_use]
    pub fn new(openbao: OpenBaoAuth, http: reqwest::Client, config: BrokerConfig) -> Self {
        Self {
            openbao,
            http,
            config,
            grant_policy: GrantPolicy::new(),
        }
    }

    /// Builder: install the server-side grant policy (tenant → approved identity).
    #[must_use]
    pub fn with_grants(mut self, grant_policy: GrantPolicy) -> Self {
        self.grant_policy = grant_policy;
        self
    }

    /// Exchange a verified trust token for scoped temporary AWS credentials.
    ///
    /// # Errors
    /// [`BrokerError::TokenRejected`] / [`BrokerError::TokenExpired`] if the
    /// token fails trust checks (before any AWS call); [`BrokerError::OpenBao`]
    /// / [`BrokerError::RootCredential`] if root creds cannot be fetched; or an
    /// STS error.
    pub async fn broker_credentials(
        &self,
        token: &TrustToken,
        verifier: &dyn Verifier,
        public_key: &[u8],
        grant_id: &str,
        now: DateTime<Utc>,
    ) -> Result<TemporaryCredentials, BrokerError> {
        // 1. Fail closed on trust before touching AWS.
        verify_token(token, verifier, public_key, now)?;

        // 2. Resolve the named grant server-side (AF-004): the caller never names a
        //    raw role ARN; the grant maps its tenant + scope to an approved role.
        let mapping = self.grant_policy.resolve(grant_id, &token.tenant_id)?;

        // 3. Fetch the broker's root credentials from OpenBao (never exposed).
        let root = self.fetch_root_credentials().await?;

        // 4. Duration tracks the token TTL, clamped to the STS window AND the
        //    grant's maximum TTL.
        let window_max = self
            .config
            .max_duration_secs
            .min(mapping.max_ttl_secs)
            .max(self.config.min_duration_secs);
        let duration = clamp_duration(
            self.config.min_duration_secs,
            window_max,
            &token.expires_at,
            now,
        )?;

        // 5. Session policy: the grant's actions on its resources, narrowed by the
        //    token's resource caveats.
        let policy = build_session_policy(mapping, token);

        // 6. AssumeRole the grant's approved role (its region if it pins one).
        let region = mapping.region.as_deref().unwrap_or(&self.config.region);
        let (amz_date, datestamp) = amz_timestamps(now);
        let params = sts::AssumeRoleParams {
            endpoint: &self.config.sts_endpoint,
            region,
            role_arn: &mapping.role_arn,
            session_name: &session_name(token),
            session_policy: &policy,
            duration_secs: duration,
            amz_date: &amz_date,
            datestamp: &datestamp,
        };
        sts::assume_role(&self.http, &root, &params).await
    }

    async fn fetch_root_credentials(&self) -> Result<RootCredentials, BrokerError> {
        let vault_token = self.openbao.login().await?;
        let data = self
            .openbao
            .get_kv_data(&vault_token, &self.config.root_cred_kv_path)
            .await?;
        let access_key_id = data
            .get("access_key_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| BrokerError::RootCredential("missing access_key_id".into()))?
            .to_string();
        let secret_access_key = data
            .get("secret_access_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| BrokerError::RootCredential("missing secret_access_key".into()))?
            .to_string();
        let session_token = data
            .get("session_token")
            .and_then(|v| v.as_str())
            .map(String::from);
        Ok(RootCredentials {
            access_key_id,
            secret_access_key,
            session_token,
        })
    }
}

/// Build an inline STS session policy: the **grant's** approved actions on its
/// approved resources, further narrowed by the token's `ResourcePrefix` caveats
/// (a caveat can only shrink the grant, never widen it). Fails closed to deny-all
/// when the grant has no actions/resources or the caveats exclude everything.
#[must_use]
pub fn build_session_policy(mapping: &GrantMapping, token: &TrustToken) -> String {
    let caveats: Vec<&str> = token
        .attenuation
        .caveats
        .iter()
        .filter(|c| c.caveat_type == CaveatType::ResourcePrefix)
        .map(|c| c.value.as_str())
        .collect();
    let resources: Vec<&str> = if caveats.is_empty() {
        mapping
            .resource_prefixes
            .iter()
            .map(String::as_str)
            .collect()
    } else {
        mapping
            .resource_prefixes
            .iter()
            .map(String::as_str)
            .filter(|r| caveats.iter().any(|c| r.starts_with(c)))
            .collect()
    };
    if resources.is_empty() || mapping.actions.is_empty() {
        return r#"{"Version":"2012-10-17","Statement":[{"Sid":"WsfNoScope","Effect":"Deny","Action":"*","Resource":"*"}]}"#
            .to_string();
    }
    let resource_json = serde_json::to_string(&resources).unwrap_or_else(|_| "[]".into());
    let action_json = serde_json::to_string(&mapping.actions).unwrap_or_else(|_| "[]".into());
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"WsfGrantScoped","Effect":"Allow","Action":{action_json},"Resource":{resource_json}}}]}}"#
    )
}

/// Verify a presented trust token — fail closed on a bad signature / revocation
/// or on expiry, before any cloud call. Shared by the AWS and GCP brokers.
pub(crate) fn verify_token(
    token: &TrustToken,
    verifier: &dyn Verifier,
    public_key: &[u8],
    now: DateTime<Utc>,
) -> Result<(), BrokerError> {
    fabric_token::verify(token, verifier, public_key)
        .map_err(|e| BrokerError::TokenRejected(e.to_string()))?;
    if fabric_token::is_expired(token, now)
        .map_err(|e| BrokerError::TokenRejected(e.to_string()))?
    {
        return Err(BrokerError::TokenExpired);
    }
    Ok(())
}

/// Compute a cloud-cred duration: the token's remaining lifetime, clamped to
/// `[min, max]`. Errors only on a malformed `expires_at`.
pub(crate) fn clamp_duration(
    min: i64,
    max: i64,
    expires_at: &str,
    now: DateTime<Utc>,
) -> Result<i64, BrokerError> {
    let exp = DateTime::parse_from_rfc3339(expires_at)
        .map_err(|e| BrokerError::TokenRejected(format!("bad expires_at: {e}")))?
        .with_timezone(&Utc);
    let remaining = (exp - now).num_seconds();
    Ok(remaining.clamp(min, max))
}

/// STS `RoleSessionName` must match `[\w+=,.@-]{2,64}`; sanitize + cap the
/// token id defensively.
fn session_name(token: &TrustToken) -> String {
    let raw: String = token
        .token_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "+=,.@-_".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    raw.chars().take(64).collect()
}

fn amz_timestamps(now: DateTime<Utc>) -> (String, String) {
    (
        now.format("%Y%m%dT%H%M%SZ").to_string(),
        now.format("%Y%m%d").to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_contracts::{
        Attenuation, Caveat, Classification, RevocationStatus, Signature, TrustToken,
    };
    use fabric_crypto::Signer;
    use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};

    fn token_with(caveats: Vec<Caveat>, expires_at: &str) -> TrustToken {
        TrustToken {
            token_id: "tok_abc-123".to_string(),
            issued_at: "2026-07-03T12:00:00Z".to_string(),
            expires_at: expires_at.to_string(),
            issuer: "wsf-trust-bridge".to_string(),
            trust_bundle_version: "2026.07.03".to_string(),
            tenant_id: "tenant-a".to_string(),
            subject_id: None,
            subject_hash: "hmac-sha256:abc".to_string(),
            service_identity: None,
            identity_id: None,
            roles: vec![],
            compliance_scopes: vec![],
            allowed_routes: vec![],
            allowed_models: vec![],
            max_data_classification: Classification::Restricted,
            country: None,
            person_type: None,
            offline_mode: false,
            revocation_status: RevocationStatus::Valid,
            budget: None,
            attenuation: Attenuation {
                parent_id: None,
                caveats,
                depth: 0,
            },
            signature: Signature {
                alg: String::new(),
                key_id: String::new(),
                value: String::new(),
            },
        }
    }

    fn resource_caveat(value: &str) -> Caveat {
        Caveat {
            caveat_type: CaveatType::ResourcePrefix,
            value: value.to_string(),
        }
    }

    fn grant(resources: Vec<&str>) -> GrantMapping {
        GrantMapping {
            tenant_id: "tenant-a".to_string(),
            role_arn: "arn:aws:iam::0:role/wsf-approved".to_string(),
            actions: vec!["s3:GetObject".to_string()],
            resource_prefixes: resources.iter().map(|s| (*s).to_string()).collect(),
            region: None,
            max_ttl_secs: 3600,
        }
    }

    #[test]
    fn session_policy_scopes_to_the_grant() {
        // No token caveats → the grant's full approved resources + actions.
        let tok = token_with(vec![], "2026-07-03T12:15:00Z");
        let m = grant(vec![
            "arn:aws:s3:::wsf-demo/*",
            "arn:aws:s3:::wsf-shared/reports/*",
        ]);
        let policy = build_session_policy(&m, &tok);
        assert!(policy.contains("\"Effect\":\"Allow\""));
        assert!(policy.contains("s3:GetObject"));
        assert!(policy.contains("arn:aws:s3:::wsf-demo/*"));
        assert!(!policy.contains("arn:aws:s3:::other-bucket"));
    }

    #[test]
    fn token_caveat_narrows_the_grant() {
        // The grant allows two prefixes; the token's caveat permits only one.
        let tok = token_with(
            vec![resource_caveat("arn:aws:s3:::wsf-demo/")],
            "2026-07-03T12:15:00Z",
        );
        let m = grant(vec![
            "arn:aws:s3:::wsf-demo/reports/*",
            "arn:aws:s3:::wsf-other/*",
        ]);
        let policy = build_session_policy(&m, &tok);
        assert!(policy.contains("arn:aws:s3:::wsf-demo/reports/*"));
        assert!(!policy.contains("wsf-other"));
    }

    #[test]
    fn session_policy_denies_all_when_grant_has_no_resources() {
        let tok = token_with(vec![], "2026-07-03T12:15:00Z");
        let policy = build_session_policy(&grant(vec![]), &tok);
        assert!(policy.contains("\"Effect\":\"Deny\""));
        assert!(policy.contains("\"Resource\":\"*\""));
    }

    #[tokio::test]
    async fn unknown_grant_is_denied() {
        let signer = RustCryptoMlDsa87::generate("k").unwrap();
        let tok = fabric_token::issue(token_with(vec![], "2099-01-01T00:00:00Z"), &signer).unwrap();
        // Empty grant policy → any grant id is unknown.
        let broker = AwsStsBroker::new(
            OpenBaoAuth::new(wsf_bridge::OpenBaoConfig::new(
                "http://127.0.0.1:1",
                "r",
                "s",
            ))
            .unwrap(),
            reqwest::Client::new(),
            BrokerConfig::new("us-east-1", "http://127.0.0.1:1", "kv/data/broker/aws-root"),
        );
        let err = broker
            .broker_credentials(
                &tok,
                &MlDsa87Verifier,
                signer.public_key(),
                "nope",
                Utc::now(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, BrokerError::GrantDenied(_)));
    }

    #[tokio::test]
    async fn cross_tenant_grant_is_denied() {
        let signer = RustCryptoMlDsa87::generate("k").unwrap();
        // token_with is tenant-a; the grant is owned by tenant-b.
        let tok = fabric_token::issue(token_with(vec![], "2099-01-01T00:00:00Z"), &signer).unwrap();
        let mut m = grant(vec!["arn:aws:s3:::b/*"]);
        m.tenant_id = "tenant-b".to_string();
        let broker = AwsStsBroker::new(
            OpenBaoAuth::new(wsf_bridge::OpenBaoConfig::new(
                "http://127.0.0.1:1",
                "r",
                "s",
            ))
            .unwrap(),
            reqwest::Client::new(),
            BrokerConfig::new("us-east-1", "http://127.0.0.1:1", "kv/data/broker/aws-root"),
        )
        .with_grants(GrantPolicy::new().with_grant("g1", m));
        let err = broker
            .broker_credentials(
                &tok,
                &MlDsa87Verifier,
                signer.public_key(),
                "g1",
                Utc::now(),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, BrokerError::GrantDenied(_)),
            "cross-tenant grant must be denied, got {err:?}"
        );
    }

    #[test]
    fn duration_tracks_ttl_and_clamps() {
        let now = DateTime::parse_from_rfc3339("2026-07-03T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        // 15 min remaining → clamps up to the 900s floor exactly.
        assert_eq!(
            clamp_duration(900, 3600, "2026-07-03T12:15:00Z", now).unwrap(),
            900
        );
        // 2h remaining → clamps down to the 3600s ceiling.
        assert_eq!(
            clamp_duration(900, 3600, "2026-07-03T14:00:00Z", now).unwrap(),
            3600
        );
        // 30 min remaining → passes through.
        assert_eq!(
            clamp_duration(900, 3600, "2026-07-03T12:30:00Z", now).unwrap(),
            1800
        );
    }

    #[test]
    fn session_name_sanitizes_and_caps() {
        let mut tok = token_with(vec![], "2026-07-03T12:15:00Z");
        tok.token_id = "tok_/weird:id".to_string();
        let name = session_name(&tok);
        assert_eq!(name, "tok_-weird-id");
        assert!(name.len() <= 64);
    }

    #[tokio::test]
    async fn rejects_token_that_fails_verification() {
        // A validly-signed token verified against the WRONG public key must be
        // refused before any OpenBao/AWS call (dummy endpoints, never reached).
        let signer = RustCryptoMlDsa87::generate("k").unwrap();
        let wrong = RustCryptoMlDsa87::generate("other").unwrap();
        let tok = fabric_token::issue(
            token_with(
                vec![resource_caveat("arn:aws:s3:::b/*")],
                "2027-01-01T00:00:00Z",
            ),
            &signer,
        )
        .unwrap();

        let broker = AwsStsBroker::new(
            OpenBaoAuth::new(wsf_bridge::OpenBaoConfig::new(
                "http://127.0.0.1:1",
                "r",
                "s",
            ))
            .unwrap(),
            reqwest::Client::new(),
            BrokerConfig::new("us-east-1", "http://127.0.0.1:1", "kv/data/broker/aws-root"),
        );
        let now = Utc::now();
        let err = broker
            .broker_credentials(
                &tok,
                &MlDsa87Verifier,
                wrong.public_key(),
                "arn:aws:iam::0:role/x",
                now,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, BrokerError::TokenRejected(_)));
    }

    #[tokio::test]
    async fn rejects_expired_token() {
        let signer = RustCryptoMlDsa87::generate("k").unwrap();
        let tok = fabric_token::issue(
            token_with(
                vec![resource_caveat("arn:aws:s3:::b/*")],
                "2020-01-01T00:00:00Z",
            ),
            &signer,
        )
        .unwrap();
        let broker = AwsStsBroker::new(
            OpenBaoAuth::new(wsf_bridge::OpenBaoConfig::new(
                "http://127.0.0.1:1",
                "r",
                "s",
            ))
            .unwrap(),
            reqwest::Client::new(),
            BrokerConfig::new("us-east-1", "http://127.0.0.1:1", "kv/data/broker/aws-root"),
        );
        let err = broker
            .broker_credentials(
                &tok,
                &MlDsa87Verifier,
                signer.public_key(),
                "arn:aws:iam::0:role/x",
                Utc::now(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, BrokerError::TokenExpired));
    }
}
