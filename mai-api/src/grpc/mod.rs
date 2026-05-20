//! gRPC service implementations for the MAI API server.
//!
//! This module contains tonic service implementations for all six MAI
//! gRPC services plus the standard grpc.health.v1 health checking protocol.
//! All services share AppState with the REST server via Arc references.
//!
//! # Services
//!
//! - `MaiInference`: Chat completion, streaming, embeddings
//! - `MaiModels`: Model listing, loading, unloading
//! - `MaiHealth`: System health, adapter health, hardware health, watch
//! - `MaiPower`: Power state queries and transitions
//! - `MaiRegistry`: Model registry queries and scanning
//! - `MaiAudit`: Audit log retrieval
//! - `Health`: Standard grpc.health.v1 (Check + Watch)
//!
//! # Auth Interceptor
//!
//! All RPCs pass through the auth interceptor which extracts the profile
//! from gRPC metadata (`x-im-profile` key) using the same logic as the
//! REST auth middleware. The profile_id is injected into request messages
//! before reaching service implementations.

#![allow(unused_variables, dead_code)]

/// Generated protobuf types from proto/mai.proto
pub mod proto {
    tonic::include_proto!("mai.v1");

    /// File descriptor set for tonic-reflection service discovery.
    pub const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("mai_descriptor");
}

pub mod inference;
pub mod models;
pub mod health;
pub mod power;
pub mod registry;
pub mod audit;
pub mod server;

use tonic::{Request, Status};

// ── Auth Interceptor ──────────────────────────────────────────────

/// gRPC metadata key for profile identification (mirrors REST X-IM-Profile header).
pub const GRPC_PROFILE_KEY: &str = "x-im-profile";

/// Extract profile ID and role from gRPC metadata.
///
/// Format: `profile_id:role` (e.g., `family-dad:admin`, `kid-timmy:child`)
/// Returns (profile_id, role_string) or a Status error.
#[allow(clippy::result_large_err)]
pub fn extract_grpc_profile<T>(request: &Request<T>) -> Result<(String, String), Status> {
    let metadata = request.metadata();
    let header_value = metadata
        .get(GRPC_PROFILE_KEY)
        .ok_or_else(|| Status::unauthenticated("missing x-im-profile metadata"))?;

    let value_str = header_value
        .to_str()
        .map_err(|_| Status::invalid_argument("x-im-profile contains non-ASCII characters"))?;

    let parts: Vec<&str> = value_str.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(Status::invalid_argument(
            "x-im-profile must be in format 'profile_id:role'",
        ));
    }

    let profile_id = parts[0].to_string();
    let role = parts[1].to_lowercase();

    // Validate role
    match role.as_str() {
        "admin" | "adult" | "teen" | "child" | "guest" => {}
        _ => {
            return Err(Status::invalid_argument(format!(
                "unknown role '{}'; expected admin, adult, teen, child, or guest",
                role
            )));
        }
    }

    Ok((profile_id, role))
}

/// Check if a role has a specific permission. Maps to the same permission
/// model as types.rs ProfileRole::permissions().
pub fn role_has_permission(role: &str, permission: &str) -> bool {
    match permission {
        "inference" => true, // All roles can do inference
        "list_models" => matches!(role, "admin" | "adult" | "teen"),
        "manage_models" => role == "admin",
        "power_control" => role == "admin",
        "registry_write" => role == "admin",
        "view_audit" => role == "admin",
        "manage_profiles" => role == "admin",
        _ => false,
    }
}

/// Convert an ApiError-style error into a tonic Status.
pub fn api_error_to_status(code: &str, message: &str) -> Status {
    match &code[..6] {
        "MAI-1" => Status::invalid_argument(message.to_string()),
        "MAI-2" => Status::not_found(message.to_string()),
        "MAI-3" => Status::internal(message.to_string()),
        "MAI-4" => Status::permission_denied(message.to_string()),
        "MAI-5" => Status::failed_precondition(message.to_string()),
        _ => Status::unknown(message.to_string()),
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_grpc_profile_valid() {
        let mut req = Request::new(());
        req.metadata_mut()
            .insert(GRPC_PROFILE_KEY, "family-dad:admin".parse().unwrap());
        let (id, role) = extract_grpc_profile(&req).unwrap();
        assert_eq!(id, "family-dad");
        assert_eq!(role, "admin");
    }

    #[test]
    fn test_extract_grpc_profile_missing() {
        let req = Request::new(());
        let result = extract_grpc_profile(&req);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }

    #[test]
    fn test_extract_grpc_profile_bad_format() {
        let mut req = Request::new(());
        req.metadata_mut()
            .insert(GRPC_PROFILE_KEY, "no-colon-here".parse().unwrap());
        let result = extract_grpc_profile(&req);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn test_extract_grpc_profile_unknown_role() {
        let mut req = Request::new(());
        req.metadata_mut()
            .insert(GRPC_PROFILE_KEY, "user:superadmin".parse().unwrap());
        let result = extract_grpc_profile(&req);
        assert!(result.is_err());
    }

    #[test]
    fn test_role_permissions() {
        assert!(role_has_permission("admin", "inference"));
        assert!(role_has_permission("admin", "manage_models"));
        assert!(role_has_permission("admin", "view_audit"));
        assert!(!role_has_permission("adult", "manage_models"));
        assert!(!role_has_permission("child", "list_models"));
        assert!(role_has_permission("guest", "inference"));
        assert!(!role_has_permission("guest", "list_models"));
    }

    #[test]
    fn test_api_error_to_status_mapping() {
        let s = api_error_to_status("MAI-1001", "bad request");
        assert_eq!(s.code(), tonic::Code::InvalidArgument);

        let s = api_error_to_status("MAI-2001", "model not found");
        assert_eq!(s.code(), tonic::Code::NotFound);

        let s = api_error_to_status("MAI-3001", "overloaded");
        assert_eq!(s.code(), tonic::Code::Internal);

        let s = api_error_to_status("MAI-4001", "forbidden");
        assert_eq!(s.code(), tonic::Code::PermissionDenied);

        let s = api_error_to_status("MAI-5001", "config error");
        assert_eq!(s.code(), tonic::Code::FailedPrecondition);
    }
}
