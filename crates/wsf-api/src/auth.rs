//! WSF transport authenticator seam (plan A2).
//!
//! Establishes the calling [`WsfPrincipal`] from a verified transport
//! credential *before* any privileged handler runs, and rejects missing,
//! malformed, expired, wrong-audience, and wrong-tenant credentials with
//! 401/403. The [`WsfAuthenticator`] trait is the seam; two implementations
//! ship:
//!
//! * [`WorkloadAuthenticator`] — verifies a signed [`WorkloadCredential`]
//!   presented as `Authorization: Workload <base64-json>`, checking signature,
//!   expiry, audience, and (optionally) a bound tenant. This is the production
//!   path (mTLS terminates at ingress and forwards a signed workload assertion;
//!   the same verification applies to a SPIFFE JWT-SVID once wired).
//! * [`LocalDevAuthenticator`] — an explicit development principal, never
//!   production-grade ([`AuthStrength::LocalDev`]).
//!
//! The [`require_principal`] middleware runs the authenticator and inserts the
//! principal into request extensions; handlers read it with
//! `Extension<WsfPrincipal>` (wired in A3).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use fabric_contracts::{Audience, AuthStrength, AuthenticatedFacts, IdentityKind, WsfPrincipal};
use fabric_crypto::Verifier;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use base64::Engine;

/// A signed workload credential presented at the transport edge. The caller
/// *claims* these facts; the authenticator verifies the signature over them
/// with the trusted authority key before any are believed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadCredential {
    /// Stable principal identifier (SPIFFE id, cert subject, AppRole name).
    pub principal_id: String,
    /// Principal kind.
    #[serde(default)]
    pub kind: IdentityKind,
    /// Tenant the credential is bound to.
    pub tenant_id: String,
    /// Pseudonymized subject (empty for pure workloads).
    #[serde(default)]
    pub subject_hash: String,
    /// Service identity, if any.
    #[serde(default)]
    pub service_identity: Option<String>,
    /// Plane this credential is minted for.
    pub audience: Audience,
    /// RFC3339 expiry. Past ⇒ rejected.
    pub expires_at: String,
    /// Base64 raw detached signature over [`Self::signing_bytes`].
    pub signature_b64: String,
}

impl WorkloadCredential {
    /// Canonical, length-prefixed signing preimage (domain-separated). Length
    /// prefixes make field boundaries unambiguous so no two distinct
    /// credentials share a preimage.
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut h = Sha256::new();
        let audience_tag = match self.audience {
            Audience::Wsf => "wsf",
            Audience::Aog => "aog",
            Audience::Mai => "mai",
        };
        for part in [
            b"wsf-workload-credential/v1".as_slice(),
            self.principal_id.as_bytes(),
            self.tenant_id.as_bytes(),
            self.subject_hash.as_bytes(),
            self.service_identity.as_deref().unwrap_or("").as_bytes(),
            audience_tag.as_bytes(),
            self.expires_at.as_bytes(),
        ] {
            h.update((part.len() as u64).to_le_bytes());
            h.update(part);
        }
        h.finalize().to_vec()
    }
}

/// Why authentication failed. Maps to the HTTP status the middleware returns
/// *before* the handler runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// No credential presented.
    MissingCredential,
    /// Credential present but unparseable (bad scheme, base64, or JSON).
    MalformedCredential(String),
    /// Signature did not verify against the trusted authority key.
    UntrustedCredential,
    /// Credential is past its expiry.
    ExpiredCredential,
    /// Credential is for a different plane than this ingress serves.
    WrongAudience { expected: Audience, got: Audience },
    /// Credential is for a different tenant than this ingress is bound to.
    WrongTenant,
}

impl AuthError {
    /// The HTTP status: 401 for "who are you / prove it" failures, 403 for
    /// "authenticated, but not for here" failures.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            AuthError::MissingCredential
            | AuthError::MalformedCredential(_)
            | AuthError::UntrustedCredential
            | AuthError::ExpiredCredential => StatusCode::UNAUTHORIZED,
            AuthError::WrongAudience { .. } | AuthError::WrongTenant => StatusCode::FORBIDDEN,
        }
    }

    fn public_message(&self) -> &'static str {
        // Deliberately terse: no oracle about which field mismatched.
        match self.status() {
            StatusCode::UNAUTHORIZED => "authentication required",
            _ => "not authorized for this resource",
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        (
            self.status(),
            Json(json!({ "error": self.public_message() })),
        )
            .into_response()
    }
}

/// The authenticator seam: turn transport headers into a verified principal.
pub trait WsfAuthenticator: Send + Sync {
    /// Establish the principal, or explain why not. Runs before every
    /// privileged handler.
    fn authenticate(&self, headers: &HeaderMap) -> Result<WsfPrincipal, AuthError>;
}

/// Verifies a signed [`WorkloadCredential`] against a trusted authority key.
pub struct WorkloadAuthenticator {
    verifier: Box<dyn Verifier>,
    authority_public_key: Vec<u8>,
    expected_audience: Audience,
    /// If set, only credentials for this exact tenant are accepted (per-tenant
    /// ingress). If `None`, any tenant the authority vouches for is admitted
    /// and tenant scoping is enforced downstream (A3/A4).
    bound_tenant: Option<String>,
}

impl WorkloadAuthenticator {
    /// New authenticator trusting `authority_public_key` for `audience`.
    #[must_use]
    pub fn new(
        verifier: Box<dyn Verifier>,
        authority_public_key: Vec<u8>,
        audience: Audience,
    ) -> Self {
        Self {
            verifier,
            authority_public_key,
            expected_audience: audience,
            bound_tenant: None,
        }
    }

    /// Bind this ingress to a single tenant (wrong-tenant ⇒ 403).
    #[must_use]
    pub fn bound_to_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.bound_tenant = Some(tenant_id.into());
        self
    }

    fn parse(headers: &HeaderMap) -> Result<WorkloadCredential, AuthError> {
        let raw = headers
            .get(axum::http::header::AUTHORIZATION)
            .ok_or(AuthError::MissingCredential)?
            .to_str()
            .map_err(|_| AuthError::MalformedCredential("non-ascii authorization".into()))?;
        let b64 = raw
            .strip_prefix("Workload ")
            .ok_or(AuthError::MalformedCredential(
                "expected `Workload` scheme".into(),
            ))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| AuthError::MalformedCredential(format!("base64: {e}")))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| AuthError::MalformedCredential(format!("json: {e}")))
    }
}

impl WsfAuthenticator for WorkloadAuthenticator {
    fn authenticate(&self, headers: &HeaderMap) -> Result<WsfPrincipal, AuthError> {
        let cred = Self::parse(headers)?;

        // 1. Signature over the canonical preimage. Do this FIRST — every field
        //    below is attacker-controlled until the signature is trusted.
        let sig = base64::engine::general_purpose::STANDARD
            .decode(cred.signature_b64.trim())
            .map_err(|_| AuthError::UntrustedCredential)?;
        let ok = self
            .verifier
            .verify(&cred.signing_bytes(), &sig, &self.authority_public_key)
            .map_err(|_| AuthError::UntrustedCredential)?;
        if !ok {
            return Err(AuthError::UntrustedCredential);
        }

        // 2. Expiry.
        let exp = DateTime::parse_from_rfc3339(&cred.expires_at)
            .map_err(|_| AuthError::MalformedCredential("expires_at not rfc3339".into()))?
            .with_timezone(&Utc);
        if Utc::now() >= exp {
            return Err(AuthError::ExpiredCredential);
        }

        // 3. Audience.
        if cred.audience != self.expected_audience {
            return Err(AuthError::WrongAudience {
                expected: self.expected_audience,
                got: cred.audience,
            });
        }

        // 4. Tenant binding, if this ingress is single-tenant.
        if let Some(bound) = &self.bound_tenant
            && &cred.tenant_id != bound
        {
            return Err(AuthError::WrongTenant);
        }

        Ok(WsfPrincipal::establish(
            AuthenticatedFacts {
                principal_id: cred.principal_id,
                kind: cred.kind,
                tenant_id: cred.tenant_id,
                subject_hash: cred.subject_hash,
                service_identity: cred.service_identity,
                auth_strength: AuthStrength::WorkloadToken,
                audience: cred.audience,
            },
            new_correlation_id(),
            Utc::now().to_rfc3339(),
        ))
    }
}

/// Explicit development authenticator: mints a fixed local-dev principal with
/// no credential required. Never production-grade.
pub struct LocalDevAuthenticator {
    tenant_id: String,
    audience: Audience,
    principal_id: String,
}

impl LocalDevAuthenticator {
    /// Dev principal for `tenant_id` on the WSF plane.
    #[must_use]
    pub fn for_wsf(tenant_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            audience: Audience::Wsf,
            principal_id: "local-dev".into(),
        }
    }
}

impl WsfAuthenticator for LocalDevAuthenticator {
    fn authenticate(&self, headers: &HeaderMap) -> Result<WsfPrincipal, AuthError> {
        // Optional overrides so a dev can act as a specific principal/subject.
        let principal_id =
            header_str(headers, "x-wsf-dev-principal").unwrap_or_else(|| self.principal_id.clone());
        let subject_hash = header_str(headers, "x-wsf-dev-subject").unwrap_or_default();
        Ok(WsfPrincipal::establish(
            AuthenticatedFacts {
                principal_id,
                kind: IdentityKind::Human,
                tenant_id: self.tenant_id.clone(),
                subject_hash,
                service_identity: None,
                auth_strength: AuthStrength::LocalDev,
                audience: self.audience,
            },
            new_correlation_id(),
            Utc::now().to_rfc3339(),
        ))
    }
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn new_correlation_id() -> String {
    format!("corr-{}", uuid::Uuid::new_v4())
}

/// Axum middleware: authenticate, then inject the [`WsfPrincipal`] into
/// extensions for the handler. On failure the request never reaches the
/// handler — 401/403 is returned here.
pub async fn require_principal(
    State(auth): State<Arc<dyn WsfAuthenticator>>,
    mut req: Request,
    next: Next,
) -> Response {
    match auth.authenticate(req.headers()) {
        Ok(principal) => {
            req.extensions_mut().insert(principal);
            next.run(req).await
        }
        Err(e) => e.into_response(),
    }
}

// Re-export so the credential minter (tests / a real issuer) can build the
// exact preimage the authenticator verifies.
pub use signing::mint_credential;

mod signing {
    use super::WorkloadCredential;
    use base64::Engine;
    use fabric_contracts::Audience;
    use fabric_crypto::Signer;

    /// Build and sign a [`WorkloadCredential`]. The credential authority (or a
    /// test) calls this; the authenticator verifies its output.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn mint_credential(
        signer: &dyn Signer,
        principal_id: impl Into<String>,
        tenant_id: impl Into<String>,
        subject_hash: impl Into<String>,
        service_identity: Option<String>,
        audience: Audience,
        expires_at: impl Into<String>,
    ) -> WorkloadCredential {
        let mut cred = WorkloadCredential {
            principal_id: principal_id.into(),
            kind: fabric_contracts::IdentityKind::Workload,
            tenant_id: tenant_id.into(),
            subject_hash: subject_hash.into(),
            service_identity,
            audience,
            expires_at: expires_at.into(),
            signature_b64: String::new(),
        };
        let sig = signer.sign(&cred.signing_bytes()).expect("sign credential");
        cred.signature_b64 = base64::engine::general_purpose::STANDARD.encode(sig);
        cred
    }
}
