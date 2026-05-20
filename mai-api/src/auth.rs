//! Authentication and authorization middleware for the MAI API.
//!
//! # Trust Model
//!
//! Every request carries a profile identity via the `X-IM-Profile` header.
//! The profile determines role-based permissions for model access, content
//! filtering, and administrative operations.
//!
//! # Backend Opacity
//!
//! Profile middleware never exposes adapter or backend names. All authorization
//! decisions reference model capabilities, not implementation details.

use axum::{extract::Request, http::HeaderMap, middleware::Next, response::Response};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::errors::ApiError;
use crate::types::{ModelAccessFilter, ProfileInfo, ProfileRole};

// ── Header Constants ──────────────────────────────────────────────────

/// Header name for profile identification.
pub const PROFILE_HEADER: &str = "X-IM-Profile";

/// Header name for optional TPM-backed auth token.
pub const AUTH_TOKEN_HEADER: &str = "X-IM-Auth-Token";

// ── Token Validation Trait ────────────────────────────────────────────

/// Trait for validating authentication tokens.
///
/// Implementations may use TPM 2.0 attestation, local key verification,
/// or other air-gap-compatible mechanisms. No network calls permitted.
#[async_trait::async_trait]
pub trait TokenValidator: Send + Sync + 'static {
    /// Validate a token and return the associated profile ID.
    /// Returns None if the token is invalid or expired.
    async fn validate(&self, token: &str) -> Option<String>;

    /// Check if a token has a specific capability.
    async fn has_capability(&self, token: &str, capability: &str) -> bool;
}

/// Default validator that accepts profile headers without token verification.
///
/// Used during development and for local-only deployments where all
/// access originates from the trusted local network segment.
#[derive(Debug, Clone)]
pub struct LocalTrustValidator;

#[async_trait::async_trait]
impl TokenValidator for LocalTrustValidator {
    async fn validate(&self, _token: &str) -> Option<String> {
        // Local trust: all tokens are valid. Profile comes from header.
        Some("local-trust".to_string())
    }

    async fn has_capability(&self, _token: &str, _capability: &str) -> bool {
        true
    }
}

// ── Profile Extraction ────────────────────────────────────────────────

/// Extract profile information from request headers.
///
/// The X-IM-Profile header format: `profile_id:role`
/// Example: `family-dad:admin`, `kid-timmy:child`
///
/// If the header is missing, returns a Guest profile.
/// If the header is malformed, returns an error.
pub fn extract_profile(headers: &HeaderMap) -> Result<ProfileInfo, ApiError> {
    let header_value = match headers.get(PROFILE_HEADER) {
        Some(v) => v,
        None => {
            debug!("No profile header present, defaulting to guest");
            return Ok(ProfileInfo {
                profile_id: "guest".to_string(),
                role: ProfileRole::Guest,
                display_name: Some("Guest".to_string()),
                permissions: ProfileRole::Guest.permissions(),
            });
        }
    };

    let header_str = header_value.to_str().map_err(|_| {
        ApiError::BadRequest("X-IM-Profile header contains non-ASCII characters".to_string())
    })?;

    parse_profile_header(header_str)
}

/// Parse a profile header value into ProfileInfo.
///
/// Format: `profile_id:role` or `profile_id:role:display_name`
fn parse_profile_header(value: &str) -> Result<ProfileInfo, ApiError> {
    let parts: Vec<&str> = value.splitn(3, ':').collect();

    if parts.len() < 2 {
        return Err(ApiError::BadRequest(format!(
            "Invalid X-IM-Profile header: expected 'profile_id:role', got '{}'",
            value
        )));
    }

    let profile_id = parts[0].trim();
    if profile_id.is_empty() {
        return Err(ApiError::BadRequest(
            "Profile ID cannot be empty in X-IM-Profile header".to_string(),
        ));
    }

    // Validate profile_id: alphanumeric, hyphens, underscores only
    if !profile_id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ApiError::BadRequest(
            "Profile ID contains invalid characters (alphanumeric, hyphens, underscores only)"
                .to_string(),
        ));
    }

    let role = parse_role(parts[1].trim())?;
    let display_name = parts.get(2).map(|s| s.trim().to_string());
    let permissions = role.permissions();

    Ok(ProfileInfo {
        profile_id: profile_id.to_string(),
        role,
        display_name,
        permissions,
    })
}

/// Parse a role string into ProfileRole.
fn parse_role(s: &str) -> Result<ProfileRole, ApiError> {
    match s.to_lowercase().as_str() {
        "admin" => Ok(ProfileRole::Admin),
        "adult" => Ok(ProfileRole::Adult),
        "teen" => Ok(ProfileRole::Teen),
        "child" => Ok(ProfileRole::Child),
        "guest" => Ok(ProfileRole::Guest),
        other => Err(ApiError::BadRequest(format!(
            "Unknown role '{}' in X-IM-Profile header. Valid: admin, adult, teen, child, guest",
            other
        ))),
    }
}

// ── Axum Extractor ───────────────────────────────────────────────────

/// Allows handlers to extract ProfileInfo directly from the request.
/// Requires that profile_middleware has run and inserted ProfileInfo
/// into request extensions.
impl<S> axum::extract::FromRequestParts<S> for ProfileInfo
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<ProfileInfo>()
            .cloned()
            .ok_or(ApiError::Unauthorized)
    }
}
// ── Middleware ─────────────────────────────────────────────────────────

/// Authentication state shared across middleware layers.
#[derive(Clone)]
pub struct AuthState {
    pub validator: Arc<dyn TokenValidator>,
}

impl AuthState {
    pub fn new(validator: Arc<dyn TokenValidator>) -> Self {
        Self { validator }
    }

    pub fn local_trust() -> Self {
        Self {
            validator: Arc::new(LocalTrustValidator),
        }
    }
}

/// Middleware that extracts the profile from request headers and injects
/// it as a request extension.
///
/// Downstream handlers access the profile via:
/// ```ignore
/// let profile = request.extensions().get::<ProfileInfo>().unwrap();
/// ```
pub async fn profile_middleware(
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let profile = extract_profile(&headers)?;

    debug!(
        profile_id = %profile.profile_id,
        role = ?profile.role,
        "Profile extracted from request"
    );

    // Inject profile into request extensions for downstream handlers
    request.extensions_mut().insert(profile);

    Ok(next.run(request).await)
}

/// Check if a profile has a specific permission.
///
/// This is called by route handlers that need to enforce authorization.
/// The profile must have been injected by profile_middleware first.
pub fn check_permission(profile: &ProfileInfo, permission: &str) -> Result<(), ApiError> {
    let allowed = match permission {
        "inference" => profile.permissions.can_inference,
        "list_models" => profile.permissions.can_list_models,
        "manage_models" => profile.permissions.can_manage_models,
        "power_control" => profile.permissions.can_power_control,
        "registry_write" => profile.permissions.can_registry_write,
        "view_audit" => profile.permissions.can_view_audit,
        "manage_profiles" => profile.permissions.can_manage_profiles,
        _ => {
            warn!(permission = %permission, "Unknown permission check requested");
            false
        }
    };

    if !allowed {
        return Err(ApiError::PermissionDenied(format!(
            "Profile '{}' (role: {:?}) lacks '{}' permission",
            profile.profile_id, profile.role, permission
        )));
    }

    Ok(())
}

// ── Model Access Filtering ────────────────────────────────────────────

/// Check if a profile is allowed to access a specific model.
///
/// Uses the ModelAccessFilter from the profile's permissions.
/// - None filter: all models accessible
/// - TeenSafe: only models tagged as teen-appropriate
/// - ChildSafe: only models tagged as child-safe
/// - DefaultOnly: only the system default model
///
/// The `is_teen_safe` and `is_child_safe` and `is_default` parameters
/// come from the model's metadata in the registry. This function does
/// not perform registry lookups itself.
pub fn can_access_model(
    profile: &ProfileInfo,
    model_name: &str,
    is_teen_safe: bool,
    is_child_safe: bool,
    is_default: bool,
) -> bool {
    match &profile.permissions.model_filter {
        None => true, // No filter = all models
        Some(ModelAccessFilter::TeenSafe) => is_teen_safe,
        Some(ModelAccessFilter::ChildSafe) => is_child_safe,
        Some(ModelAccessFilter::DefaultOnly) => is_default,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_admin_profile() {
        let result = parse_profile_header("family-dad:admin").unwrap();
        assert_eq!(result.profile_id, "family-dad");
        assert!(matches!(result.role, ProfileRole::Admin));
        assert!(result.permissions.can_power_control);
        assert!(result.permissions.can_view_audit);
        assert!(result.display_name.is_none());
    }

    #[test]
    fn test_parse_profile_with_display_name() {
        let result = parse_profile_header("kid-timmy:child:Timmy").unwrap();
        assert_eq!(result.profile_id, "kid-timmy");
        assert!(matches!(result.role, ProfileRole::Child));
        assert_eq!(result.display_name.as_deref(), Some("Timmy"));
        assert!(matches!(
            result.permissions.content_filter,
            ContentFilterLevel::Strict
        ));
    }

    #[test]
    fn test_parse_guest_default() {
        let headers = HeaderMap::new();
        let result = extract_profile(&headers).unwrap();
        assert_eq!(result.profile_id, "guest");
        assert!(matches!(result.role, ProfileRole::Guest));
        // Guest can_inference is true per types.rs definition
        assert!(result.permissions.can_inference);
    }

    #[test]
    fn test_parse_malformed_header() {
        let result = parse_profile_header("nocolon");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_profile_id() {
        let result = parse_profile_header(":admin");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_invalid_role() {
        let result = parse_profile_header("user1:superadmin");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_invalid_characters() {
        let result = parse_profile_header("user@evil:admin");
        assert!(result.is_err());
    }

    #[test]
    fn test_teen_permissions() {
        let perms = ProfileRole::Teen.permissions();
        assert!(perms.can_inference);
        assert!(!perms.can_manage_models);
        assert!(matches!(perms.content_filter, ContentFilterLevel::Moderate));
        assert!(matches!(
            perms.model_filter,
            Some(ModelAccessFilter::TeenSafe)
        ));
    }

    #[test]
    fn test_child_permissions() {
        let perms = ProfileRole::Child.permissions();
        assert!(perms.can_inference);
        assert!(!perms.can_list_models);
        assert!(!perms.can_manage_models);
        assert!(matches!(perms.content_filter, ContentFilterLevel::Strict));
        assert!(matches!(
            perms.model_filter,
            Some(ModelAccessFilter::ChildSafe)
        ));
    }

    #[test]
    fn test_model_access_admin() {
        let profile = ProfileInfo {
            profile_id: "admin1".to_string(),
            role: ProfileRole::Admin,
            display_name: None,
            permissions: ProfileRole::Admin.permissions(),
        };
        assert!(can_access_model(
            &profile,
            "llama-3.1-70b",
            false,
            false,
            false
        ));
        assert!(can_access_model(&profile, "anything", true, true, true));
    }

    #[test]
    fn test_model_access_child() {
        let profile = ProfileInfo {
            profile_id: "kid1".to_string(),
            role: ProfileRole::Child,
            display_name: None,
            permissions: ProfileRole::Child.permissions(),
        };
        // Child can only access child-safe models
        assert!(can_access_model(&profile, "phi-4-mini", false, true, false));
        assert!(!can_access_model(
            &profile,
            "llama-3.1-70b",
            false,
            false,
            false
        ));
    }

    #[test]
    fn test_model_access_teen() {
        let profile = ProfileInfo {
            profile_id: "teen1".to_string(),
            role: ProfileRole::Teen,
            display_name: None,
            permissions: ProfileRole::Teen.permissions(),
        };
        assert!(can_access_model(&profile, "phi-4", true, false, false));
        assert!(!can_access_model(
            &profile,
            "uncensored-model",
            false,
            false,
            false
        ));
    }

    #[test]
    fn test_model_access_guest() {
        let profile = ProfileInfo {
            profile_id: "guest".to_string(),
            role: ProfileRole::Guest,
            display_name: None,
            permissions: ProfileRole::Guest.permissions(),
        };
        // Guest can only access default model
        assert!(can_access_model(
            &profile,
            "default-model",
            false,
            false,
            true
        ));
        assert!(!can_access_model(
            &profile,
            "other-model",
            false,
            false,
            false
        ));
    }

    #[test]
    fn test_check_permission_admin() {
        let profile = ProfileInfo {
            profile_id: "admin1".to_string(),
            role: ProfileRole::Admin,
            display_name: None,
            permissions: ProfileRole::Admin.permissions(),
        };
        assert!(check_permission(&profile, "inference").is_ok());
        assert!(check_permission(&profile, "manage_models").is_ok());
        assert!(check_permission(&profile, "power_control").is_ok());
        assert!(check_permission(&profile, "view_audit").is_ok());
        assert!(check_permission(&profile, "manage_profiles").is_ok());
    }

    #[test]
    fn test_check_permission_adult_denied() {
        let profile = ProfileInfo {
            profile_id: "adult1".to_string(),
            role: ProfileRole::Adult,
            display_name: None,
            permissions: ProfileRole::Adult.permissions(),
        };
        assert!(check_permission(&profile, "inference").is_ok());
        assert!(check_permission(&profile, "manage_models").is_err());
        assert!(check_permission(&profile, "power_control").is_err());
        assert!(check_permission(&profile, "view_audit").is_err());
    }

    #[test]
    fn test_check_permission_unknown() {
        let profile = ProfileInfo {
            profile_id: "admin1".to_string(),
            role: ProfileRole::Admin,
            display_name: None,
            permissions: ProfileRole::Admin.permissions(),
        };
        // Unknown permission is denied
        assert!(check_permission(&profile, "nonexistent").is_err());
    }
}
