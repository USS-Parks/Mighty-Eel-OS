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
use super::{extract_grpc_profile, role_has_permission};
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
        let (profile_id, role) = extract_grpc_profile(&request)?;
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

    async fn chat_completion_stream(
        &self,
        request: Request<proto::ChatCompletionRequest>,
    ) -> Result<Response<Self::ChatCompletionStreamStream>, Status> {
        let (profile_id, role) = extract_grpc_profile(&request)?;
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
        let model = req.model.clone();

        debug!(
            request_id = %request_id,
            profile_id = %profile_id,
            model = %model,
            "gRPC ChatCompletionStream request"
        );

        let messages: Vec<ChatMessage> = req
            .messages
            .iter()
            .map(|m| ChatMessage {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();

        #[allow(clippy::cast_possible_truncation)]
        let estimated_tokens: u32 = messages.iter().map(|m| (m.content.len() / 4) as u32).sum();

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
            streaming: true,
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
            error!(request_id = %request_id, error = %e, "streaming route failed");
            Status::internal("streaming inference routing failed")
        })?;

        let adapter_id = decision.instance_id.to_string();
        let model_id = decision.instance_id.to_string();

        // Create channel for streaming chunks to client
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let rid = request_id.clone();
        let state = self.state.clone();

        // Spawn streaming task
        // TODO(basho): wire real adapter IPC streaming; this task currently
        // sends a single completion chunk.
        tokio::spawn(async move {
            let created = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_secs());

            // Send role chunk then done chunk
            let role_chunk = proto::ChatCompletionChunk {
                id: rid.clone(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model_id.clone(),
                choices: vec![proto::ChunkChoice {
                    index: 0,
                    delta: Some(proto::ChunkDelta {
                        role: "assistant".to_string(),
                        content: String::new(),
                    }),
                    finish_reason: String::new(),
                }],
            };

            if tx.send(Ok(role_chunk)).await.is_err() {
                return;
            }

            // Final chunk with finish_reason
            let done_chunk = proto::ChatCompletionChunk {
                id: rid.clone(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model_id,
                choices: vec![proto::ChunkChoice {
                    index: 0,
                    delta: Some(proto::ChunkDelta {
                        role: String::new(),
                        content: String::new(),
                    }),
                    finish_reason: "stop".to_string(),
                }],
            };

            let _ = tx.send(Ok(done_chunk)).await;

            // Mark request completed
            state
                .scheduler
                .release_sequence(&mai_scheduler::InstanceId::new(&adapter_id), session_id);
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    /// Unary embedding generation.
    async fn embed(
        &self,
        request: Request<proto::EmbeddingRequest>,
    ) -> Result<Response<proto::EmbeddingResponse>, Status> {
        let (profile_id, role) = extract_grpc_profile(&request)?;
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
            inputs = req.input.len(),
            "gRPC Embed request"
        );

        #[allow(clippy::cast_possible_truncation)]
        let estimated_tokens: u32 = req.input.iter().map(|t| (t.len() / 4) as u32).sum();

        let inference_req = InferenceRequest {
            id: request_uuid,
            profile_id: profile_uuid,
            model_name: if req.model.is_empty() {
                None
            } else {
                Some(req.model.clone())
            },
            request_type: RequestType::Embedding,
            payload: RequestPayload::Embedding {
                texts: req.input.clone(),
            },
            priority: RequestPriority::Normal,
            timeout: Duration::from_secs(120),
            streaming: false,
            enqueued_at: Instant::now(),
            estimated_tokens,
        };

        let model_alias = if req.model.is_empty() {
            "default-embedding"
        } else {
            &req.model
        };
        let sched_req = ScheduleRequest::new(model_alias, SchedulerPriority::Normal);
        let session_id = sched_req.session_id;

        let decision = self.state.scheduler.schedule(&sched_req).map_err(|e| {
            error!(request_id = %request_id, error = %e, "embedding route failed");
            Status::internal("embedding request routing failed")
        })?;

        self.state
            .scheduler
            .release_sequence(&decision.instance_id, session_id);

        // TODO(basho): wire actual embedding computation via adapter IPC;
        // currently returns empty embeddings.
        let embeddings: Vec<proto::EmbeddingData> = req
            .input
            .iter()
            .enumerate()
            .map(|(i, _)| proto::EmbeddingData {
                object: "embedding".to_string(),
                embedding: Vec::new(), // Populated by adapter pipeline
                #[allow(clippy::cast_possible_truncation)]
                index: i as u32,
            })
            .collect();

        let response = proto::EmbeddingResponse {
            object: "list".to_string(),
            data: embeddings,
            model: decision.instance_id.to_string(),
            usage: Some(proto::EmbeddingUsage {
                prompt_tokens: estimated_tokens,
                total_tokens: estimated_tokens,
            }),
        };

        Ok(Response::new(response))
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
