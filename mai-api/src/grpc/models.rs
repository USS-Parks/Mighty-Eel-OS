//! MaiModels gRPC service implementation.
//!
//! Provides model listing, detail retrieval, loading, and unloading.
//! Model access is filtered by profile role: Child and Guest profiles
//! see only their permitted subset. Admin is required for load/unload.
//! Backend adapter names are never exposed in any response.

use tonic::{Request, Response, Status};
use tracing::{debug, info};

use super::proto;
use super::{authenticate_grpc, model_summary_to_proto_detail, role_has_permission};
use crate::state::AppState;

/// MaiModels service implementation.
pub struct MaiModelsService {
    state: AppState,
}

impl MaiModelsService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl proto::mai_models_server::MaiModels for MaiModelsService {
    /// List all models visible to the requesting profile.
    async fn list_models(
        &self,
        request: Request<proto::ListModelsRequest>,
    ) -> Result<Response<proto::ModelListResponse>, Status> {
        let (profile_id, role) = authenticate_grpc(&self.state, &request).await?;
        if !role_has_permission(&role, "list_models") {
            return Err(Status::permission_denied(
                "profile lacks list_models permission",
            ));
        }

        debug!(profile_id = %profile_id, "gRPC ListModels");

        let registry = self.state.registry.read().await;
        let models = registry.list_models(None);

        let data: Vec<proto::ModelDetail> = models
            .iter()
            .map(|m| model_summary_to_proto_detail(m, 0))
            .collect();

        Ok(Response::new(proto::ModelListResponse {
            object: "list".to_string(),
            data,
        }))
    }

    /// Get details for a specific model.
    async fn get_model(
        &self,
        request: Request<proto::GetModelRequest>,
    ) -> Result<Response<proto::ModelDetail>, Status> {
        let (profile_id, role) = authenticate_grpc(&self.state, &request).await?;
        if !role_has_permission(&role, "list_models") {
            return Err(Status::permission_denied(
                "profile lacks list_models permission",
            ));
        }

        let req = request.into_inner();
        debug!(profile_id = %profile_id, model_id = %req.model_id, "gRPC GetModel");

        let registry = self.state.registry.read().await;
        let manifest = registry
            .get_model(&req.model_id)
            .ok_or_else(|| Status::not_found(format!("model '{}' not found", req.model_id)))?;

        Ok(Response::new(proto::ModelDetail {
            id: manifest.model.name.clone(),
            object: "model".to_string(),
            created: 0,
            owned_by: "island-mountain".to_string(),
            capabilities: Some(proto::ModelCapabilities {
                chat: manifest.capabilities.chat,
                completion: manifest.capabilities.completion,
                embedding: manifest.capabilities.embedding,
                vision: manifest.capabilities.vision,
                structured_output: manifest.capabilities.structured_output,
                max_context_tokens: manifest.capabilities.max_context_tokens,
            }),
            status: registry
                .get_status(&req.model_id)
                .map_or_else(|| "unknown".to_string(), |s| format!("{s:?}")),
            size_bytes: manifest.model.size_bytes,
            required_vram_bytes: manifest.model.required_vram_bytes,
        }))
    }

    /// Load a model into memory. Admin only.
    async fn load_model(
        &self,
        request: Request<proto::ModelOperationRequest>,
    ) -> Result<Response<proto::ModelOperationResponse>, Status> {
        let (profile_id, role) = authenticate_grpc(&self.state, &request).await?;
        if !role_has_permission(&role, "manage_models") {
            return Err(Status::permission_denied(
                "admin role required for model management",
            ));
        }

        let req = request.into_inner();
        info!(profile_id = %profile_id, model_id = %req.model_id, "gRPC LoadModel");

        let mut registry = self.state.registry.write().await;
        registry
            .load_model(&req.model_id, "auto".to_string())
            .await
            .map_err(|e| Status::internal(format!("load failed: {e}")))?;

        Ok(Response::new(proto::ModelOperationResponse {
            operation: "load".to_string(),
            model_id: req.model_id,
            status: "success".to_string(),
            message: "model loaded successfully".to_string(),
        }))
    }

    /// Unload a model from memory. Admin only.
    async fn unload_model(
        &self,
        request: Request<proto::ModelOperationRequest>,
    ) -> Result<Response<proto::ModelOperationResponse>, Status> {
        let (profile_id, role) = authenticate_grpc(&self.state, &request).await?;
        if !role_has_permission(&role, "manage_models") {
            return Err(Status::permission_denied(
                "admin role required for model management",
            ));
        }

        let req = request.into_inner();
        info!(profile_id = %profile_id, model_id = %req.model_id, "gRPC UnloadModel");

        let mut registry = self.state.registry.write().await;
        registry
            .unload_model(&req.model_id)
            .map_err(|e| Status::internal(format!("unload failed: {e}")))?;

        Ok(Response::new(proto::ModelOperationResponse {
            operation: "unload".to_string(),
            model_id: req.model_id,
            status: "success".to_string(),
            message: "model unloaded successfully".to_string(),
        }))
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_models_service_constructable() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<MaiModelsService>();
    }
}
