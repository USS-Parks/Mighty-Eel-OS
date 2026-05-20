//! MaiAudit gRPC service implementation.
//!
//! Provides paginated audit log retrieval. The audit trail is a
//! SHA3-256 hash-chained log of all API interactions. Only Admin
//! profiles can view audit logs.

use tonic::{Request, Response, Status};
use tracing::debug;

use super::proto;
use super::{extract_grpc_profile, role_has_permission};
use crate::state::AppState;

/// MaiAudit service implementation.
pub struct MaiAuditService {
    state: AppState,
}

impl MaiAuditService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl proto::mai_audit_server::MaiAudit for MaiAuditService {
    /// Retrieve audit log entries with pagination.
    async fn get_audit_log(
        &self,
        request: Request<proto::AuditLogRequest>,
    ) -> Result<Response<proto::AuditLogResponse>, Status> {
        let (profile_id, role) = extract_grpc_profile(&request)?;
        if !role_has_permission(&role, "view_audit") {
            return Err(Status::permission_denied(
                "admin role required to view audit logs",
            ));
        }

        let req = request.into_inner();
        let limit = if req.limit == 0 || req.limit > 100 {
            50u64
        } else {
            req.limit
        };

        debug!(
            profile_id = %profile_id,
            offset = req.offset,
            limit = limit,
            profile_filter = %req.profile_filter,
            "gRPC GetAuditLog"
        );

        let audit_writer = &self.state.audit_writer;

        // Use the AuditWriter trait methods: read_recent or read_by_profile
        let raw_entries = if req.profile_filter.is_empty() {
            // Read recent entries (offset + limit to get enough, then slice)
            audit_writer
                .read_recent((req.offset + limit) as usize)
                .await
                .map_err(|e| Status::internal(format!("audit read failed: {e}")))?
        } else {
            audit_writer
                .read_by_profile(&req.profile_filter, (req.offset + limit) as usize)
                .await
                .map_err(|e| Status::internal(format!("audit read failed: {e}")))?
        };

        // Get total count for pagination
        let total = audit_writer
            .entry_count()
            .await
            .map_err(|e| Status::internal(format!("audit count failed: {e}")))?;

        // Apply offset (skip first N entries)
        let offset = req.offset as usize;
        let page_entries: Vec<_> = raw_entries
            .into_iter()
            .skip(offset)
            .take(limit as usize)
            .collect();

        let entries: Vec<proto::AuditEntry> = page_entries
            .iter()
            .enumerate()
            .map(|(i, e)| proto::AuditEntry {
                sequence: (offset + i) as u64,
                timestamp: e.timestamp.to_string(),
                profile_id: e.profile_id.clone(),
                method: e.method.clone(),
                endpoint: e.path.clone(),
                model: e.model_name.clone().unwrap_or_default(),
                tokens_in: 0,  // Not tracked in AuditEntry
                tokens_out: 0, // Not tracked in AuditEntry
                latency_ms: e.duration_ms,
                status_code: e.status_code as u32,
                request_id: e.entry_id.clone(),
                chain_hash: e.entry_hash.clone(),
            })
            .collect();

        Ok(Response::new(proto::AuditLogResponse {
            entries,
            total,
            offset: req.offset,
            limit,
        }))
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_service_constructable() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<MaiAuditService>();
    }
}
