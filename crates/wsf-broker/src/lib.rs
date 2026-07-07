//! `wsf-broker` — exchange a verified WSF trust token for ephemeral, scoped
//! cloud credentials (the net-new "sovereign STS broker").
//!
//! AWS first: verify the token → read the broker's root credentials from OpenBao
//! (never exposed) → STS `AssumeRole` with an **inline session policy** allowing
//! only the grant's approved IAM actions on the token's `ResourcePrefix` caveats
//! (plan B3: never `Action:"*"`) → return temporary credentials whose duration
//! tracks the token's remaining TTL, tightened by the grant's TTL ceiling. GCP
//! (W7) and Azure (W8) follow the same shape.
//!
//! Fail-closed on trust: a token that does not verify, is revoked, or has expired
//! is refused **before** any AWS call. A token with no resource scope — or a
//! grant with no approved actions — brokers a deny-all session policy (no
//! standing access).

pub mod azure;
pub mod error;
pub mod gcp;
mod sigv4;
mod sts;

pub use azure::{AzureBroker, AzureBrokerConfig, AzureCredentials};
pub use error::BrokerError;
pub use gcp::{GcpBroker, GcpBrokerConfig, GcpCredentials};
pub use sts::{RootCredentials, TemporaryCredentials};

use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use fabric_contracts::{CaveatType, TrustToken};
use fabric_crypto::Verifier;
use fabric_revocation::MonotonicRevocationStore;
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

/// Server-side scope for one credential exchange, resolved from an approved
/// grant (plan B3). Every field is server-side truth — the caller never
/// supplies any of it; the presented token can only narrow it further.
#[derive(Debug, Clone)]
pub struct GrantScope {
    /// The approved role ARN to assume.
    pub role_arn: String,
    /// IAM actions the inline session policy allows (least privilege). Empty
    /// means no actions were approved → the policy denies everything.
    pub allowed_actions: Vec<String>,
    /// Optional signing-region override; `None` uses the broker default.
    pub region: Option<String>,
    /// Optional `ExternalId` the role's trust policy requires
    /// (confused-deputy defense).
    pub external_id: Option<String>,
    /// Optional TTL ceiling (seconds) tightening the broker's max duration.
    pub max_ttl_secs: Option<i64>,
}

impl GrantScope {
    /// A scope for `role_arn` allowing `actions`, with broker-default region,
    /// no external id, and no extra TTL ceiling.
    #[must_use]
    pub fn new(role_arn: impl Into<String>, actions: &[&str]) -> Self {
        Self {
            role_arn: role_arn.into(),
            allowed_actions: actions.iter().map(ToString::to_string).collect(),
            region: None,
            external_id: None,
            max_ttl_secs: None,
        }
    }
}

/// The AWS STS credential broker.
pub struct AwsStsBroker {
    openbao: OpenBaoAuth,
    http: reqwest::Client,
    config: BrokerConfig,
    revocation: Option<Arc<RwLock<MonotonicRevocationStore>>>,
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
            revocation: None,
        }
    }

    /// Wire a revocation store (plan R consumer wiring). Once configured, the
    /// broker fails closed: every exchange requires a held, unexpired snapshot
    /// that does not revoke the presented token on any dimension.
    #[must_use]
    pub fn with_revocation_store(mut self, store: Arc<RwLock<MonotonicRevocationStore>>) -> Self {
        self.revocation = Some(store);
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
        grant: &GrantScope,
        now: DateTime<Utc>,
    ) -> Result<TemporaryCredentials, BrokerError> {
        // 1. Fail closed on trust before touching AWS.
        verify_token(token, verifier, public_key, self.revocation.as_deref(), now)?;

        // 2. Duration tracks the token TTL, clamped to the STS window; a grant
        //    TTL ceiling tightens the window but can never fall below the STS
        //    floor (that grant is refused, not widened). Checked before any
        //    OpenBao round-trip — a doomed request never touches custody.
        let max = grant
            .max_ttl_secs
            .map_or(self.config.max_duration_secs, |g| {
                g.min(self.config.max_duration_secs)
            });
        if max < self.config.min_duration_secs {
            return Err(BrokerError::Grant(format!(
                "grant max_ttl {max}s is below the STS floor of {}s",
                self.config.min_duration_secs
            )));
        }
        let duration = clamp_duration(self.config.min_duration_secs, max, &token.expires_at, now)?;

        // 3. Session policy: the grant's approved actions on the token's
        //    resource scope — never `Action:"*"` (B3).
        let policy = build_session_policy(token, &grant.allowed_actions);

        // 4. Fetch the broker's root credentials from OpenBao (never exposed).
        let root = self.fetch_root_credentials().await?;

        // 5. AssumeRole.
        let (amz_date, datestamp) = amz_timestamps(now);
        let params = sts::AssumeRoleParams {
            endpoint: &self.config.sts_endpoint,
            region: grant.region.as_deref().unwrap_or(&self.config.region),
            role_arn: &grant.role_arn,
            session_name: &session_name(token),
            session_policy: &policy,
            duration_secs: duration,
            external_id: grant.external_id.as_deref(),
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

/// Build an inline STS session policy: the grant's approved IAM actions on the
/// token's `ResourcePrefix` caveats (plan B3 — the old policy allowed
/// `Action:"*"` on the scoped resources).
///
/// Fail closed on both axes: no resource caveats **or** no approved actions →
/// a deny-all policy (a token/grant with no scope brokers no standing access).
/// If the token carries `ToolAllowlist` caveats, they intersect the grant's
/// actions — an attenuated token can narrow the approved set, never widen it.
#[must_use]
pub fn build_session_policy(token: &TrustToken, allowed_actions: &[String]) -> String {
    const DENY_ALL: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"WsfNoScope","Effect":"Deny","Action":"*","Resource":"*"}]}"#;

    let resources: Vec<&str> = token
        .attenuation
        .caveats
        .iter()
        .filter(|c| c.caveat_type == CaveatType::ResourcePrefix)
        .map(|c| c.value.as_str())
        .collect();
    let tool_caveats: Vec<&str> = token
        .attenuation
        .caveats
        .iter()
        .filter(|c| c.caveat_type == CaveatType::ToolAllowlist)
        .map(|c| c.value.as_str())
        .collect();
    let actions: Vec<&str> = allowed_actions
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "*" && (tool_caveats.is_empty() || tool_caveats.contains(a)))
        .collect();
    if resources.is_empty() || actions.is_empty() {
        return DENY_ALL.to_string();
    }
    let resource_json = serde_json::to_string(&resources).unwrap_or_else(|_| "[]".into());
    let action_json = serde_json::to_string(&actions).unwrap_or_else(|_| "[]".into());
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"WsfScopedResources","Effect":"Allow","Action":{action_json},"Resource":{resource_json}}}]}}"#
    )
}

/// Verify a presented trust token — fail closed on a bad signature / revocation
/// or on expiry, before any cloud call. Shared by the AWS, GCP, and Azure
/// brokers. When a revocation store is wired (plan R consumer wiring), the
/// broker fails closed: no held snapshot, an expired snapshot, or a snapshot
/// revoking the token on any dimension all refuse the exchange.
pub(crate) fn verify_token(
    token: &TrustToken,
    verifier: &dyn Verifier,
    public_key: &[u8],
    revocation: Option<&RwLock<MonotonicRevocationStore>>,
    now: DateTime<Utc>,
) -> Result<(), BrokerError> {
    fabric_token::verify(token, verifier, public_key)
        .map_err(|e| BrokerError::TokenRejected(e.to_string()))?;
    if fabric_token::is_expired(token, now)
        .map_err(|e| BrokerError::TokenRejected(e.to_string()))?
    {
        return Err(BrokerError::TokenExpired);
    }
    let Some(store) = revocation else {
        return Ok(());
    };
    let store = store.read().expect("revocation store lock");
    let Some(snapshot) = store.current() else {
        return Err(BrokerError::TokenRejected(
            "revocation state unavailable (fail closed)".to_string(),
        ));
    };
    let fresh = DateTime::parse_from_rfc3339(&snapshot.expires_at)
        .map(|e| e.with_timezone(&Utc) > now)
        .unwrap_or(false);
    if !fresh {
        return Err(BrokerError::TokenRejected(
            "revocation snapshot expired (fail closed)".to_string(),
        ));
    }
    if let Some(dimension) = snapshot.revokes(token) {
        return Err(BrokerError::TokenRejected(format!(
            "token revoked ({dimension})"
        )));
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

    fn actions(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn session_policy_scopes_to_resource_caveats_and_grant_actions() {
        let tok = token_with(
            vec![
                resource_caveat("arn:aws:s3:::wsf-demo/*"),
                resource_caveat("arn:aws:s3:::wsf-shared/reports/*"),
            ],
            "2026-07-03T12:15:00Z",
        );
        let policy = build_session_policy(&tok, &actions(&["s3:GetObject", "s3:ListBucket"]));
        assert!(policy.contains("\"Effect\":\"Allow\""));
        assert!(policy.contains("arn:aws:s3:::wsf-demo/*"));
        assert!(policy.contains("arn:aws:s3:::wsf-shared/reports/*"));
        // B3: the action list is exactly the grant's — no wildcard anywhere.
        assert!(policy.contains(r#""Action":["s3:GetObject","s3:ListBucket"]"#));
        assert!(!policy.contains(r#""Action":"*""#));
        // A resource NOT granted is outside the policy → implicitly denied.
        assert!(!policy.contains("arn:aws:s3:::other-bucket"));
    }

    #[test]
    fn session_policy_denies_all_without_scope() {
        let tok = token_with(vec![], "2026-07-03T12:15:00Z");
        let policy = build_session_policy(&tok, &actions(&["s3:GetObject"]));
        assert!(policy.contains("\"Effect\":\"Deny\""));
        assert!(policy.contains("\"Resource\":\"*\""));
    }

    #[test]
    fn session_policy_denies_all_without_grant_actions() {
        // Resources scoped but the grant approved no actions → deny-all.
        let tok = token_with(
            vec![resource_caveat("arn:aws:s3:::wsf-demo/*")],
            "2026-07-03T12:15:00Z",
        );
        let policy = build_session_policy(&tok, &[]);
        assert!(policy.contains("\"Effect\":\"Deny\""));
    }

    #[test]
    fn wildcard_grant_action_is_refused_not_widened() {
        // A grant misconfigured with "*" must not reproduce the old
        // Action:"*" policy — the wildcard is dropped; alone, it denies all.
        let tok = token_with(
            vec![resource_caveat("arn:aws:s3:::wsf-demo/*")],
            "2026-07-03T12:15:00Z",
        );
        let policy = build_session_policy(&tok, &actions(&["*"]));
        assert!(policy.contains("\"Effect\":\"Deny\""));
        let mixed = build_session_policy(&tok, &actions(&["*", "s3:GetObject"]));
        assert!(mixed.contains(r#""Action":["s3:GetObject"]"#));
        assert!(!mixed.contains(r#""Action":"*""#));
    }

    #[test]
    fn tool_allowlist_caveats_narrow_grant_actions() {
        // An attenuated token carrying ToolAllowlist caveats intersects the
        // grant's actions — it can narrow, never widen.
        let tok = token_with(
            vec![
                resource_caveat("arn:aws:s3:::wsf-demo/*"),
                Caveat {
                    caveat_type: CaveatType::ToolAllowlist,
                    value: "s3:GetObject".to_string(),
                },
            ],
            "2026-07-03T12:15:00Z",
        );
        let policy = build_session_policy(&tok, &actions(&["s3:GetObject", "s3:PutObject"]));
        assert!(policy.contains(r#""Action":["s3:GetObject"]"#));
        assert!(!policy.contains("s3:PutObject"));

        // A caveat naming an action the grant never approved adds nothing.
        let tok2 = token_with(
            vec![
                resource_caveat("arn:aws:s3:::wsf-demo/*"),
                Caveat {
                    caveat_type: CaveatType::ToolAllowlist,
                    value: "iam:CreateUser".to_string(),
                },
            ],
            "2026-07-03T12:15:00Z",
        );
        let policy2 = build_session_policy(&tok2, &actions(&["s3:GetObject"]));
        assert!(
            policy2.contains("\"Effect\":\"Deny\""),
            "empty intersection denies"
        );
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
        assert!(
            build_session_policy(&tok, &actions(&["s3:GetObject"])).contains("\"Effect\":\"Deny\"")
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
                &GrantScope::new("arn:aws:iam::0:role/x", &["s3:GetObject"]),
                now,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, BrokerError::TokenRejected(_)));
    }

    #[tokio::test]
    async fn refuses_grant_with_ttl_ceiling_below_sts_floor() {
        // A grant capping TTL below the STS 900s floor is refused before any
        // OpenBao or AWS call (dummy endpoints, never reached) — never widened
        // back up to the floor.
        let signer = RustCryptoMlDsa87::generate("k").unwrap();
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
        let mut grant = GrantScope::new("arn:aws:iam::0:role/x", &["s3:GetObject"]);
        grant.max_ttl_secs = Some(600); // below the 900s STS floor
        let err = broker
            .broker_credentials(
                &tok,
                &MlDsa87Verifier,
                signer.public_key(),
                &grant,
                Utc::now(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, BrokerError::Grant(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn revoked_token_is_refused_before_any_cloud_call() {
        // R consumer wiring: with a store configured, a revoked tenant is
        // refused before OpenBao or STS are touched (dummy endpoints, never
        // reached) — and an empty store fails closed the same way.
        let signer = RustCryptoMlDsa87::generate("k").unwrap();
        let rev_anchor = RustCryptoMlDsa87::generate("rev-anchor").unwrap();
        let tok = fabric_token::issue(
            token_with(
                vec![resource_caveat("arn:aws:s3:::b/*")],
                "2027-01-01T00:00:00Z",
            ),
            &signer,
        )
        .unwrap();

        let store = Arc::new(RwLock::new(MonotonicRevocationStore::new()));
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
        .with_revocation_store(store.clone());
        let grant = GrantScope::new("arn:aws:iam::0:role/x", &["s3:GetObject"]);

        // Empty store → fail closed.
        let err = broker
            .broker_credentials(
                &tok,
                &MlDsa87Verifier,
                signer.public_key(),
                &grant,
                Utc::now(),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(&err, BrokerError::TokenRejected(m) if m.contains("unavailable")),
            "got {err:?}"
        );

        // Snapshot revoking the token's tenant → refused with the dimension.
        let mut snap = fabric_revocation::RevocationSnapshot::new(
            "rev-1",
            "2026-07-07T00:00:00Z",
            "2027-01-01T00:00:00Z",
        )
        .with_sequence(1);
        snap.revoked_tenants.push("tenant-a".to_string());
        let snap = fabric_revocation::sign(snap, &rev_anchor).unwrap();
        store
            .write()
            .unwrap()
            .advance(snap, &MlDsa87Verifier, rev_anchor.public_key())
            .unwrap();
        let err = broker
            .broker_credentials(
                &tok,
                &MlDsa87Verifier,
                signer.public_key(),
                &grant,
                Utc::now(),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(&err, BrokerError::TokenRejected(m) if m.contains("revoked (tenant)")),
            "got {err:?}"
        );
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
                &GrantScope::new("arn:aws:iam::0:role/x", &["s3:GetObject"]),
                Utc::now(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, BrokerError::TokenExpired));
    }
}
