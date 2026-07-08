//! MaiAudit gRPC service implementation.
//!
//! Provides paginated audit log retrieval. The audit trail is a
//! SHA3-256 hash-chained log of all API interactions. Only Admin
//! profiles can view audit logs.

use tonic::{Request, Response, Status};
use tracing::debug;

use super::proto;
use super::{authenticate_grpc, role_has_permission};
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
        let (profile_id, role) = authenticate_grpc(&self.state, &request).await?;
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
            #[allow(clippy::cast_possible_truncation)]
            let count = (req.offset + limit) as usize;
            audit_writer
                .read_recent(count)
                .await
                .map_err(|e| Status::internal(format!("audit read failed: {e}")))?
        } else {
            #[allow(clippy::cast_possible_truncation)]
            let count = (req.offset + limit) as usize;
            audit_writer
                .read_by_profile(&req.profile_filter, count)
                .await
                .map_err(|e| Status::internal(format!("audit read failed: {e}")))?
        };

        // Get total count for pagination
        let total = audit_writer
            .entry_count()
            .await
            .map_err(|e| Status::internal(format!("audit count failed: {e}")))?;

        // Apply offset (skip first N entries)
        #[allow(clippy::cast_possible_truncation)]
        let offset = req.offset as usize;
        #[allow(clippy::cast_possible_truncation)]
        let limit_usize = limit as usize;
        let page_entries: Vec<_> = raw_entries
            .into_iter()
            .skip(offset)
            .take(limit_usize)
            .collect();

        let entries: Vec<proto::AuditEntry> = page_entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                #[allow(clippy::cast_possible_truncation)]
                let sequence = (offset + i) as u64;
                let status_code = u32::from(e.status_code);
                proto::AuditEntry {
                    sequence,
                    timestamp: e.timestamp.to_string(),
                    profile_id: e.profile_id.clone(),
                    method: e.method.clone(),
                    endpoint: e.path.clone(),
                    model: e.model_name.clone().unwrap_or_else(String::new),
                    tokens_in: 0,  // Not tracked in AuditEntry
                    tokens_out: 0, // Not tracked in AuditEntry
                    latency_ms: e.duration_ms,
                    status_code,
                    request_id: e.entry_id.clone(),
                    chain_hash: e.entry_hash.clone(),
                }
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
