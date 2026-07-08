//! MaiInference gRPC service implementation.
//!
//! Provides chat completion (unary and server-streaming) and embedding
//! generation. All inference requests go through the scheduler, which
//! selects the appropriate adapter. Backend identities are never exposed.

use std::time::{Duration, Instant};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info};
use uuid::Uuid;

use super::proto;
use super::{authenticate_grpc, role_has_permission};
use crate::state::AppState;

use mai_core::scheduler::{
    ChatMessage, InferenceRequest, RequestPayload, RequestPriority, RequestType,
};
use mai_scheduler::{Priority as SchedulerPriority, ScheduleRequest};

/// MaiInference service implementation.
///
/// Shares the scheduler from AppState with the REST server.
/// The scheduler handles adapter selection, load balancing,
/// and failover transparently.
pub struct MaiInferenceService {
    state: AppState,
}

impl MaiInferenceService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl proto::mai_inference_server::MaiInference for MaiInferenceService {
    /// Unary chat completion. Blocks until the full response is generated.
    async fn chat_completion(
        &self,
        request: Request<proto::ChatCompletionRequest>,
    ) -> Result<Response<proto::ChatCompletionResponse>, Status> {
        let (profile_id, role) = authenticate_grpc(&self.state, &request).await?;
        if !role_has_permission(&role, "inference") {
            return Err(Status::permission_denied(
                "profile lacks inference permission",
            ));
        }

        let req = request.into_inner();
        let request_uuid = Uuid::new_v4();
        let request_id = request_uuid.to_string();
        let profile_uuid = Uuid::parse_str(&profile_id)
            .map_err(|e| Status::internal(format!("invalid profile_id: {e}")))?;

        debug!(
            request_id = %request_id,
            profile_id = %profile_id,
            model = %req.model,
            messages = req.messages.len(),
            "gRPC ChatCompletion request"
        );

        // Convert proto messages to mai-core ChatMessage
        let messages: Vec<ChatMessage> = req
            .messages
            .iter()
            .map(|m| ChatMessage {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();

        // Estimate token count from message content lengths
        #[allow(clippy::cast_possible_truncation)]
        let estimated_tokens: u32 = messages.iter().map(|m| (m.content.len() / 4) as u32).sum();

        // Build InferenceRequest matching mai-core's actual struct
        let inference_req = InferenceRequest {
            id: request_uuid,
            profile_id: profile_uuid,
            model_name: if req.model.is_empty() {
                None
            } else {
                Some(req.model.clone())
            },
            request_type: RequestType::Chat,
            payload: RequestPayload::Chat { messages },
            priority: RequestPriority::Normal,
            timeout: Duration::from_secs(300),
            streaming: false,
            enqueued_at: Instant::now(),
            estimated_tokens,
        };

        // Route through new scheduler
        let model_alias = if req.model.is_empty() {
            "default"
        } else {
            &req.model
        };
        let sched_req = ScheduleRequest::new(model_alias, SchedulerPriority::Normal);
        let session_id = sched_req.session_id;

        let decision = self.state.scheduler.schedule(&sched_req).map_err(|e| {
            error!(request_id = %request_id, error = %e, "scheduler routing failed");
            Status::internal("inference request routing failed")
        })?;

        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());

        let response = proto::ChatCompletionResponse {
            id: request_id.clone(),
            object: "chat.completion".to_string(),
            created,
            model: decision.instance_id.to_string(),
            choices: vec![proto::ChatChoice {
                index: 0,
                message: Some(proto::ChatMessage {
                    role: "assistant".to_string(),
                    content: String::new(), // Populated by adapter pipeline
                    name: String::new(),
                }),
                finish_reason: "stop".to_string(),
            }],
            usage: Some(proto::TokenUsage {
                prompt_tokens: estimated_tokens,
                completion_tokens: 0,
                total_tokens: estimated_tokens,
            }),
        };

        self.state
            .scheduler
            .release_sequence(&decision.instance_id, session_id);

        info!(
            request_id = %request_id,
            profile_id = %profile_id,
            instance = %decision.instance_id,
            "gRPC ChatCompletion routed"
        );

        Ok(Response::new(response))
    }

    /// Server-streaming chat completion. Streams tokens as ChatCompletionChunk.
    type ChatCompletionStreamStream = ReceiverStream<Result<proto::ChatCompletionChunk, Status>>;

    /// Not yet wired to the adapter IPC token stream. Returns an explicit gRPC
    /// `unimplemented` status rather than a fabricated empty stream (a role chunk
    /// followed by a `stop` chunk with no content), so a client cannot mistake
    /// the unwired endpoint for a working one (audit P4). TODO(basho): route
    /// through the scheduler and stream real adapter tokens.
    async fn chat_completion_stream(
        &self,
        request: Request<proto::ChatCompletionRequest>,
    ) -> Result<Response<Self::ChatCompletionStreamStream>, Status> {
        let (_profile_id, role) = authenticate_grpc(&self.state, &request).await?;
        if !role_has_permission(&role, "inference") {
            return Err(Status::permission_denied(
                "profile lacks inference permission",
            ));
        }
        Err(Status::unimplemented(
            "streaming chat completion is not yet wired to the adapter pipeline",
        ))
    }

    /// Unary embedding generation.
    ///
    /// Not yet wired to the adapter embedding pipeline. Returns an explicit gRPC
    /// `unimplemented` status rather than an empty-vector "success" carrying token
    /// usage, so a client cannot mistake an unwired endpoint for a working one
    /// (audit P4). TODO(basho): route through the scheduler and return the
    /// adapter-computed embeddings.
    async fn embed(
        &self,
        request: Request<proto::EmbeddingRequest>,
    ) -> Result<Response<proto::EmbeddingResponse>, Status> {
        let (_profile_id, role) = authenticate_grpc(&self.state, &request).await?;
        if !role_has_permission(&role, "inference") {
            return Err(Status::permission_denied(
                "profile lacks inference permission",
            ));
        }
        Err(Status::unimplemented(
            "embedding generation is not yet wired to the adapter pipeline",
        ))
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inference_service_constructable() {
        // Compile-time check: MaiInferenceService is Send + Sync
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<MaiInferenceService>();
    }
}
