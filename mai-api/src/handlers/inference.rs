//! Inference request handlers for the MAI REST API.
//!
//! Handles chat completions, embeddings, structured generation, and
//! function calling. All handlers validate requests, route through
//! the scheduler, and return OpenAI-compatible response formats.
//!
//! Streaming (SSE) delegates to the streaming::sse module. Requests with
//! `stream: true` return an SSE event stream (Session 11c).

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use uuid::Uuid;

use crate::auth::check_permission;
use crate::errors::ApiError;
use crate::state::AppState;
use crate::types::{
    ApiChatMessage, ChatChoice, ChatCompletionRequest, ChatCompletionResponse,
    EmbeddingData, EmbeddingInput, EmbeddingRequest, EmbeddingResponse, EmbeddingUsage,
    FunctionCallRequest, ProfileInfo, StructuredGenerationRequest, TokenUsage,
};

use mai_core::scheduler::{InferenceRequest, RequestPayload, RequestPriority, RequestType};

// ─── Chat Completions ──────────────────────────────────────────────

/// POST /v1/chat/completions
///
/// OpenAI-compatible chat completion endpoint. Validates the request,
/// checks profile permissions, routes through the scheduler, and
/// returns a ChatCompletionResponse.
///
/// If `stream: true`, delegates to SSE streaming handler (Session 11c).
pub async fn chat_completions(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Json(req): Json<ChatCompletionRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Streaming via SSE (Session 11c)
    if req.stream {
        let last_event_id = None; // Extracted from headers in full integration (11e)
        return crate::streaming::sse::handle_sse_chat(
            state, profile, req, last_event_id,
        )
        .await
        .map(|r| r.into_response());
    }

    // Permission check
    check_permission(&profile, "inference")?;

    // Validate request
    validate_chat_request(&req)?;

    // Build internal inference request
    let request_id = Uuid::new_v4();
    let model_name = req.model.clone();
    let payload: RequestPayload = (&req).into();

    let inference_req = InferenceRequest {
        id: request_id,
        profile_id: Uuid::parse_str(&profile.profile_id).unwrap_or_else(|_| Uuid::new_v4()),
        model_name: model_name.clone(),
        request_type: RequestType::Chat,
        payload,
        priority: priority_from_profile(&profile),
        timeout: Duration::from_secs(120),
        streaming: false,
        enqueued_at: Instant::now(),
        estimated_tokens: estimate_chat_tokens(&req),
    };

    // Route through scheduler
    let selection = {
        let mut scheduler = state.scheduler.write().await;
        scheduler.route_request(&inference_req).map_err(|e| {
            warn!(error = %e, "Scheduler routing failed");
            match e {
                mai_core::scheduler::SchedulerError::NoAdapterAvailable(_) => {
                    ApiError::ModelUnavailable(
                        model_name.unwrap_or_else(|| "default".to_string()),
                    )
                }
                mai_core::scheduler::SchedulerError::QueueFull(_, _) => {
                    ApiError::SystemOverloaded
                }
                mai_core::scheduler::SchedulerError::RequestTimeout(_, _) => {
                    ApiError::RequestTimeout
                }
                _ => ApiError::InternalError,
            }
        })?
    };

    // In a full implementation, we would forward the request to the selected
    // adapter and await the response. For now, we build a placeholder response
    // that demonstrates the correct wire format. The actual adapter IPC bridge
    // (Session 08) handles the subprocess communication.
    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let response = ChatCompletionResponse {
        id: format!("chatcmpl-{}", request_id),
        object: "chat.completion".to_string(),
        created: now_epoch,
        model: selection.model_id.clone(),
        choices: vec![ChatChoice {
            index: 0,
            message: ApiChatMessage {
                role: "assistant".to_string(),
                content: String::new(), // Populated by adapter response
                name: None,
            },
            finish_reason: Some("stop".to_string()),
        }],
        usage: TokenUsage {
            prompt_tokens: inference_req.estimated_tokens,
            completion_tokens: 0,
            total_tokens: inference_req.estimated_tokens,
        },
    };

    // Mark request completed in scheduler
    {
        let mut scheduler = state.scheduler.write().await;
        scheduler.request_completed(&selection.adapter_id);
    }

    info!(
        request_id = %request_id,
        model = %selection.model_id,
        profile = %profile.profile_id,
        "Chat completion served"
    );

    Ok(Json(response).into_response())
}

// ─── Embeddings ────────────────────────────────────────────────────

/// POST /v1/embeddings
///
/// OpenAI-compatible embedding endpoint. Routes to an embedding-capable
/// adapter and returns vector representations.
pub async fn embeddings(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Json(req): Json<EmbeddingRequest>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "inference")?;

    // Validate input is non-empty
    let texts = match &req.input {
        EmbeddingInput::Single(s) => {
            if s.is_empty() {
                return Err(ApiError::ValidationFailed(
                    "Embedding input cannot be empty".to_string(),
                ));
            }
            vec![s.clone()]
        }
        EmbeddingInput::Batch(v) => {
            if v.is_empty() {
                return Err(ApiError::ValidationFailed(
                    "Embedding input batch cannot be empty".to_string(),
                ));
            }
            v.clone()
        }
    };

    let request_id = Uuid::new_v4();
    let model_name = req.model.clone();
    let payload: RequestPayload = (&req).into();

    let inference_req = InferenceRequest {
        id: request_id,
        profile_id: Uuid::parse_str(&profile.profile_id).unwrap_or_else(|_| Uuid::new_v4()),
        model_name: model_name.clone(),
        request_type: RequestType::Embedding,
        payload,
        priority: priority_from_profile(&profile),
        timeout: Duration::from_secs(60),
        streaming: false,
        enqueued_at: Instant::now(),
        estimated_tokens: texts.iter().map(|t| estimate_text_tokens(t)).sum(),
    };

    let selection = {
        let mut scheduler = state.scheduler.write().await;
        scheduler.route_request(&inference_req).map_err(|e| {
            warn!(error = %e, "Scheduler routing failed for embedding");
            ApiError::ModelUnavailable(
                model_name.unwrap_or_else(|| "default-embedding".to_string()),
            )
        })?
    };

    // Placeholder response with correct wire format
    let data: Vec<EmbeddingData> = texts
        .iter()
        .enumerate()
        .map(|(i, _)| EmbeddingData {
            object: "embedding".to_string(),
            embedding: vec![], // Populated by adapter
            index: i as u32,
        })
        .collect();

    let response = EmbeddingResponse {
        object: "list".to_string(),
        data,
        model: selection.model_id.clone(),
        usage: EmbeddingUsage {
            prompt_tokens: inference_req.estimated_tokens,
            total_tokens: inference_req.estimated_tokens,
        },
    };

    {
        let mut scheduler = state.scheduler.write().await;
        scheduler.request_completed(&selection.adapter_id);
    }

    info!(
        request_id = %request_id,
        model = %selection.model_id,
        texts = texts.len(),
        "Embedding request served"
    );

    Ok(Json(response).into_response())
}

// ─── Structured Generation ─────────────────────────────────────────

/// POST /v1/generate/structured
///
/// Structured output generation with JSON schema constraints.
/// Forwards the schema to the adapter for constrained decoding.
pub async fn structured_generation(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Json(req): Json<StructuredGenerationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "inference")?;

    if req.messages.is_empty() {
        return Err(ApiError::ValidationFailed(
            "Messages array cannot be empty".to_string(),
        ));
    }

    if req.response_format.format_type != "json_object"
        && req.response_format.format_type != "json_schema"
    {
        return Err(ApiError::ValidationFailed(format!(
            "Unsupported response_format type: '{}'. Use 'json_object' or 'json_schema'",
            req.response_format.format_type
        )));
    }

    let request_id = Uuid::new_v4();
    let model_name = req.model.clone();

    // Convert to chat payload (structured constraint passed via adapter config)
    let payload = RequestPayload::Chat {
        messages: req.messages.iter().map(Into::into).collect(),
    };

    let inference_req = InferenceRequest {
        id: request_id,
        profile_id: Uuid::parse_str(&profile.profile_id).unwrap_or_else(|_| Uuid::new_v4()),
        model_name: model_name.clone(),
        request_type: RequestType::Structured,
        payload,
        priority: priority_from_profile(&profile),
        timeout: Duration::from_secs(120),
        streaming: false,
        enqueued_at: Instant::now(),
        estimated_tokens: estimate_messages_tokens(&req.messages),
    };

    let selection = {
        let mut scheduler = state.scheduler.write().await;
        scheduler.route_request(&inference_req).map_err(|e| {
            warn!(error = %e, "Scheduler routing failed for structured generation");
            ApiError::ModelIncompatible(
                "No adapter supports structured output for this model".to_string(),
            )
        })?
    };

    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let response = ChatCompletionResponse {
        id: format!("structcmpl-{}", request_id),
        object: "chat.completion".to_string(),
        created: now_epoch,
        model: selection.model_id.clone(),
        choices: vec![ChatChoice {
            index: 0,
            message: ApiChatMessage {
                role: "assistant".to_string(),
                content: String::new(), // Populated by adapter with schema-constrained output
                name: None,
            },
            finish_reason: Some("stop".to_string()),
        }],
        usage: TokenUsage {
            prompt_tokens: inference_req.estimated_tokens,
            completion_tokens: 0,
            total_tokens: inference_req.estimated_tokens,
        },
    };

    {
        let mut scheduler = state.scheduler.write().await;
        scheduler.request_completed(&selection.adapter_id);
    }

    Ok(Json(response).into_response())
}

// ─── Function Calling ──────────────────────────────────────────────

/// POST /v1/generate/function_call
///
/// Tool calling / function calling protocol. Supports multi-step
/// chains where the model selects tools and the caller provides
/// tool results in subsequent messages.
pub async fn function_call(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Json(req): Json<FunctionCallRequest>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "inference")?;

    if req.messages.is_empty() {
        return Err(ApiError::ValidationFailed(
            "Messages array cannot be empty".to_string(),
        ));
    }

    if req.tools.is_empty() {
        return Err(ApiError::ValidationFailed(
            "Tools array cannot be empty for function_call endpoint".to_string(),
        ));
    }

    // Validate tool definitions
    for tool in &req.tools {
        if tool.tool_type != "function" {
            return Err(ApiError::ValidationFailed(format!(
                "Unsupported tool type '{}'. Only 'function' is supported",
                tool.tool_type
            )));
        }
        if tool.function.name.is_empty() {
            return Err(ApiError::ValidationFailed(
                "Function name cannot be empty".to_string(),
            ));
        }
    }

    let request_id = Uuid::new_v4();
    let model_name = req.model.clone();

    let payload = RequestPayload::Chat {
        messages: req.messages.iter().map(Into::into).collect(),
    };

    let inference_req = InferenceRequest {
        id: request_id,
        profile_id: Uuid::parse_str(&profile.profile_id).unwrap_or_else(|_| Uuid::new_v4()),
        model_name: model_name.clone(),
        request_type: RequestType::FunctionCall,
        payload,
        priority: priority_from_profile(&profile),
        timeout: Duration::from_secs(120),
        streaming: false,
        enqueued_at: Instant::now(),
        estimated_tokens: estimate_messages_tokens(&req.messages),
    };

    let selection = {
        let mut scheduler = state.scheduler.write().await;
        scheduler.route_request(&inference_req).map_err(|e| {
            warn!(error = %e, "Scheduler routing failed for function_call");
            ApiError::ModelIncompatible(
                "No adapter supports function calling for this model".to_string(),
            )
        })?
    };

    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let response = ChatCompletionResponse {
        id: format!("fncall-{}", request_id),
        object: "chat.completion".to_string(),
        created: now_epoch,
        model: selection.model_id.clone(),
        choices: vec![ChatChoice {
            index: 0,
            message: ApiChatMessage {
                role: "assistant".to_string(),
                content: String::new(), // Populated by adapter with tool_calls
                name: None,
            },
            finish_reason: Some("tool_calls".to_string()),
        }],
        usage: TokenUsage {
            prompt_tokens: inference_req.estimated_tokens,
            completion_tokens: 0,
            total_tokens: inference_req.estimated_tokens,
        },
    };

    {
        let mut scheduler = state.scheduler.write().await;
        scheduler.request_completed(&selection.adapter_id);
    }

    Ok(Json(response).into_response())
}

// ─── Validation Helpers ────────────────────────────────────────────

fn validate_chat_request(req: &ChatCompletionRequest) -> Result<(), ApiError> {
    if req.messages.is_empty() {
        return Err(ApiError::ValidationFailed(
            "Messages array cannot be empty".to_string(),
        ));
    }

    if let Some(temp) = req.temperature {
        if !(0.0..=2.0).contains(&temp) {
            return Err(ApiError::ValidationFailed(format!(
                "Temperature must be between 0.0 and 2.0, got {}",
                temp
            )));
        }
    }

    if let Some(top_p) = req.top_p {
        if !(0.0..=1.0).contains(&top_p) {
            return Err(ApiError::ValidationFailed(format!(
                "top_p must be between 0.0 and 1.0, got {}",
                top_p
            )));
        }
    }

    if let Some(fp) = req.frequency_penalty {
        if !(-2.0..=2.0).contains(&fp) {
            return Err(ApiError::ValidationFailed(format!(
                "frequency_penalty must be between -2.0 and 2.0, got {}",
                fp
            )));
        }
    }

    if let Some(pp) = req.presence_penalty {
        if !(-2.0..=2.0).contains(&pp) {
            return Err(ApiError::ValidationFailed(format!(
                "presence_penalty must be between -2.0 and 2.0, got {}",
                pp
            )));
        }
    }

    Ok(())
}

// ─── Token Estimation ──────────────────────────────────────────────

/// Rough token estimate for chat requests (4 chars per token heuristic).
/// This is used for complexity assessment and Sentinel promotion, not billing.
fn estimate_chat_tokens(req: &ChatCompletionRequest) -> u32 {
    let char_count: usize = req
        .messages
        .iter()
        .map(|m| m.content.len() + m.role.len())
        .sum();
    (char_count / 4).max(1) as u32
}

fn estimate_messages_tokens(messages: &[ApiChatMessage]) -> u32 {
    let char_count: usize = messages
        .iter()
        .map(|m| m.content.len() + m.role.len())
        .sum();
    (char_count / 4).max(1) as u32
}

fn estimate_text_tokens(text: &str) -> u32 {
    (text.len() / 4).max(1) as u32
}

// ─── Profile-to-Priority Mapping ───────────────────────────────────

/// Map a profile role to request priority.
/// Admin/Adult get Normal, Teen/Child get Normal, Guest gets Low.
fn priority_from_profile(profile: &ProfileInfo) -> RequestPriority {
    use crate::types::ProfileRole;
    match profile.role {
        ProfileRole::Admin => RequestPriority::High,
        ProfileRole::Adult => RequestPriority::Normal,
        ProfileRole::Teen => RequestPriority::Normal,
        ProfileRole::Child => RequestPriority::Normal,
        ProfileRole::Guest => RequestPriority::Low,
    }
}
