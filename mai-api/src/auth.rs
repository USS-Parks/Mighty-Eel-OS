//! Authentication and authorization middleware for the MAI API.
//!
//! # Trust Model
//!
//! Every non-health request must carry a valid API key via the
//! `X-IM-Auth-Token` header. The key maps to a profile with role-based
//! permissions. Health endpoints are exempt from auth.
//!
//! Internal service-to-service calls may use the `X-IM-Profile` header
//! directly, but ONLY when `allow_internal_profile_header` is enabled
//! in config (disabled by default).
//!
//! # Rate Limiting
//!
//! Per-key request rate limiting with configurable threshold (default
//! 60 requests/minute). Returns 429 Too Many Requests when exceeded.
//!
//! # Backend Opacity
//!
//! Profile middleware never exposes adapter or backend names. All
//! authorization decisions reference model capabilities, not
//! implementation details.

use axum::{
    extract::{Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use sha3::{Digest, Sha3_256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::errors::ApiError;
use crate::state::AppState;
use crate::types::{ModelAccessFilter, ProfileInfo, ProfileRole};

// -- Header Constants --

/// Header name for API key authentication.
pub const AUTH_TOKEN_HEADER: &str = "X-IM-Auth-Token";

/// Header name for internal service-to-service profile identification.
/// Only honored when `allow_internal_profile_header` is enabled.
pub const PROFILE_HEADER: &str = "X-IM-Profile";

/// Paths exempt from API key authentication. `/v1/metrics` is exempt
/// because host-local Prometheus scrapers can't carry an
/// API key on every scrape — the redaction guarantee on the metrics
/// registry (see `metrics::sanitize_label_value`) means the body never
/// leaks secrets even when read by an unauthenticated client.
const AUTH_EXEMPT_PREFIXES: &[&str] = &["/v1/health", "/v1/metrics"];

// -- API Key Store --

/// An authenticated API key entry mapping a key hash to a profile.
#[derive(Debug, Clone)]
pub struct ApiKeyEntry {
    /// SHA-256 hash of the raw API key (hex-encoded).
    pub key_hash: String,
    /// Profile ID this key authenticates as.
    pub profile_id: String,
    /// Role for this profile.
    pub role: ProfileRole,
    /// Optional display name.
    pub display_name: Option<String>,
}

/// API key store: maps hashed keys to profile entries.
#[derive(Debug, Clone)]
pub struct ApiKeyStore {
    /// Keys indexed by their SHA-256 hash.
    keys: HashMap<String, ApiKeyEntry>,
    /// Allow X-IM-Profile header for internal service-to-service calls.
    /// Disabled by default. When enabled, requests with X-IM-Profile but
    /// no X-IM-Auth-Token are treated as internal calls.
    pub allow_internal_profile_header: bool,
}

impl ApiKeyStore {
    /// Create an empty key store.
    pub fn new() -> Self {
        Self {
            keys: HashMap::new(),
            allow_internal_profile_header: false,
        }
    }

    /// Add a key by its raw plaintext value. The store hashes it internally.
    pub fn add_key_raw(
        &mut self,
        raw_key: &str,
        profile_id: String,
        role: ProfileRole,
        display_name: Option<String>,
    ) {
        let hash = hash_api_key(raw_key);
        self.keys.insert(
            hash.clone(),
            ApiKeyEntry {
                key_hash: hash,
                profile_id,
                role,
                display_name,
            },
        );
    }

    /// Add a key by its pre-computed hash (for loading from config).
    pub fn add_key_hashed(
        &mut self,
        key_hash: String,
        profile_id: String,
        role: ProfileRole,
        display_name: Option<String>,
    ) {
        self.keys.insert(
            key_hash.clone(),
            ApiKeyEntry {
                key_hash,
                profile_id,
                role,
                display_name,
            },
        );
    }

    /// Validate a raw API key. Returns the entry if valid.
    pub fn validate(&self, raw_key: &str) -> Option<&ApiKeyEntry> {
        let hash = hash_api_key(raw_key);
        self.keys.get(&hash)
    }

    /// Number of registered keys.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

impl Default for ApiKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Hash an API key with SHA-256, return hex-encoded string.
pub fn hash_api_key(raw_key: &str) -> String {
    let mut hasher = Sha3_256::new();
    hasher.update(raw_key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Generate a cryptographically random API key (32 bytes, hex-encoded = 64 chars).
///
/// Uses [`rand::rngs::OsRng`], the platform CSPRNG (`BCryptGenRandom` on
/// Windows, `getrandom(2)` on Linux). The raw key is returned to the caller
/// exactly once; only the hash is ever persisted.
pub fn generate_api_key() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("im-{}", hex::encode(bytes))
}

// -- Rate Limiter --

/// Per-key sliding window rate limiter.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    /// Maximum requests per window.
    pub max_requests: u32,
    /// Window duration in seconds.
    pub window_seconds: u64,
    /// Per-key request timestamps.
    windows: Arc<RwLock<HashMap<String, Vec<Instant>>>>,
}

impl RateLimiter {
    /// Create a new rate limiter.
    pub fn new(max_requests: u32, window_seconds: u64) -> Self {
        Self {
            max_requests,
            window_seconds,
            windows: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Default: 60 requests per minute.
    pub fn default_per_minute() -> Self {
        Self::new(60, 60)
    }

    /// Check if a key is within its rate limit. Returns Ok(remaining)
    /// or Err(retry_after_seconds).
    pub async fn check_rate_limit(&self, key_hash: &str) -> Result<u32, u64> {
        let now = Instant::now();
        let window_duration = std::time::Duration::from_secs(self.window_seconds);
        let mut windows = self.windows.write().await;

        let timestamps = windows.entry(key_hash.to_string()).or_default();

        // Prune expired timestamps
        timestamps.retain(|t| now.duration_since(*t) < window_duration);

        if timestamps.len() >= self.max_requests as usize {
            // Find the oldest timestamp to calculate retry-after
            let oldest = timestamps.first().copied().unwrap_or(now);
            let elapsed = now.duration_since(oldest);
            let retry_after = self.window_seconds.saturating_sub(elapsed.as_secs());
            return Err(retry_after.max(1));
        }

        timestamps.push(now);
        #[allow(clippy::cast_possible_truncation)]
        let remaining = self.max_requests - timestamps.len() as u32;
        Ok(remaining)
    }
}

// -- Profile Extraction --

/// Extract profile information from request headers.
///
/// The X-IM-Profile header format: `profile_id:role`
/// Example: `family-dad:admin`, `kid-timmy:child`
///
/// If the header is missing, returns a Guest profile.
/// If the header is malformed, returns an error.
pub fn extract_profile(headers: &HeaderMap) -> Result<ProfileInfo, ApiError> {
    let Some(header_value) = headers.get(PROFILE_HEADER) else {
        debug!("No profile header present, defaulting to guest");
        return Ok(ProfileInfo {
            profile_id: "guest".to_string(),
            role: ProfileRole::Guest,
            display_name: Some("Guest".to_string()),
            permissions: ProfileRole::Guest.permissions(),
        });
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
            "Invalid X-IM-Profile header: expected 'profile_id:role', got '{value}'"
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
            "Unknown role '{other}' in X-IM-Profile header. Valid: admin, adult, teen, child, guest"
        ))),
    }
}

// -- Axum Extractor --

/// Allows handlers to extract ProfileInfo directly from the request.
/// Requires that auth_middleware has run and inserted ProfileInfo
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

// -- Authentication State --

/// Authentication state shared across middleware layers.
#[derive(Clone)]
pub struct AuthState {
    pub key_store: Arc<RwLock<ApiKeyStore>>,
    pub rate_limiter: Arc<RateLimiter>,
}

impl AuthState {
    /// Create auth state with an API key store and rate limiter.
    pub fn new(key_store: ApiKeyStore, rate_limiter: RateLimiter) -> Self {
        Self {
            key_store: Arc::new(RwLock::new(key_store)),
            rate_limiter: Arc::new(rate_limiter),
        }
    }

    /// Create auth state for local-trust development mode.
    /// All requests are accepted without API key validation.
    /// A default admin key is still generated and logged.
    pub fn local_trust() -> Self {
        let mut store = ApiKeyStore::new();
        store.allow_internal_profile_header = true;
        Self {
            key_store: Arc::new(RwLock::new(store)),
            rate_limiter: Arc::new(RateLimiter::default_per_minute()),
        }
    }

    /// Create auth state from a loaded key store with default rate limits.
    pub fn with_key_store(store: ApiKeyStore) -> Self {
        Self {
            key_store: Arc::new(RwLock::new(store)),
            rate_limiter: Arc::new(RateLimiter::default_per_minute()),
        }
    }

    /// Create auth state with custom rate limits.
    pub fn with_rate_limit(store: ApiKeyStore, max_requests: u32, window_seconds: u64) -> Self {
        Self {
            key_store: Arc::new(RwLock::new(store)),
            rate_limiter: Arc::new(RateLimiter::new(max_requests, window_seconds)),
        }
    }
}

// -- Middleware --

/// Check if a request path is exempt from authentication.
fn is_auth_exempt(path: &str) -> bool {
    AUTH_EXEMPT_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
}

/// Combined authentication + profile extraction middleware.
///
/// For non-exempt paths:
/// 1. Requires X-IM-Auth-Token header with a valid API key
/// 2. Maps the key to a profile (role, permissions)
/// 3. Checks per-key rate limit
/// 4. Injects ProfileInfo into request extensions
///
/// For exempt paths (health):
/// 1. Injects a Guest profile (no auth required)
///
/// When `allow_internal_profile_header` is enabled and no auth token
/// is present, falls back to X-IM-Profile header parsing (for internal
/// service-to-service calls only).
pub async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let path = request.uri().path().to_string();

    // Health endpoints are auth-exempt
    if is_auth_exempt(&path) {
        let profile = extract_profile(&headers).unwrap_or(ProfileInfo {
            profile_id: "guest".to_string(),
            role: ProfileRole::Guest,
            display_name: Some("Guest".to_string()),
            permissions: ProfileRole::Guest.permissions(),
        });
        request.extensions_mut().insert(profile);
        return Ok(next.run(request).await);
    }

    // Get auth state from AppState
    let auth_state = state.auth.clone();

    // Try API key authentication first
    if let Some(token_value) = headers.get(AUTH_TOKEN_HEADER) {
        let token_str = token_value.to_str().map_err(|_| {
            ApiError::BadRequest("X-IM-Auth-Token header contains non-ASCII characters".to_string())
        })?;

        let store = auth_state.key_store.read().await;
        let entry = store.validate(token_str).ok_or_else(|| {
            warn!("Invalid API key presented");
            ApiError::TokenInvalid
        })?;

        // Rate limit check (keyed by the key's hash)
        let key_hash = entry.key_hash.clone();
        let profile_id = entry.profile_id.clone();
        let role = entry.role;
        let display_name = entry.display_name.clone();
        drop(store); // Release read lock before rate limit check

        match auth_state.rate_limiter.check_rate_limit(&key_hash).await {
            Ok(_remaining) => {}
            Err(retry_after) => {
                warn!(
                    profile_id = %profile_id,
                    retry_after = retry_after,
                    "Rate limit exceeded"
                );
                return Err(ApiError::RateLimited(retry_after));
            }
        }

        let permissions = role.permissions();
        let profile = ProfileInfo {
            profile_id,
            role,
            display_name,
            permissions,
        };

        debug!(
            profile_id = %profile.profile_id,
            role = ?profile.role,
            "Authenticated via API key"
        );

        request.extensions_mut().insert(profile);
        return Ok(next.run(request).await);
    }

    // Fall back to internal profile header if allowed
    let store = auth_state.key_store.read().await;
    if store.allow_internal_profile_header {
        drop(store);
        let profile = extract_profile(&headers)?;

        debug!(
            profile_id = %profile.profile_id,
            role = ?profile.role,
            "Profile extracted via internal header (no API key)"
        );

        request.extensions_mut().insert(profile);
        return Ok(next.run(request).await);
    }
    drop(store);

    // No valid auth mechanism found
    warn!("No authentication credentials provided");
    Err(ApiError::Unauthorized)
}

/// Check if a profile has a specific permission.
///
/// This is called by route handlers that need to enforce authorization.
/// The profile must have been injected by auth_middleware first.
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
            "Profile '{}' (role: {:?}) lacks '{permission}' permission",
            profile.profile_id, profile.role
        )));
    }

    Ok(())
}

// -- Model Access Filtering --

/// Check if a profile is allowed to access a specific model.
pub fn can_access_model(
    profile: &ProfileInfo,
    model_name: &str,
    is_teen_safe: bool,
    is_child_safe: bool,
    is_default: bool,
) -> bool {
    match &profile.permissions.model_filter {
        None => true,
        Some(ModelAccessFilter::TeenSafe) => is_teen_safe,
        Some(ModelAccessFilter::ChildSafe) => is_child_safe,
        Some(ModelAccessFilter::DefaultOnly) => is_default,
    }
}

// -- Config Loading --

/// Load API keys from a TOML config file.
///
/// Expected format:
/// ```toml
/// [settings]
/// allow_internal_profile_header = false
/// rate_limit_per_minute = 60
///
/// [[keys]]
/// hash = "sha256-hex-hash-of-key"
/// profile_id = "family-dad"
/// role = "admin"
/// display_name = "Dad"
/// ```
pub fn load_api_keys_from_toml(path: &std::path::Path) -> Result<ApiKeyStore, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Cannot read auth config {}: {e}", path.display()))?;

    let table: toml::Table =
        toml::from_str(&content).map_err(|e| format!("Invalid auth config TOML: {e}"))?;

    let mut store = ApiKeyStore::new();

    // Parse settings
    if let Some(settings) = table.get("settings").and_then(|v| v.as_table()) {
        store.allow_internal_profile_header = settings
            .get("allow_internal_profile_header")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false);
    }

    // Parse keys
    if let Some(keys_array) = table.get("keys").and_then(|v| v.as_array()) {
        for key_val in keys_array {
            let key_table = key_val
                .as_table()
                .ok_or("Each [[keys]] entry must be a table")?;

            let key_hash = key_table
                .get("hash")
                .and_then(|v| v.as_str())
                .ok_or("Each key must have a 'hash' field")?
                .to_string();

            let profile_id = key_table
                .get("profile_id")
                .and_then(|v| v.as_str())
                .ok_or("Each key must have a 'profile_id' field")?
                .to_string();

            let role_str = key_table
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("guest");

            let role = match role_str.to_lowercase().as_str() {
                "admin" => ProfileRole::Admin,
                "adult" => ProfileRole::Adult,
                "teen" => ProfileRole::Teen,
                "child" => ProfileRole::Child,
                _ => ProfileRole::Guest,
            };

            let display_name = key_table
                .get("display_name")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string);

            store.add_key_hashed(key_hash, profile_id, role, display_name);
        }
    }

    info!(keys = store.len(), "Loaded API key store from config");
    Ok(store)
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ContentFilterLevel;

    #[test]
    fn test_hash_api_key_deterministic() {
        let key = "im-test-key-12345";
        let h1 = hash_api_key(key);
        let h2 = hash_api_key(key);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }

    #[test]
    fn test_api_key_store_add_and_validate() {
        let mut store = ApiKeyStore::new();
        store.add_key_raw(
            "im-test-key-abc",
            "admin1".to_string(),
            ProfileRole::Admin,
            Some("Test Admin".to_string()),
        );

        assert_eq!(store.len(), 1);

        let entry = store.validate("im-test-key-abc");
        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert_eq!(entry.profile_id, "admin1");
        assert!(matches!(entry.role, ProfileRole::Admin));

        // Wrong key should fail
        assert!(store.validate("im-wrong-key").is_none());
    }

    #[test]
    fn test_api_key_store_hashed_add() {
        let mut store = ApiKeyStore::new();
        let hash = hash_api_key("im-my-secret");
        store.add_key_hashed(hash, "user1".to_string(), ProfileRole::Adult, None);

        assert!(store.validate("im-my-secret").is_some());
        assert!(store.validate("im-other-secret").is_none());
    }

    #[test]
    fn test_generate_api_key_format() {
        let key = generate_api_key();
        assert!(key.starts_with("im-"));
        // 32 random bytes -> 64 hex chars, plus the "im-" prefix.
        assert_eq!(key.len(), 67);
        // Body must be valid hex.
        assert!(key[3..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_api_key_is_unique() {
        // CSPRNG should never collide across short bursts.
        let mut keys = std::collections::HashSet::new();
        for _ in 0..50 {
            assert!(keys.insert(generate_api_key()));
        }
    }

    #[test]
    fn test_rate_limiter_allows_within_limit() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let limiter = RateLimiter::new(5, 60);
        rt.block_on(async {
            for _ in 0..5 {
                assert!(limiter.check_rate_limit("key1").await.is_ok());
            }
            // 6th request should be rejected
            assert!(limiter.check_rate_limit("key1").await.is_err());
        });
    }

    #[test]
    fn test_rate_limiter_different_keys_independent() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let limiter = RateLimiter::new(2, 60);
        rt.block_on(async {
            assert!(limiter.check_rate_limit("key_a").await.is_ok());
            assert!(limiter.check_rate_limit("key_a").await.is_ok());
            assert!(limiter.check_rate_limit("key_a").await.is_err());
            // key_b should be unaffected
            assert!(limiter.check_rate_limit("key_b").await.is_ok());
        });
    }

    #[test]
    fn test_is_auth_exempt() {
        assert!(is_auth_exempt("/v1/health"));
        assert!(is_auth_exempt("/v1/health/adapters"));
        assert!(is_auth_exempt("/v1/health/hardware"));
        assert!(!is_auth_exempt("/v1/chat/completions"));
        assert!(!is_auth_exempt("/v1/models"));
        assert!(!is_auth_exempt("/v1/power"));
    }

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
        assert!(check_permission(&profile, "nonexistent").is_err());
    }
}
