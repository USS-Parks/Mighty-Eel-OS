//! `wsf-broker` — exchange a verified WSF trust token for ephemeral, scoped
//! cloud credentials (the net-new "sovereign STS broker").
//!
//! AWS first: verify the token → read the broker's root credentials from OpenBao
//! (never exposed) → STS `AssumeRole` with an **inline session policy** narrowed
//! to the token's `ResourcePrefix` caveats → return temporary credentials whose
//! duration tracks the token's remaining TTL. GCP (W7) and Azure (W8) follow the
//! same shape.
//!
//! Fail-closed on trust: a token that does not verify, is revoked, or has expired
//! is refused **before** any AWS call. A token that carries no resource scope
//! brokers a deny-all session policy (no standing access).

pub mod error;
pub mod gcp;
mod sigv4;
mod sts;

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

/// The AWS STS credential broker.
pub struct AwsStsBroker {
    openbao: OpenBaoAuth,
    http: reqwest::Client,
    config: BrokerConfig,
}

impl AwsStsBroker {
    /// Assemble a broker from an OpenBao client (root-cred custody), an HTTP
    /// client, and config.
    #[must_use]
    pub fn new(openbao: OpenBaoAuth, http: reqwest::Client, config: BrokerConfig) -> Self {
        Self {
            openbao,
            http,
            config,
        }
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
        role_arn: &str,
        now: DateTime<Utc>,
    ) -> Result<TemporaryCredentials, BrokerError> {
        // 1. Fail closed on trust before touching AWS.
        verify_token(token, verifier, public_key, now)?;

        // 2. Fetch the broker's root credentials from OpenBao (never exposed).
        let root = self.fetch_root_credentials().await?;

        // 3. Duration tracks the token TTL, clamped to the STS window.
        let duration = clamp_duration(
            self.config.min_duration_secs,
            self.config.max_duration_secs,
            &token.expires_at,
            now,
        )?;

        // 4. Session policy narrowed to the token's resource scope.
        let policy = build_session_policy(token);

        // 5. AssumeRole.
        let (amz_date, datestamp) = amz_timestamps(now);
        let params = sts::AssumeRoleParams {
            endpoint: &self.config.sts_endpoint,
            region: &self.config.region,
            role_arn,
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

/// Build an inline STS session policy scoping the assumed role to the token's
/// `ResourcePrefix` caveats. With no resource caveats it denies everything
/// (fail closed — a token with no scope brokers no standing access).
#[must_use]
pub fn build_session_policy(token: &TrustToken) -> String {
    let resources: Vec<&str> = token
        .attenuation
        .caveats
        .iter()
        .filter(|c| c.caveat_type == CaveatType::ResourcePrefix)
        .map(|c| c.value.as_str())
        .collect();
    if resources.is_empty() {
        return r#"{"Version":"2012-10-17","Statement":[{"Sid":"WsfNoScope","Effect":"Deny","Action":"*","Resource":"*"}]}"#
            .to_string();
    }
    let resource_json = serde_json::to_string(&resources).unwrap_or_else(|_| "[]".into());
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"WsfScopedResources","Effect":"Allow","Action":"*","Resource":{resource_json}}}]}}"#
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

    #[test]
    fn session_policy_scopes_to_resource_caveats() {
        let tok = token_with(
            vec![
                resource_caveat("arn:aws:s3:::wsf-demo/*"),
                resource_caveat("arn:aws:s3:::wsf-shared/reports/*"),
            ],
            "2026-07-03T12:15:00Z",
        );
        let policy = build_session_policy(&tok);
        assert!(policy.contains("\"Effect\":\"Allow\""));
        assert!(policy.contains("arn:aws:s3:::wsf-demo/*"));
        assert!(policy.contains("arn:aws:s3:::wsf-shared/reports/*"));
        // A resource NOT granted is outside the policy → implicitly denied.
        assert!(!policy.contains("arn:aws:s3:::other-bucket"));
    }

    #[test]
    fn session_policy_denies_all_without_scope() {
        let tok = token_with(vec![], "2026-07-03T12:15:00Z");
        let policy = build_session_policy(&tok);
        assert!(policy.contains("\"Effect\":\"Deny\""));
        assert!(policy.contains("\"Resource\":\"*\""));
    }

    #[test]
    fn non_resource_caveats_do_not_widen_scope() {
        let tok = token_with(
            vec![Caveat {
                caveat_type: CaveatType::ToolAllowlist,
                value: "s3:GetObject".to_string(),
            }],
            "2026-07-03T12:15:00Z",
        );
        // No ResourcePrefix caveats → deny-all (fail closed).
        assert!(build_session_policy(&tok).contains("\"Effect\":\"Deny\""));
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
