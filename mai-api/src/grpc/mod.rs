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
//! # Authentication
//!
//! Privileged RPCs authenticate the caller per-request via `authenticate_grpc`,
//! which resolves the role from an `x-im-auth-token` API key against the shared
//! `ApiKeyStore` — the same store the REST path uses. Caller-supplied role
//! metadata (`x-im-profile`) is honored only as an explicit dev fallback when the
//! store has `allow_internal_profile_header` enabled (never in production).

#![allow(unused_variables, dead_code)]

/// Generated protobuf types from proto/mai.proto
#[allow(
    clippy::default_trait_access,
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::clone_on_ref_ptr,
    clippy::wildcard_imports,
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::derive_partial_eq_without_eq,
    clippy::redundant_closure_for_method_calls,
    clippy::return_self_not_must_use,
    clippy::struct_excessive_bools,
    clippy::unnecessary_wraps,
    clippy::used_underscore_binding,
    clippy::needless_pass_by_value
)]
pub mod proto {
    tonic::include_proto!("mai.v1");

    /// File descriptor set for tonic-reflection service discovery.
    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("mai_descriptor");
}

pub mod audit;
pub mod health;
pub mod inference;
pub mod models;
pub mod power;
pub mod registry;
pub mod server;

use tonic::{Request, Status};

use crate::auth::ApiKeyStore;
use crate::state::AppState;
use crate::types::ProfileRole;

/// Shared helper to convert a `mai-core` `ModelSummary` to the gRPC `ModelDetail`.
///
/// Keep this in one place so all gRPC services stay consistent.
pub(crate) fn model_summary_to_proto_detail(
    m: &mai_core::registry::ModelSummary,
    created: u64,
) -> proto::ModelDetail {
    proto::ModelDetail {
        id: m.model_id.clone(),
        object: "model".to_string(),
        created,
        owned_by: "island-mountain".to_string(),
        capabilities: Some(proto::ModelCapabilities {
            chat: m.capabilities.chat,
            completion: m.capabilities.completion,
            embedding: m.capabilities.embedding,
            vision: m.capabilities.vision,
            structured_output: m.capabilities.structured_output,
            max_context_tokens: m.capabilities.max_context_tokens,
        }),
        status: format!("{:?}", m.status),
        size_bytes: m.size_bytes,
        required_vram_bytes: m.required_vram_bytes,
    }
}

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
                "unknown role '{role}'; expected admin, adult, teen, child, or guest"
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
        "manage_models" | "power_control" | "registry_write" | "view_audit" | "manage_profiles" => {
            role == "admin"
        }
        _ => false,
    }
}

/// gRPC metadata key carrying the API key (mirrors the REST `X-IM-Auth-Token`).
pub const GRPC_AUTH_TOKEN_KEY: &str = "x-im-auth-token";

/// Lowercase role name consumed by [`role_has_permission`].
fn role_to_str(role: ProfileRole) -> &'static str {
    match role {
        ProfileRole::Admin => "admin",
        ProfileRole::Adult => "adult",
        ProfileRole::Teen => "teen",
        ProfileRole::Child => "child",
        ProfileRole::Guest => "guest",
    }
}

/// Resolve the authenticated caller identity for a privileged gRPC request
/// (finding AF-03). The role is derived from a verified `x-im-auth-token` API
/// key against the shared key store — never from caller-supplied `x-im-profile`
/// metadata. The legacy self-declared-profile path is honored only when the
/// store explicitly enables `allow_internal_profile_header` (dev/internal),
/// exactly mirroring the REST auth middleware; in production it is unreachable.
#[allow(clippy::result_large_err)]
pub fn resolve_grpc_identity<T>(
    store: &ApiKeyStore,
    request: &Request<T>,
) -> Result<(String, String), Status> {
    if let Some(token) = request.metadata().get(GRPC_AUTH_TOKEN_KEY) {
        let raw = token.to_str().map_err(|_| {
            Status::unauthenticated("x-im-auth-token contains non-ASCII characters")
        })?;
        let entry = store
            .validate(raw)
            .ok_or_else(|| Status::unauthenticated("invalid api key"))?;
        return Ok((
            entry.profile_id.clone(),
            role_to_str(entry.role).to_string(),
        ));
    }
    if store.allow_internal_profile_header {
        // Dev/internal only: trust the self-declared profile header. This path is
        // off in production (`allow_internal_profile_header == false`).
        return extract_grpc_profile(request);
    }
    Err(Status::unauthenticated(
        "missing x-im-auth-token; caller-supplied x-im-profile is not trusted",
    ))
}

/// Authenticate a privileged gRPC request against the shared key store.
#[allow(clippy::result_large_err)]
pub async fn authenticate_grpc<T>(
    state: &AppState,
    request: &Request<T>,
) -> Result<(String, String), Status> {
    let store = state.auth.key_store.read().await;
    resolve_grpc_identity(&store, request)
}

/// Convert an ApiError-style error into a tonic Status.
pub fn api_error_to_status(code: &str, message: &str) -> Status {
    if code.starts_with("MAI-1") {
        Status::invalid_argument(message.to_string())
    } else if code.starts_with("MAI-2") {
        Status::not_found(message.to_string())
    } else if code.starts_with("MAI-3") {
        Status::internal(message.to_string())
    } else if code.starts_with("MAI-4") {
        Status::permission_denied(message.to_string())
    } else if code.starts_with("MAI-5") {
        Status::failed_precondition(message.to_string())
    } else {
        Status::unknown(message.to_string())
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
    fn test_resolve_grpc_identity_valid_token() {
        let mut store = ApiKeyStore::new();
        store.add_key_raw("im-secret", "svc".to_string(), ProfileRole::Admin, None);
        let mut req = Request::new(());
        req.metadata_mut()
            .insert(GRPC_AUTH_TOKEN_KEY, "im-secret".parse().unwrap());
        let (id, role) = resolve_grpc_identity(&store, &req).unwrap();
        assert_eq!(id, "svc");
        assert_eq!(role, "admin");
    }

    #[test]
    fn test_resolve_grpc_identity_invalid_token() {
        let store = ApiKeyStore::new();
        let mut req = Request::new(());
        req.metadata_mut()
            .insert(GRPC_AUTH_TOKEN_KEY, "wrong".parse().unwrap());
        let err = resolve_grpc_identity(&store, &req).unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    #[test]
    fn test_resolve_grpc_identity_rejects_caller_claimed_role() {
        // AF-03 regression: a caller-supplied x-im-profile with no API key must
        // NOT confer a role when the internal-header path is off (production).
        let store = ApiKeyStore::new(); // allow_internal_profile_header = false
        let mut req = Request::new(());
        req.metadata_mut()
            .insert(GRPC_PROFILE_KEY, "attacker:admin".parse().unwrap());
        let err = resolve_grpc_identity(&store, &req).unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    #[test]
    fn test_resolve_grpc_identity_dev_header_fallback() {
        // Dev/internal only: with the header path explicitly enabled, a
        // self-declared profile is honored (mirrors REST semantics).
        let mut store = ApiKeyStore::new();
        store.allow_internal_profile_header = true;
        let mut req = Request::new(());
        req.metadata_mut()
            .insert(GRPC_PROFILE_KEY, "dev:admin".parse().unwrap());
        let (id, role) = resolve_grpc_identity(&store, &req).unwrap();
        assert_eq!(id, "dev");
        assert_eq!(role, "admin");
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
