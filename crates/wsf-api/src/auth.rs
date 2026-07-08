//! WSF principal authentication + server-derived issuance authority (AF-01).
//!
//! Every privileged WSF route runs behind an [`Authenticator`]. The default is
//! [`DenyAllAuthenticator`] — absent a configured production authenticator the
//! trust plane refuses to mint authority (fail closed). A resolved
//! [`WsfPrincipal`] carries the **server-side** tenant, roles, audience, budget
//! ceiling, and model allowlist; a request body may only *narrow* these via
//! [`derive_issue_authority`], never widen them.

use std::collections::HashMap;
use std::sync::Mutex;

use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use fabric_contracts::Budget;

/// What kinds of issuance a principal is permitted to perform.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IssuancePermissions {
    /// Mint tokens scoped to the principal's own subject.
    pub self_scoped: bool,
    /// Mint tokens for other subjects within the principal's tenant.
    pub delegated: bool,
    /// Mint service/workload tokens.
    pub service: bool,
    /// Administrative issuance.
    pub admin: bool,
}

/// The kind of token being requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssuanceKind {
    /// Scoped to the principal's own subject.
    SelfScoped,
    /// For another subject within the tenant.
    Delegated,
    /// A service/workload token.
    Service,
}

impl IssuanceKind {
    /// Parse an issuance-kind name.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "self" | "self_scoped" => Some(IssuanceKind::SelfScoped),
            "delegated" => Some(IssuanceKind::Delegated),
            "service" => Some(IssuanceKind::Service),
            _ => None,
        }
    }
}

/// An authenticated caller. Every authoritative field is server-derived; the
/// request body may only narrow it.
#[derive(Debug, Clone)]
pub struct WsfPrincipal {
    /// The workload / service identity (from the authenticator).
    pub service_identity: String,
    /// The tenant this principal is bound to.
    pub tenant_id: String,
    /// Roles the principal holds (the ceiling for issued roles).
    pub roles: Vec<String>,
    /// Audience the principal issues for.
    pub audience: String,
    /// Budget ceiling (`None` = unbounded, e.g. an administrative identity).
    pub budget_ceiling: Option<Budget>,
    /// Model allowlist (`empty` = no identity-imposed model constraint).
    pub allowed_models: Vec<String>,
    /// Which issuance kinds this principal may perform.
    pub permissions: IssuancePermissions,
}

/// Authentication / authorization failure. Everything fails closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// No credential was presented.
    MissingCredential,
    /// A credential was presented but is invalid.
    InvalidCredential(String),
    /// The principal is authenticated but not authorized for the action.
    Forbidden(String),
}

impl AuthError {
    /// The HTTP status for this failure.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            AuthError::MissingCredential | AuthError::InvalidCredential(_) => {
                StatusCode::UNAUTHORIZED
            }
            AuthError::Forbidden(_) => StatusCode::FORBIDDEN,
        }
    }

    /// A safe message for this failure (never echoes credential material).
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            AuthError::MissingCredential => "authentication required".to_string(),
            AuthError::InvalidCredential(m) => format!("invalid credential: {m}"),
            AuthError::Forbidden(m) => format!("forbidden: {m}"),
        }
    }
}

/// The authentication seam. A production deployment wires an mTLS / workload-
/// identity authenticator; the default [`DenyAllAuthenticator`] fails closed.
pub trait Authenticator: Send + Sync {
    /// Authenticate a request from its headers, or fail closed.
    ///
    /// # Errors
    /// [`AuthError`] when no valid principal can be established.
    fn authenticate(&self, headers: &HeaderMap) -> Result<WsfPrincipal, AuthError>;
}

/// Fail-closed default: rejects every request. Wired when no production
/// authenticator is configured, so an unconfigured trust plane mints nothing.
pub struct DenyAllAuthenticator;

impl Authenticator for DenyAllAuthenticator {
    fn authenticate(&self, _headers: &HeaderMap) -> Result<WsfPrincipal, AuthError> {
        Err(AuthError::InvalidCredential(
            "no authenticator configured (fail-closed default)".to_string(),
        ))
    }
}

/// A development / test authenticator that maps an explicit bearer credential to
/// a pre-registered principal. **Not for production** — production wires an
/// mTLS / workload-identity authenticator. Unknown or missing credentials fail
/// closed.
#[derive(Default)]
pub struct StaticAuthenticator {
    principals: HashMap<String, WsfPrincipal>,
}

impl StaticAuthenticator {
    /// A new, empty authenticator (registers no principals — rejects everything).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a principal behind a bearer credential.
    #[must_use]
    pub fn with_principal(
        mut self,
        credential: impl Into<String>,
        principal: WsfPrincipal,
    ) -> Self {
        self.principals.insert(credential.into(), principal);
        self
    }
}

impl Authenticator for StaticAuthenticator {
    fn authenticate(&self, headers: &HeaderMap) -> Result<WsfPrincipal, AuthError> {
        let cred = bearer(headers)?;
        self.principals
            .get(cred)
            .cloned()
            .ok_or_else(|| AuthError::InvalidCredential("unknown credential".to_string()))
    }
}

fn bearer(headers: &HeaderMap) -> Result<&str, AuthError> {
    let raw = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(AuthError::MissingCredential)?;
    raw.strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .ok_or(AuthError::MissingCredential)
}

/// The requested (narrowing) authority parsed from a caller's issue body.
pub struct IssuanceRequest<'a> {
    /// The requested issuance kind.
    pub kind: IssuanceKind,
    /// The requested tenant (must equal the principal's tenant, or be absent).
    pub requested_tenant: Option<&'a str>,
    /// The subject the token is minted for.
    pub subject_id: &'a str,
    /// Requested roles (must be a subset of the principal's).
    pub requested_roles: &'a [String],
    /// Requested budget (must be within the principal's ceiling).
    pub requested_budget: Option<&'a Budget>,
    /// Requested models (must be a subset of the principal's allowlist).
    pub requested_models: &'a [String],
}

/// The server-derived authority that will actually be signed.
#[derive(Debug, Clone)]
pub struct DerivedAuthority {
    /// Server-derived tenant.
    pub tenant_id: String,
    /// Subject the token is for.
    pub subject_id: String,
    /// Effective roles (⊆ principal roles).
    pub roles: Vec<String>,
    /// Effective audience (from the principal).
    pub audience: String,
    /// Effective budget (≤ ceiling).
    pub budget: Option<Budget>,
    /// Effective model allowlist (⊆ principal allowlist).
    pub allowed_models: Vec<String>,
}

/// Derive the authority to sign, enforcing that the request only *narrows* the
/// principal's authority.
///
/// # Errors
/// [`AuthError::Forbidden`] when the request widens tenant, roles, budget, or
/// models, or when the principal lacks permission for the issuance kind.
pub fn derive_issue_authority(
    principal: &WsfPrincipal,
    req: &IssuanceRequest<'_>,
) -> Result<DerivedAuthority, AuthError> {
    let permitted = match req.kind {
        IssuanceKind::SelfScoped => principal.permissions.self_scoped,
        IssuanceKind::Delegated => principal.permissions.delegated,
        IssuanceKind::Service => principal.permissions.service,
    };
    if !permitted {
        return Err(AuthError::Forbidden(format!(
            "principal '{}' may not perform {:?} issuance",
            principal.service_identity, req.kind
        )));
    }

    if let Some(t) = req.requested_tenant
        && t != principal.tenant_id
    {
        return Err(AuthError::Forbidden(format!(
            "cross-tenant issuance denied (principal tenant '{}', requested '{t}')",
            principal.tenant_id
        )));
    }

    for r in req.requested_roles {
        if !principal.roles.iter().any(|pr| pr == r) {
            return Err(AuthError::Forbidden(format!(
                "role '{r}' exceeds the principal's granted roles"
            )));
        }
    }
    let roles = if req.requested_roles.is_empty() {
        principal.roles.clone()
    } else {
        req.requested_roles.to_vec()
    };

    let budget = derive_budget(principal.budget_ceiling.as_ref(), req.requested_budget)?;
    let allowed_models = derive_models(&principal.allowed_models, req.requested_models)?;

    Ok(DerivedAuthority {
        tenant_id: principal.tenant_id.clone(),
        subject_id: req.subject_id.to_string(),
        roles,
        audience: principal.audience.clone(),
        budget,
        allowed_models,
    })
}

fn derive_budget(
    ceiling: Option<&Budget>,
    requested: Option<&Budget>,
) -> Result<Option<Budget>, AuthError> {
    match (ceiling, requested) {
        (None, Some(b)) => Ok(Some(b.clone())),
        (Some(c), Some(b)) => {
            if b.token_cap > c.token_cap
                || b.usd_cap_cents > c.usd_cap_cents
                || b.tool_call_cap > c.tool_call_cap
            {
                return Err(AuthError::Forbidden(
                    "requested budget exceeds the principal's ceiling".to_string(),
                ));
            }
            Ok(Some(b.clone()))
        }
        (Some(c), None) => Ok(Some(c.clone())),
        (None, None) => Ok(None),
    }
}

fn derive_models(allowed: &[String], requested: &[String]) -> Result<Vec<String>, AuthError> {
    if allowed.is_empty() {
        return Ok(requested.to_vec());
    }
    if requested.is_empty() {
        return Ok(allowed.to_vec());
    }
    for m in requested {
        if !allowed.iter().any(|am| am == m) {
            return Err(AuthError::Forbidden(format!(
                "model '{m}' is outside the principal's allowed set"
            )));
        }
    }
    Ok(requested.to_vec())
}

/// A minimal per-principal fixed-window issuance rate limiter.
pub struct RateLimiter {
    max_per_window: u32,
    window_secs: i64,
    state: Mutex<HashMap<String, (i64, u32)>>,
}

impl RateLimiter {
    /// A limiter allowing `max_per_window` issuances per `window_secs`.
    #[must_use]
    pub fn new(max_per_window: u32, window_secs: i64) -> Self {
        Self {
            max_per_window,
            window_secs: window_secs.max(1),
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Record a hit for `key` at `now_epoch` (seconds); `true` when within the limit.
    pub fn check(&self, key: &str, now_epoch: i64) -> bool {
        let mut state = self.state.lock().expect("rate limiter lock");
        let window = now_epoch / self.window_secs;
        let entry = state.entry(key.to_string()).or_insert((window, 0));
        if entry.0 != window {
            *entry = (window, 0);
        }
        entry.1 += 1;
        entry.1 <= self.max_per_window
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(120, 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal() -> WsfPrincipal {
        WsfPrincipal {
            service_identity: "svc-a".to_string(),
            tenant_id: "tenant-a".to_string(),
            roles: vec!["clinician".to_string(), "reader".to_string()],
            audience: "aud-a".to_string(),
            budget_ceiling: Some(Budget {
                token_cap: 1000,
                tokens_spent: 0,
                usd_cap_cents: 5000,
                usd_spent_cents: 0,
                tool_call_cap: 100,
                tool_calls_spent: 0,
            }),
            allowed_models: vec!["gpt-4o".to_string(), "local".to_string()],
            permissions: IssuancePermissions {
                self_scoped: true,
                delegated: true,
                service: false,
                admin: false,
            },
        }
    }

    fn req<'a>(
        kind: IssuanceKind,
        tenant: Option<&'a str>,
        roles: &'a [String],
        budget: Option<&'a Budget>,
        models: &'a [String],
    ) -> IssuanceRequest<'a> {
        IssuanceRequest {
            kind,
            requested_tenant: tenant,
            subject_id: "subject-1",
            requested_roles: roles,
            requested_budget: budget,
            requested_models: models,
        }
    }

    #[test]
    fn deny_all_fails_closed() {
        let a = DenyAllAuthenticator;
        assert!(a.authenticate(&HeaderMap::new()).is_err());
    }

    #[test]
    fn static_authenticator_requires_known_bearer() {
        let a = StaticAuthenticator::new().with_principal("cred-1", principal());
        assert!(a.authenticate(&HeaderMap::new()).is_err(), "missing header");
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, "Bearer nope".parse().unwrap());
        assert!(a.authenticate(&h).is_err(), "unknown credential");
        let mut ok = HeaderMap::new();
        ok.insert(AUTHORIZATION, "Bearer cred-1".parse().unwrap());
        assert_eq!(a.authenticate(&ok).unwrap().tenant_id, "tenant-a");
    }

    #[test]
    fn narrowing_request_succeeds_and_is_server_bound() {
        let roles = vec!["reader".to_string()];
        let models = vec!["local".to_string()];
        let d = derive_issue_authority(
            &principal(),
            &req(
                IssuanceKind::Delegated,
                Some("tenant-a"),
                &roles,
                None,
                &models,
            ),
        )
        .unwrap();
        assert_eq!(d.tenant_id, "tenant-a");
        assert_eq!(d.roles, vec!["reader".to_string()]);
        assert_eq!(d.allowed_models, vec!["local".to_string()]);
        assert_eq!(d.audience, "aud-a");
    }

    #[test]
    fn empty_request_inherits_principal_ceiling() {
        let d = derive_issue_authority(
            &principal(),
            &req(IssuanceKind::Delegated, None, &[], None, &[]),
        )
        .unwrap();
        assert_eq!(d.roles, vec!["clinician".to_string(), "reader".to_string()]);
        assert_eq!(
            d.allowed_models,
            vec!["gpt-4o".to_string(), "local".to_string()]
        );
        assert_eq!(d.budget.unwrap().token_cap, 1000);
    }

    #[test]
    fn cross_tenant_is_denied() {
        let err = derive_issue_authority(
            &principal(),
            &req(IssuanceKind::Delegated, Some("tenant-b"), &[], None, &[]),
        )
        .unwrap_err();
        assert!(matches!(err, AuthError::Forbidden(_)));
    }

    #[test]
    fn role_elevation_is_denied() {
        let roles = vec!["admin".to_string()];
        let err = derive_issue_authority(
            &principal(),
            &req(IssuanceKind::Delegated, None, &roles, None, &[]),
        )
        .unwrap_err();
        assert!(matches!(err, AuthError::Forbidden(_)));
    }

    #[test]
    fn budget_widening_is_denied() {
        let over = Budget {
            token_cap: 100_000,
            tokens_spent: 0,
            usd_cap_cents: 5000,
            usd_spent_cents: 0,
            tool_call_cap: 100,
            tool_calls_spent: 0,
        };
        let err = derive_issue_authority(
            &principal(),
            &req(IssuanceKind::Delegated, None, &[], Some(&over), &[]),
        )
        .unwrap_err();
        assert!(matches!(err, AuthError::Forbidden(_)));
    }

    #[test]
    fn model_widening_is_denied() {
        let models = vec!["claude-3-5-sonnet".to_string()];
        let err = derive_issue_authority(
            &principal(),
            &req(IssuanceKind::Delegated, None, &[], None, &models),
        )
        .unwrap_err();
        assert!(matches!(err, AuthError::Forbidden(_)));
    }

    #[test]
    fn disallowed_issuance_kind_is_denied() {
        // The fixture principal lacks `service` permission.
        let err = derive_issue_authority(
            &principal(),
            &req(IssuanceKind::Service, None, &[], None, &[]),
        )
        .unwrap_err();
        assert!(matches!(err, AuthError::Forbidden(_)));
    }

    #[test]
    fn rate_limiter_bounds_per_window() {
        let rl = RateLimiter::new(2, 60);
        assert!(rl.check("svc-a", 0));
        assert!(rl.check("svc-a", 10));
        assert!(!rl.check("svc-a", 20), "third hit in window is blocked");
        assert!(rl.check("svc-a", 60), "next window resets");
        assert!(
            rl.check("svc-b", 20),
            "a different principal is independent"
        );
    }
}
