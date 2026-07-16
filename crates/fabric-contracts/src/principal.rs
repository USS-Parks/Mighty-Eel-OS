//! WSF principal contract (plan A1) — see `contracts/identity.md`.
//!
//! A [`WsfPrincipal`] is the **authenticated** answer to *who is calling*,
//! established by the transport authenticator ([`crate` consumer] `wsf-api`'s
//! authenticator seam, plan A2) from a verified credential — mTLS client cert,
//! workload token, or an explicit local-dev credential. It is deliberately
//! **not** the same thing as an [`Identity`](crate::Identity) claim carried in a
//! request body: a caller may *assert* an identity in JSON, but only the
//! authenticator may *establish* a principal.
//!
//! # Why this type does not implement `Deserialize`
//!
//! The single most damaging WSF finding was that privileged routes
//! read `tenant_id` / `subject_id` / `roles` straight from request JSON, so any
//! caller could self-assign authority. The structural fix is a type the wire
//! layer *cannot* manufacture: `WsfPrincipal` derives [`Serialize`] (so it can
//! be written into receipts and audit records) but **not** `Deserialize`.
//!
//! Because of that, none of these compile — the boundary is enforced by the
//! type system, not by review vigilance:
//!
//! ```compile_fail
//! use fabric_contracts::WsfPrincipal;
//! // axum's `Json<T>` / `serde_json::from_str::<T>` both require `T: Deserialize`.
//! let _p: WsfPrincipal = serde_json::from_str("{}").unwrap();
//! ```
//!
//! ```compile_fail
//! use fabric_contracts::WsfPrincipal;
//! fn assert_de<T: serde::de::DeserializeOwned>() {}
//! assert_de::<WsfPrincipal>(); // WsfPrincipal: DeserializeOwned is not satisfied
//! ```
//!
//! The only way to obtain one is [`WsfPrincipal::establish`], which the
//! authenticator calls after it has verified a credential.

use serde::{Deserialize, Serialize};

use crate::identity::IdentityKind;

/// How strongly the calling principal was authenticated. Ordered weakest →
/// strongest; production policy (plan A2/V1) rejects [`AuthStrength::LocalDev`].
///
/// This value enum is `Deserialize` (config/credential parsing needs it); the
/// no-JSON boundary applies to [`WsfPrincipal`] as a whole, not its fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStrength {
    /// Explicit local-dev credential. Never valid under the production profile.
    LocalDev,
    /// Bearer workload token (e.g. OpenBao AppRole secret-id, SPIFFE JWT-SVID).
    WorkloadToken,
    /// Mutual TLS with a verified client-certificate chain.
    MutualTls,
}

impl AuthStrength {
    /// Whether this strength is acceptable outside local development.
    #[must_use]
    pub fn is_production_grade(self) -> bool {
        !matches!(self, AuthStrength::LocalDev)
    }
}

/// The plane a principal is authenticated *for*. A token minted for one
/// audience must not verify at another (plan T1/A4 audience binding).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Audience {
    /// The WSF trust-fabric control plane (token issue / attenuate / seal).
    Wsf,
    /// The AOG gateway.
    Aog,
    /// The MAI inference API.
    Mai,
}

/// The authenticated principal behind a privileged call.
///
/// Constructed only by [`WsfPrincipal::establish`] from a verified credential;
/// never deserialized from a request. Carries no cleartext subject — only the
/// pseudonymized `subject_hash`, matching the token/receipt contracts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WsfPrincipal {
    /// Stable identifier of the authenticated principal (e.g. SPIFFE ID,
    /// certificate subject, or AppRole role name).
    pub principal_id: String,
    /// What kind of principal this is.
    pub kind: IdentityKind,
    /// Tenant the credential is bound to. Server-authoritative — the caller
    /// cannot influence it.
    pub tenant_id: String,
    /// Pseudonymized subject. Empty for pure workload principals.
    pub subject_hash: String,
    /// Service identity for workload/service principals, if any.
    pub service_identity: Option<String>,
    /// Roles proven by the authenticator. An empty set grants no role-based
    /// authority; request payload roles never populate this field.
    pub roles: Vec<String>,
    /// Immutable root token lineage when the authenticating credential is
    /// itself token-derived. Workload identities that are not token-derived
    /// carry `None`.
    pub token_lineage: Option<String>,
    /// How strongly the principal was authenticated.
    pub auth_strength: AuthStrength,
    /// Plane this principal is authenticated for.
    pub audience: Audience,
    /// Correlation id threaded into every receipt this principal produces.
    pub correlation_id: String,
    /// RFC3339 instant the authenticator established this principal.
    pub authenticated_at: String,
    /// Private witness: makes `WsfPrincipal { .. }` literal construction
    /// impossible outside this module, so [`establish`](Self::establish) is the
    /// *only* constructor even within the crate's own consumers.
    _sealed: Sealed,
}

/// Zero-size construction witness. Private field ⇒ no external struct literal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Sealed;

/// Everything the authenticator has proven about a credential, minus the
/// derived plumbing (`correlation_id`, `authenticated_at`) that `establish`
/// stamps. Grouping the proven facts keeps the constructor call-site honest.
#[derive(Debug, Clone)]
pub struct AuthenticatedFacts {
    pub principal_id: String,
    pub kind: IdentityKind,
    pub tenant_id: String,
    pub subject_hash: String,
    pub service_identity: Option<String>,
    pub roles: Vec<String>,
    pub token_lineage: Option<String>,
    pub auth_strength: AuthStrength,
    pub audience: Audience,
}

impl WsfPrincipal {
    /// Establish a principal from facts a credential authenticator has already
    /// verified. This is the sole constructor; the wire layer has no path to it.
    #[must_use]
    pub fn establish(
        facts: AuthenticatedFacts,
        correlation_id: impl Into<String>,
        authenticated_at: impl Into<String>,
    ) -> Self {
        Self {
            principal_id: facts.principal_id,
            kind: facts.kind,
            tenant_id: facts.tenant_id,
            subject_hash: facts.subject_hash,
            service_identity: facts.service_identity,
            roles: facts.roles,
            token_lineage: facts.token_lineage,
            auth_strength: facts.auth_strength,
            audience: facts.audience,
            correlation_id: correlation_id.into(),
            authenticated_at: authenticated_at.into(),
            _sealed: Sealed,
        }
    }

    /// Whether this principal may act on the given plane.
    #[must_use]
    pub fn is_for(&self, audience: Audience) -> bool {
        self.audience == audience
    }
}

/// Privileged operation established by the server after routing. Each variant
/// has exactly one valid audience, preventing a WSF-authenticated identity from
/// being reused on an AOG sink (and vice versa).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOperation {
    WsfIssue,
    WsfVerify,
    WsfAttenuate,
    WsfSeal,
    WsfUnseal,
    WsfBroker,
    WsfAuditRead,
    WsfAuditExport,
    AogCreate,
    AogRead,
    AogList,
    AogUpdate,
    AogDelete,
}

impl RequestOperation {
    #[must_use]
    pub fn audience(self) -> Audience {
        match self {
            Self::WsfIssue
            | Self::WsfVerify
            | Self::WsfAttenuate
            | Self::WsfSeal
            | Self::WsfUnseal
            | Self::WsfBroker
            | Self::WsfAuditRead
            | Self::WsfAuditExport => Audience::Wsf,
            Self::AogCreate | Self::AogRead | Self::AogList | Self::AogUpdate | Self::AogDelete => {
                Audience::Aog
            }
        }
    }
}

/// Final canonical resource selected by server routing and lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CanonicalResource {
    kind: String,
    name: String,
    tenant_id: Option<String>,
}

impl CanonicalResource {
    /// Construct a server-resolved resource. Empty kind/name values are denied
    /// so callers cannot smuggle an ambiguous authorization target.
    pub fn resolved(
        kind: impl Into<String>,
        name: impl Into<String>,
        tenant_id: Option<String>,
    ) -> Result<Self, RequestContextError> {
        let kind = kind.into();
        let name = name.into();
        if kind.trim().is_empty() || name.trim().is_empty() {
            return Err(RequestContextError::AmbiguousResource);
        }
        Ok(Self {
            kind,
            name,
            tenant_id,
        })
    }

    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn tenant_id(&self) -> Option<&str> {
        self.tenant_id.as_deref()
    }
}

/// Authenticated, server-routed context required by privileged boundaries.
/// It is serializable for receipts but deliberately not deserializable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifiedRequestContext {
    principal: WsfPrincipal,
    operation: RequestOperation,
    resource: CanonicalResource,
}

impl VerifiedRequestContext {
    /// Bind an authenticated principal to the operation and final canonical
    /// resource selected by the server. Wrong-audience use fails before the
    /// privileged sink receives a context.
    pub fn establish(
        principal: WsfPrincipal,
        operation: RequestOperation,
        resource: CanonicalResource,
    ) -> Result<Self, RequestContextError> {
        if !principal.is_for(operation.audience()) {
            return Err(RequestContextError::WrongAudience {
                expected: operation.audience(),
                got: principal.audience,
            });
        }
        if let (Some(principal_tenant), Some(resource_tenant)) =
            (Some(principal.tenant_id.as_str()), resource.tenant_id())
            && principal_tenant != resource_tenant
        {
            return Err(RequestContextError::WrongTenant);
        }
        Ok(Self {
            principal,
            operation,
            resource,
        })
    }

    #[must_use]
    pub fn principal(&self) -> &WsfPrincipal {
        &self.principal
    }

    #[must_use]
    pub fn operation(&self) -> RequestOperation {
        self.operation
    }

    #[must_use]
    pub fn resource(&self) -> &CanonicalResource {
        &self.resource
    }

    /// Prove that this context was established for the exact operation a
    /// privileged sink is about to perform. Sinks call this before side
    /// effects so a context for one operation cannot be replayed at another.
    pub fn require_operation(&self, expected: RequestOperation) -> Result<(), RequestContextError> {
        if self.operation != expected {
            return Err(RequestContextError::WrongOperation {
                expected,
                got: self.operation,
            });
        }
        Ok(())
    }
}

/// Failure to establish a privileged request context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestContextError {
    WrongAudience {
        expected: Audience,
        got: Audience,
    },
    WrongOperation {
        expected: RequestOperation,
        got: RequestOperation,
    },
    WrongTenant,
    AmbiguousResource,
}

impl std::fmt::Display for RequestContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongAudience { expected, got } => {
                write!(
                    f,
                    "request audience mismatch: expected {expected:?}, got {got:?}"
                )
            }
            Self::WrongOperation { expected, got } => {
                write!(
                    f,
                    "request operation mismatch: expected {expected:?}, got {got:?}"
                )
            }
            Self::WrongTenant => f.write_str("request resource tenant does not match principal"),
            Self::AmbiguousResource => f.write_str("canonical resource kind and name are required"),
        }
    }
}

impl std::error::Error for RequestContextError {}

/// Explicit privileged capability proven from server-authenticated roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivilegedCapability {
    TenantRevocation,
    EstateRevocation,
    GlobalObjectMutation,
    RingKeyDestruction,
    PolicyPublication,
}

impl PrivilegedCapability {
    #[must_use]
    pub fn required_role(self) -> &'static str {
        match self {
            Self::TenantRevocation => "tenant:revocation",
            Self::EstateRevocation => "estate:revocation",
            Self::GlobalObjectMutation => "estate:global-mutation",
            Self::RingKeyDestruction => "estate:ring-key-destruction",
            Self::PolicyPublication => "estate:policy-publication",
        }
    }

    #[must_use]
    pub fn is_estate(self) -> bool {
        !matches!(self, Self::TenantRevocation)
    }
}

/// Proof that a request may act only inside one authenticated tenant.
/// Private fields and no `Deserialize` prevent request JSON from manufacturing
/// this scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TenantScope {
    tenant_id: String,
    capability: PrivilegedCapability,
}

impl TenantScope {
    /// Authorize an exact tenant capability against the final canonical
    /// resource. Estate capabilities can never produce this type.
    pub fn authorize(
        context: &VerifiedRequestContext,
        capability: PrivilegedCapability,
    ) -> Result<Self, ScopeAuthorizationError> {
        if capability.is_estate() {
            return Err(ScopeAuthorizationError::WrongScope);
        }
        let principal = context.principal();
        let resource_tenant = context
            .resource()
            .tenant_id()
            .ok_or(ScopeAuthorizationError::WrongScope)?;
        if principal.tenant_id.is_empty() || principal.tenant_id != resource_tenant {
            return Err(ScopeAuthorizationError::WrongTenant);
        }
        if !principal
            .roles
            .iter()
            .any(|role| role == capability.required_role())
        {
            return Err(ScopeAuthorizationError::MissingCapability(capability));
        }
        Ok(Self {
            tenant_id: principal.tenant_id.clone(),
            capability,
        })
    }

    #[must_use]
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    #[must_use]
    pub fn capability(&self) -> PrivilegedCapability {
        self.capability
    }
}

/// Proof of authority over an estate-scoped privileged operation.
/// Tenant-bound principals are categorically unable to construct this type,
/// even if a malformed token were to carry an estate-looking role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EstateScope {
    capability: PrivilegedCapability,
}

impl EstateScope {
    /// Authorize an estate capability. The final resource must be explicitly
    /// global, the principal must be unbound from a tenant, and the exact
    /// server-authenticated role must be present.
    pub fn authorize(
        context: &VerifiedRequestContext,
        capability: PrivilegedCapability,
    ) -> Result<Self, ScopeAuthorizationError> {
        if !capability.is_estate() {
            return Err(ScopeAuthorizationError::WrongScope);
        }
        let principal = context.principal();
        if context.resource().tenant_id().is_some() || !principal.tenant_id.is_empty() {
            return Err(ScopeAuthorizationError::WrongScope);
        }
        let authorized = principal
            .roles
            .iter()
            .any(|role| role == capability.required_role() || role == "estate-system");
        if !authorized {
            return Err(ScopeAuthorizationError::MissingCapability(capability));
        }
        Ok(Self { capability })
    }

    #[must_use]
    pub fn capability(&self) -> PrivilegedCapability {
        self.capability
    }
}

/// Failure to derive a tenant/estate proof from authenticated request facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeAuthorizationError {
    WrongScope,
    WrongTenant,
    MissingCapability(PrivilegedCapability),
}

impl std::fmt::Display for ScopeAuthorizationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongScope => {
                f.write_str("principal or resource has the wrong authorization scope")
            }
            Self::WrongTenant => f.write_str("principal and resource tenant do not match"),
            Self::MissingCapability(capability) => write!(
                f,
                "missing privileged capability role {}",
                capability.required_role()
            ),
        }
    }
}

impl std::error::Error for ScopeAuthorizationError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> AuthenticatedFacts {
        AuthenticatedFacts {
            principal_id: "spiffe://mai/wsf/issuer".into(),
            kind: IdentityKind::Workload,
            tenant_id: "tenant-a".into(),
            subject_hash: "blake3:deadbeef".into(),
            service_identity: Some("issuer".into()),
            roles: vec!["issuer".into()],
            token_lineage: Some("root-token".into()),
            auth_strength: AuthStrength::MutualTls,
            audience: Audience::Wsf,
        }
    }

    #[test]
    fn establish_is_the_only_constructor_and_stamps_derived_fields() {
        let p = WsfPrincipal::establish(facts(), "corr-1", "2026-07-07T00:00:00Z");
        assert_eq!(p.tenant_id, "tenant-a");
        assert_eq!(p.correlation_id, "corr-1");
        assert_eq!(p.authenticated_at, "2026-07-07T00:00:00Z");
        assert!(p.is_for(Audience::Wsf));
        assert!(!p.is_for(Audience::Aog));
    }

    #[test]
    fn principal_serializes_for_receipts_without_leaking_cleartext_subject() {
        let p = WsfPrincipal::establish(facts(), "corr-2", "2026-07-07T00:00:00Z");
        let json = serde_json::to_value(&p).unwrap();
        // Serializable (receipts/audit need it) …
        assert_eq!(json["tenant_id"], "tenant-a");
        assert_eq!(json["auth_strength"], "mutual_tls");
        assert_eq!(json["audience"], "wsf");
        // … but only the hash, never a cleartext subject id, and no witness leak
        // beyond an empty struct.
        assert_eq!(json["subject_hash"], "blake3:deadbeef");
        assert!(json.get("subject_id").is_none());
    }

    #[test]
    fn auth_strength_orders_and_gates_production() {
        assert!(AuthStrength::MutualTls > AuthStrength::LocalDev);
        assert!(AuthStrength::WorkloadToken.is_production_grade());
        assert!(AuthStrength::MutualTls.is_production_grade());
        assert!(!AuthStrength::LocalDev.is_production_grade());
    }

    #[test]
    fn request_context_rejects_wrong_audience_tenant_and_ambiguous_resource() {
        let principal = WsfPrincipal::establish(facts(), "corr-3", "2026-07-07T00:00:00Z");
        let own = CanonicalResource::resolved("token", "tok-1", Some("tenant-a".into())).unwrap();
        assert!(
            VerifiedRequestContext::establish(
                principal.clone(),
                RequestOperation::WsfAttenuate,
                own
            )
            .is_ok()
        );
        let attenuate = VerifiedRequestContext::establish(
            principal.clone(),
            RequestOperation::WsfAttenuate,
            CanonicalResource::resolved("token", "tok-1", Some("tenant-a".into())).unwrap(),
        )
        .unwrap();
        assert_eq!(
            attenuate
                .require_operation(RequestOperation::WsfUnseal)
                .unwrap_err(),
            RequestContextError::WrongOperation {
                expected: RequestOperation::WsfUnseal,
                got: RequestOperation::WsfAttenuate,
            }
        );
        let foreign =
            CanonicalResource::resolved("token", "tok-1", Some("tenant-b".into())).unwrap();
        assert_eq!(
            VerifiedRequestContext::establish(
                principal.clone(),
                RequestOperation::WsfAttenuate,
                foreign
            )
            .unwrap_err(),
            RequestContextError::WrongTenant
        );
        let aog = CanonicalResource::resolved("Tenant", "tenant-a", None).unwrap();
        assert!(matches!(
            VerifiedRequestContext::establish(principal, RequestOperation::AogRead, aog),
            Err(RequestContextError::WrongAudience { .. })
        ));
        assert_eq!(
            CanonicalResource::resolved("", "tok-1", None).unwrap_err(),
            RequestContextError::AmbiguousResource
        );
    }

    #[test]
    fn tenant_and_estate_capability_matrix_is_fail_closed() {
        let mut tenant_facts = facts();
        tenant_facts.audience = Audience::Aog;
        tenant_facts.roles = vec![
            PrivilegedCapability::TenantRevocation
                .required_role()
                .into(),
            PrivilegedCapability::EstateRevocation
                .required_role()
                .into(),
        ];
        let tenant_principal =
            WsfPrincipal::establish(tenant_facts, "corr-scope-tenant", "2026-07-15T00:00:00Z");
        let tenant_context = VerifiedRequestContext::establish(
            tenant_principal.clone(),
            RequestOperation::AogCreate,
            CanonicalResource::resolved("RevocationIntent", "tenant-kill", Some("tenant-a".into()))
                .unwrap(),
        )
        .unwrap();
        assert!(
            TenantScope::authorize(&tenant_context, PrivilegedCapability::TenantRevocation).is_ok()
        );
        assert_eq!(
            EstateScope::authorize(&tenant_context, PrivilegedCapability::EstateRevocation)
                .unwrap_err(),
            ScopeAuthorizationError::WrongScope,
            "a tenant-bound principal cannot become estate-scoped even with an estate-looking role"
        );

        let global_context = VerifiedRequestContext::establish(
            tenant_principal,
            RequestOperation::AogDelete,
            CanonicalResource::resolved("PolicyBundle", "global", None).unwrap(),
        )
        .unwrap();
        for capability in [
            PrivilegedCapability::EstateRevocation,
            PrivilegedCapability::GlobalObjectMutation,
            PrivilegedCapability::RingKeyDestruction,
            PrivilegedCapability::PolicyPublication,
        ] {
            assert_eq!(
                EstateScope::authorize(&global_context, capability).unwrap_err(),
                ScopeAuthorizationError::WrongScope,
                "tenant principal authorized {capability:?}"
            );
        }

        let mut estate_facts = facts();
        estate_facts.audience = Audience::Aog;
        estate_facts.tenant_id.clear();
        estate_facts.roles = vec!["estate-system".into()];
        let estate_principal =
            WsfPrincipal::establish(estate_facts, "corr-scope-estate", "2026-07-15T00:00:00Z");
        let estate_context = VerifiedRequestContext::establish(
            estate_principal,
            RequestOperation::AogDelete,
            CanonicalResource::resolved("PolicyBundle", "global", None).unwrap(),
        )
        .unwrap();
        for capability in [
            PrivilegedCapability::EstateRevocation,
            PrivilegedCapability::GlobalObjectMutation,
            PrivilegedCapability::RingKeyDestruction,
            PrivilegedCapability::PolicyPublication,
        ] {
            assert!(EstateScope::authorize(&estate_context, capability).is_ok());
        }
    }
}
