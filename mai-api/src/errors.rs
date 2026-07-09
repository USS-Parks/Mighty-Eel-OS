//! API error types with MAI-XYYY error codes and HTTP status mapping.
//!
//! Error codes follow the MAI scheme:
//! - MAI-1XXX: Request errors (bad input, validation failure)
//! - MAI-2XXX: Model errors (unavailable, incompatible, loading)
//! - MAI-3XXX: System errors (overloaded, hardware, internal)
//! - MAI-4XXX: Auth errors (unauthorized, forbidden, profile, rate-limited)
//! - MAI-5XXX: Config errors (invalid config, air-gap violation)
//!
//! Backend opacity: no backend-specific names, paths, or internal
//! details are ever exposed through error responses. All backend
//! errors are mapped to generic MAI codes.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// API error with MAI error code, HTTP status, and safe message.
///
/// This enum covers all error conditions the API can return.
/// Internal details are logged server-side via tracing but never
/// included in the response body.
#[derive(Debug, Clone)]
pub enum ApiError {
    // ─── MAI-1XXX: Request Errors ───────────────────────────────
    /// MAI-1001: Malformed request body (JSON parse failure)
    BadRequest(String),
    /// MAI-1002: Request validation failed (missing field, out of range)
    ValidationFailed(String),
    /// MAI-1003: Unsupported content type
    UnsupportedContentType,
    /// MAI-1004: Request payload too large
    PayloadTooLarge,
    /// MAI-1005: Request timeout (client-side or queue timeout)
    RequestTimeout,

    // ─── MAI-2XXX: Model Errors ─────────────────────────────────
    /// MAI-2001: Requested model not found in registry
    ModelNotFound(String),
    /// MAI-2002: Model exists but is not loaded / available
    ModelUnavailable(String),
    /// MAI-2003: Model does not support requested operation
    ModelIncompatible(String),
    /// MAI-2004: Model is currently loading
    ModelLoading,

    // ─── MAI-3XXX: System Errors ────────────────────────────────
    /// MAI-3001: System overloaded (backpressure active)
    SystemOverloaded,
    /// MAI-3002: Internal error (details logged, not exposed)
    InternalError,
    /// MAI-3003: Hardware fault detected
    HardwareFault,
    /// MAI-3004: Service unavailable (shutting down or starting up)
    ServiceUnavailable,
    /// MAI-3005: Adapter process crashed during inference
    AdapterCrashed(String),

    // ─── MAI-4XXX: Auth Errors ──────────────────────────────────
    /// MAI-4001: Permission denied for this profile role
    PermissionDenied(String),
    /// MAI-4002: Missing or invalid profile header
    Unauthorized,
    /// MAI-4003: Profile not found in profile store
    ProfileNotFound(String),
    /// MAI-4004: Token validation failed
    TokenInvalid,
    /// MAI-4005: Rate limit exceeded. Payload is retry_after seconds.
    RateLimited(u64),

    // ─── MAI-5XXX: Config Errors ────────────────────────────────
    /// MAI-5001: Configuration error
    ConfigError(String),
    /// MAI-5002: Air-gap violation detected
    AirGapViolation(String),
    /// MAI-5003: Endpoint deliberately disabled by the active profile
    /// (e.g. `POST /v1/auth/exchange_token` under a profile
    /// with `TrustExchangeMode::Disabled`). Distinguished from a 404 so
    /// operators can tell "endpoint disabled by config" from "route
    /// missing from build".
    EndpointDisabled(String),
    /// MAI-5004: A wired endpoint whose behavior is not yet implemented.
    /// Returned instead of a fabricated success so a client can distinguish an
    /// unimplemented endpoint from a working one (audit P4). Maps to HTTP 501.
    NotImplemented(String),
}

impl ApiError {
    /// MAI error code string (e.g., "MAI-1001")
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "MAI-1001",
            Self::ValidationFailed(_) => "MAI-1002",
            Self::UnsupportedContentType => "MAI-1003",
            Self::PayloadTooLarge => "MAI-1004",
            Self::RequestTimeout => "MAI-1005",
            Self::ModelNotFound(_) => "MAI-2001",
            Self::ModelUnavailable(_) => "MAI-2002",
            Self::ModelIncompatible(_) => "MAI-2003",
            Self::ModelLoading => "MAI-2004",
            Self::SystemOverloaded => "MAI-3001",
            Self::InternalError => "MAI-3002",
            Self::HardwareFault => "MAI-3003",
            Self::ServiceUnavailable => "MAI-3004",
            Self::AdapterCrashed(_) => "MAI-3005",
            Self::PermissionDenied(_) => "MAI-4001",
            Self::Unauthorized => "MAI-4002",
            Self::ProfileNotFound(_) => "MAI-4003",
            Self::TokenInvalid => "MAI-4004",
            Self::RateLimited(_) => "MAI-4005",
            Self::ConfigError(_) => "MAI-5001",
            Self::AirGapViolation(_) => "MAI-5002",
            Self::EndpointDisabled(_) => "MAI-5003",
            Self::NotImplemented(_) => "MAI-5004",
        }
    }

    /// HTTP status code for this error
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) | Self::ModelIncompatible(_) => StatusCode::BAD_REQUEST,
            Self::ValidationFailed(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::UnsupportedContentType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::RequestTimeout => StatusCode::REQUEST_TIMEOUT,
            Self::ModelNotFound(_) => StatusCode::NOT_FOUND,
            Self::ModelUnavailable(_)
            | Self::ModelLoading
            | Self::ServiceUnavailable
            | Self::AdapterCrashed(_)
            | Self::AirGapViolation(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::SystemOverloaded | Self::RateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::InternalError | Self::HardwareFault | Self::ConfigError(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            Self::PermissionDenied(_) => StatusCode::FORBIDDEN,
            Self::Unauthorized | Self::ProfileNotFound(_) | Self::TokenInvalid => {
                StatusCode::UNAUTHORIZED
            }
            Self::EndpointDisabled(_) => StatusCode::GONE,
            Self::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
        }
    }

    /// Error type category for the response JSON
    pub fn error_type(&self) -> &'static str {
        match self {
            Self::BadRequest(_)
            | Self::ValidationFailed(_)
            | Self::UnsupportedContentType
            | Self::PayloadTooLarge
            | Self::RequestTimeout => "request_error",
            Self::ModelNotFound(_)
            | Self::ModelUnavailable(_)
            | Self::ModelIncompatible(_)
            | Self::ModelLoading => "model_error",
            Self::SystemOverloaded
            | Self::InternalError
            | Self::HardwareFault
            | Self::ServiceUnavailable
            | Self::AdapterCrashed(_) => "system_error",
            Self::PermissionDenied(_)
            | Self::Unauthorized
            | Self::ProfileNotFound(_)
            | Self::TokenInvalid
            | Self::RateLimited(_) => "auth_error",
            Self::ConfigError(_) | Self::AirGapViolation(_) | Self::EndpointDisabled(_) => {
                "config_error"
            }
            Self::NotImplemented(_) => "server_error",
        }
    }

    /// Safe, user-facing error message. Never includes backend names,
    /// internal paths, or stack traces.
    pub fn safe_message(&self) -> String {
        match self {
            Self::BadRequest(detail) => {
                format!("Bad request: {}", sanitize_error_detail(detail))
            }
            Self::ValidationFailed(detail) => {
                format!("Validation failed: {}", sanitize_error_detail(detail))
            }
            Self::UnsupportedContentType => "Unsupported content type".to_string(),
            Self::PayloadTooLarge => "Request payload exceeds size limit".to_string(),
            Self::RequestTimeout => "Request timed out".to_string(),
            Self::ModelNotFound(id) => format!("Model not found: {id}"),
            Self::ModelUnavailable(_) => "Requested model is not available".to_string(),
            Self::ModelIncompatible(detail) => {
                format!(
                    "Model does not support this operation: {}",
                    sanitize_error_detail(detail)
                )
            }
            Self::ModelLoading => "Model is currently loading, try again shortly".to_string(),
            Self::SystemOverloaded => "System is at capacity, try again later".to_string(),
            Self::InternalError => "An internal error occurred".to_string(),
            Self::HardwareFault => "A hardware fault was detected".to_string(),
            Self::ServiceUnavailable => "Service is temporarily unavailable".to_string(),
            Self::AdapterCrashed(_) => "Adapter process crashed, request failed".to_string(),
            Self::PermissionDenied(detail) => {
                format!("Permission denied: {}", sanitize_error_detail(detail))
            }
            Self::Unauthorized => "Authentication required".to_string(),
            Self::ProfileNotFound(_) => "Profile not found".to_string(),
            Self::TokenInvalid => "Invalid authentication token".to_string(),
            Self::RateLimited(retry_after) => {
                format!("Rate limit exceeded, retry after {retry_after} seconds")
            }
            Self::ConfigError(_) => "Configuration error".to_string(),
            Self::AirGapViolation(_) => {
                "Air-gap policy violation detected, service suspended".to_string()
            }
            Self::EndpointDisabled(detail) => {
                format!(
                    "Endpoint disabled by active profile: {}",
                    sanitize_error_detail(detail)
                )
            }
            Self::NotImplemented(detail) => {
                format!("Not implemented: {}", sanitize_error_detail(detail))
            }
        }
    }
}

/// JSON error response body matching the spec:
/// `{ "error": { "code": "MAI-XYYY", "message": "...", "type": "..." } }`
///
/// Rate-limited responses additionally include `retry_after_seconds`.
#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: String,
    message: String,
    #[serde(rename = "type")]
    error_type: String,
    /// Seconds until the client should retry. Only set for rate-limited errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_seconds: Option<u64>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let retry_after = match &self {
            ApiError::RateLimited(secs) => Some(*secs),
            _ => None,
        };

        let body = ErrorResponse {
            error: ErrorBody {
                code: self.code().to_string(),
                message: self.safe_message(),
                error_type: self.error_type().to_string(),
                retry_after_seconds: retry_after,
            },
        };

        let status = self.status();

        // Log the error server-side with full detail
        tracing::warn!(
            code = self.code(),
            status = status.as_u16(),
            error_type = self.error_type(),
            "API error response"
        );

        let mut response = (status, axum::Json(body)).into_response();

        // Add Retry-After header for rate-limited responses (RFC 7231 §7.1.3)
        if let Some(secs) = retry_after
            && let Ok(val) = axum::http::HeaderValue::from_str(&secs.to_string())
        {
            response.headers_mut().insert("Retry-After", val);
        }

        response
    }
}

// ─── From Conversions: mai-core errors -> ApiError ──────────────────

impl From<mai_core::CoreError> for ApiError {
    fn from(err: mai_core::CoreError) -> Self {
        // Log the full internal error server-side
        tracing::error!(error = %err, "Core error mapped to API error");

        match err {
            mai_core::CoreError::RequestFailed(detail) => {
                // Strip any backend-specific info
                Self::BadRequest(sanitize_error_detail(&detail))
            }
            mai_core::CoreError::ModelUnavailable(detail) => {
                Self::ModelUnavailable(sanitize_error_detail(&detail))
            }
            mai_core::CoreError::Overloaded => Self::SystemOverloaded,
            mai_core::CoreError::AirGapViolation(detail) => {
                Self::AirGapViolation(sanitize_error_detail(&detail))
            }
            mai_core::CoreError::Internal(_) => {
                // Never expose internal error details
                Self::InternalError
            }
        }
    }
}

/// Strip potential backend-specific information from error details.
/// This is a defense-in-depth measure: even if a backend name leaks
/// into a CoreError message, it won't reach the API consumer.
fn sanitize_error_detail(detail: &str) -> String {
    // List of backend names that must never appear in API responses
    const BACKEND_NAMES: &[&str] = &[
        "ollama",
        "vllm",
        "llama.cpp",
        "llamacpp",
        "tgi",
        "tensorrt",
        "exllamav2",
        "sglang",
        "huggingface",
    ];

    let detail = redact_paths(detail);
    let lower = detail.to_lowercase();
    for name in BACKEND_NAMES {
        if lower.contains(name) {
            return "Operation could not be completed".to_string();
        }
    }
    detail
}

fn redact_paths(input: &str) -> String {
    const REDACTED: &str = "<redacted_path>";
    let mut out: Vec<&str> = Vec::new();
    for token in input.split_whitespace() {
        let looks_windows = token.contains(":\\") || token.contains(":/");
        let looks_posix = token.starts_with("/Users/")
            || token.starts_with("/home/")
            || token.starts_with("/etc/")
            || token.starts_with("/var/")
            || token.starts_with("/opt/")
            || token.starts_with("/tmp/");
        if looks_windows || looks_posix {
            out.push(REDACTED);
        } else {
            out.push(token);
        }
    }
    out.join(" ")
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code(), self.safe_message())
    }
}

impl std::error::Error for ApiError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_codes_match_categories() {
        assert!(ApiError::BadRequest("x".into()).code().starts_with("MAI-1"));
        assert!(
            ApiError::ModelNotFound("x".into())
                .code()
                .starts_with("MAI-2")
        );
        assert!(ApiError::SystemOverloaded.code().starts_with("MAI-3"));
        assert!(ApiError::Unauthorized.code().starts_with("MAI-4"));
        assert!(
            ApiError::ConfigError("x".into())
                .code()
                .starts_with("MAI-5")
        );
    }

    #[test]
    fn test_status_codes() {
        assert_eq!(
            ApiError::BadRequest("x".into()).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(ApiError::Unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            ApiError::PermissionDenied("x".into()).status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            ApiError::ModelNotFound("x".into()).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ApiError::SystemOverloaded.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            ApiError::InternalError.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn not_implemented_maps_to_501() {
        let err = ApiError::NotImplemented("listing all profiles".into());
        assert_eq!(err.code(), "MAI-5004");
        assert_eq!(err.status(), StatusCode::NOT_IMPLEMENTED);
        assert_eq!(err.error_type(), "server_error");
        assert!(err.safe_message().starts_with("Not implemented"));
    }

    #[test]
    fn test_rate_limited_error() {
        let err = ApiError::RateLimited(30);
        assert_eq!(err.code(), "MAI-4005");
        assert_eq!(err.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(err.error_type(), "auth_error");
        assert!(err.safe_message().contains("30"));
        assert!(err.safe_message().contains("retry"));
    }

    #[test]
    fn test_sanitize_strips_backend_names() {
        let detail = "ollama adapter returned error";
        let sanitized = sanitize_error_detail(detail);
        assert_eq!(sanitized, "Operation could not be completed");

        let detail = "Model not loaded";
        let sanitized = sanitize_error_detail(detail);
        assert_eq!(sanitized, "Model not loaded");
    }

    #[test]
    fn test_sanitize_redacts_paths() {
        let detail = "failed to read C:\\secrets\\auth_keys.toml: access denied";
        let sanitized = sanitize_error_detail(detail);
        assert!(!sanitized.contains("C:\\"));
        assert!(sanitized.contains("<redacted_path>"));

        let detail = "invalid config at /etc/mai/auth_keys.toml";
        let sanitized = sanitize_error_detail(detail);
        assert!(!sanitized.contains("/etc/mai/"));
        assert!(sanitized.contains("<redacted_path>"));
    }

    #[test]
    fn test_core_error_mapping() {
        let err: ApiError = mai_core::CoreError::Overloaded.into();
        assert_eq!(err.code(), "MAI-3001");

        let err: ApiError = mai_core::CoreError::ModelUnavailable("test".into()).into();
        assert_eq!(err.code(), "MAI-2002");

        let err: ApiError = mai_core::CoreError::Internal("secret".into()).into();
        assert_eq!(err.code(), "MAI-3002");
        // Internal details must not leak
        assert!(!err.safe_message().contains("secret"));
    }

    #[test]
    fn test_display_impl() {
        let err = ApiError::Unauthorized;
        let display = format!("{err}");
        assert!(display.contains("MAI-4002"));
        assert!(display.contains("Authentication required"));
    }
}
