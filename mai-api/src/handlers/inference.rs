//! Inference request handlers for the MAI REST API.
//!
//! Handles chat completions, embeddings, structured generation, and
//! function calling. All handlers validate requests, route through
//! the scheduler, and return OpenAI-compatible response formats.
//!
//! Streaming (SSE) delegates to the streaming::sse module. Requests with
//! `stream: true` return an SSE event stream.

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use std::fmt::Write as _;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};
use uuid::Uuid;
use validator::Validate;

use crate::auth::check_permission;
use crate::errors::ApiError;
use crate::state::AppState;
use crate::types::{
    ApiChatMessage, ChatChoice, ChatCompletionRequest, ChatCompletionResponse, EmbeddingData,
    EmbeddingInput, EmbeddingRequest, EmbeddingResponse, EmbeddingUsage, FunctionCallRequest,
    ProfileInfo, StructuredGenerationRequest, TokenUsage,
};

use mai_core::scheduler::{InferenceRequest, RequestPayload, RequestPriority, RequestType};
use mai_hil::traits::GenerationParams;
use mai_scheduler::{Priority as SchedulerPriority, ScheduleRequest};

// ─── Chat Completions ──────────────────────────────────────────────

/// POST /v1/chat/completions
///
/// OpenAI-compatible chat completion endpoint. Validates the request,
/// checks profile permissions, routes through the scheduler, and
/// returns a ChatCompletionResponse.
///
/// If `stream: true`, delegates to SSE streaming handler.
#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
pub async fn chat_completions(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Json(req): Json<ChatCompletionRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Streaming via SSE
    if req.stream {
        let last_event_id = None; // Extracted from headers in full integration (11e)
        return crate::streaming::sse::handle_sse_chat(state, profile, req, last_event_id)
            .await
            .map(axum::response::IntoResponse::into_response);
    }

    // Permission check
    check_permission(&profile, "inference")?;

    // Validate request
    req.validate()
        .map_err(|e| ApiError::ValidationFailed(format!("Invalid chat completion request: {e}")))?;
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

    // Route through new scheduler
    let sched_priority = scheduler_priority_from_profile(&profile);
    let sched_req =
        ScheduleRequest::new(model_name.as_deref().unwrap_or("default"), sched_priority);
    let session_id = sched_req.session_id;

    let decision = state.scheduler.schedule(&sched_req).map_err(|e| {
        warn!(error = %e, "Scheduler routing failed");
        match e {
            mai_scheduler::SchedulerError::NoInstanceAvailable(_) => {
                ApiError::ModelUnavailable(model_name.unwrap_or_else(|| "default".to_string()))
            }
            mai_scheduler::SchedulerError::SystemOverloaded(_, _) => ApiError::SystemOverloaded,
            _ => ApiError::InternalError,
        }
    })?;

    // Extract adapter name from instance_id (format: "adapter:model")
    let adapter_name = decision
        .instance_id
        .as_str()
        .split(':')
        .next()
        .unwrap_or(decision.instance_id.as_str())
        .to_string();

    // Build prompt from chat messages (concatenated for adapter consumption)
    let prompt = build_chat_prompt(&req.messages);

    // Build generation params from request
    let gen_params = build_generation_params(&req);

    // Route to the real adapter via AdapterManager
    let tokens = {
        let mgr = state.adapter_manager.lock().await;
        mgr.generate(&adapter_name, prompt, gen_params)
            .await
            .map_err(|e| {
                error!(error = %e, adapter = %adapter_name, "Adapter generate failed");
                match e {
                    mai_adapters::errors::FrameworkError::ProcessCrashed { .. } => {
                        ApiError::AdapterCrashed(adapter_name.clone())
                    }
                    mai_adapters::errors::FrameworkError::ResponseTimeout { .. } => {
                        ApiError::RequestTimeout
                    }
                    _ => ApiError::InternalError,
                }
            })?
    };

    // Collect generated text from tokens
    let generated_text: String = tokens.iter().map(|t| t.text.as_str()).collect();
    let completion_tokens = tokens.len() as u32;

    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());

    let response = ChatCompletionResponse {
        id: format!("chatcmpl-{request_id}"),
        object: "chat.completion".to_string(),
        created: now_epoch,
        model: decision.instance_id.to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: ApiChatMessage {
                role: "assistant".to_string(),
                content: generated_text,
                name: None,
            },
            finish_reason: Some("stop".to_string()),
        }],
        usage: TokenUsage {
            prompt_tokens: inference_req.estimated_tokens,
            completion_tokens,
            total_tokens: inference_req.estimated_tokens + completion_tokens,
        },
    };

    // Release sequence in scheduler
    state
        .scheduler
        .release_sequence(&decision.instance_id, session_id);

    info!(
        request_id = %request_id,
        instance = %decision.instance_id,
        profile = %profile.profile_id,
        tokens = completion_tokens,
        "Chat completion served"
    );

    Ok(Json(response).into_response())
}

// ─── Embeddings ────────────────────────────────────────────────────

/// POST /v1/embeddings
///
/// OpenAI-compatible embedding endpoint. Routes to an embedding-capable
/// adapter and returns vector representations.
#[allow(clippy::cast_possible_truncation)]
pub async fn embeddings(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Json(req): Json<EmbeddingRequest>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "inference")?;

    req.validate()
        .map_err(|e| ApiError::ValidationFailed(format!("Invalid embedding request: {e}")))?;

    // Convert validated input into a flat text list
    let texts = match &req.input {
        EmbeddingInput::Single(s) => vec![s.clone()],
        EmbeddingInput::Batch(v) => v.clone(),
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
        estimated_tokens: texts.iter().map(|t| estimate_text_tokens(t.as_str())).sum(),
    };

    let sched_priority = scheduler_priority_from_profile(&profile);
    let sched_req = ScheduleRequest::new(
        model_name.as_deref().unwrap_or("default-embedding"),
        sched_priority,
    );
    let session_id = sched_req.session_id;

    let decision = state.scheduler.schedule(&sched_req).map_err(|e| {
        warn!(error = %e, "Scheduler routing failed for embedding");
        ApiError::ModelUnavailable(model_name.unwrap_or_else(|| "default-embedding".to_string()))
    })?;

    let adapter_name = decision
        .instance_id
        .as_str()
        .split(':')
        .next()
        .unwrap_or(decision.instance_id.as_str())
        .to_string();

    // Route to real adapter for embedding
    let embeddings = {
        let mgr = state.adapter_manager.lock().await;
        mgr.embed(&adapter_name, texts.clone()).await.map_err(|e| {
            error!(error = %e, adapter = %adapter_name, "Adapter embed failed");
            match e {
                mai_adapters::errors::FrameworkError::ProcessCrashed { .. } => {
                    ApiError::AdapterCrashed(adapter_name.clone())
                }
                mai_adapters::errors::FrameworkError::ResponseTimeout { .. } => {
                    ApiError::RequestTimeout
                }
                _ => ApiError::InternalError,
            }
        })?
    };

    let data: Vec<EmbeddingData> = embeddings
        .iter()
        .enumerate()
        .map(|(i, emb)| EmbeddingData {
            object: "embedding".to_string(),
            embedding: emb.vector.clone(),
            index: i as u32,
        })
        .collect();

    let response = EmbeddingResponse {
        object: "list".to_string(),
        data,
        model: decision.instance_id.to_string(),
        usage: EmbeddingUsage {
            prompt_tokens: inference_req.estimated_tokens,
            total_tokens: inference_req.estimated_tokens,
        },
    };

    state
        .scheduler
        .release_sequence(&decision.instance_id, session_id);

    info!(
        request_id = %request_id,
        instance = %decision.instance_id,
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
#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
pub async fn structured_generation(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Json(req): Json<StructuredGenerationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "inference")?;

    req.validate()
        .map_err(|e| ApiError::ValidationFailed(e.to_string()))?;

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
            req.response_format.format_type,
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

    let sched_priority = scheduler_priority_from_profile(&profile);
    let sched_req =
        ScheduleRequest::new(model_name.as_deref().unwrap_or("default"), sched_priority);
    let session_id = sched_req.session_id;

    let decision = state.scheduler.schedule(&sched_req).map_err(|e| {
        warn!(error = %e, "Scheduler routing failed for structured generation");
        ApiError::ModelIncompatible(
            "No adapter supports structured output for this model".to_string(),
        )
    })?;

    let adapter_name = decision
        .instance_id
        .as_str()
        .split(':')
        .next()
        .unwrap_or(decision.instance_id.as_str())
        .to_string();
    let prompt = build_chat_prompt(&req.messages);
    let gen_params = build_structured_gen_params(&req);

    let tokens = {
        let mgr = state.adapter_manager.lock().await;
        mgr.generate(&adapter_name, prompt, gen_params)
            .await
            .map_err(|e| {
                error!(error = %e, adapter = %adapter_name, "Adapter structured gen failed");
                match e {
                    mai_adapters::errors::FrameworkError::ProcessCrashed { .. } => {
                        ApiError::AdapterCrashed(adapter_name.clone())
                    }
                    mai_adapters::errors::FrameworkError::ResponseTimeout { .. } => {
                        ApiError::RequestTimeout
                    }
                    _ => ApiError::InternalError,
                }
            })?
    };

    let generated_text: String = tokens.iter().map(|t| t.text.as_str()).collect();
    let completion_tokens = tokens.len() as u32;

    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());

    let response = ChatCompletionResponse {
        id: format!("structcmpl-{request_id}"),
        object: "chat.completion".to_string(),
        created: now_epoch,
        model: decision.instance_id.to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: ApiChatMessage {
                role: "assistant".to_string(),
                content: generated_text,
                name: None,
            },
            finish_reason: Some("stop".to_string()),
        }],
        usage: TokenUsage {
            prompt_tokens: inference_req.estimated_tokens,
            completion_tokens,
            total_tokens: inference_req.estimated_tokens + completion_tokens,
        },
    };

    state
        .scheduler
        .release_sequence(&decision.instance_id, session_id);

    Ok(Json(response).into_response())
}

// ─── Function Calling ──────────────────────────────────────────────

/// POST /v1/generate/function_call
///
/// Tool calling / function calling protocol. Supports multi-step
/// chains where the model selects tools and the caller provides
/// tool results in subsequent messages.
#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
pub async fn function_call(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Json(req): Json<FunctionCallRequest>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "inference")?;

    req.validate()
        .map_err(|e| ApiError::ValidationFailed(format!("Invalid function_call request: {e}")))?;

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

    let sched_priority = scheduler_priority_from_profile(&profile);
    let sched_req =
        ScheduleRequest::new(model_name.as_deref().unwrap_or("default"), sched_priority);
    let session_id = sched_req.session_id;

    let decision = state.scheduler.schedule(&sched_req).map_err(|e| {
        warn!(error = %e, "Scheduler routing failed for function_call");
        ApiError::ModelIncompatible(
            "No adapter supports function calling for this model".to_string(),
        )
    })?;

    let adapter_name = decision
        .instance_id
        .as_str()
        .split(':')
        .next()
        .unwrap_or(decision.instance_id.as_str())
        .to_string();

    // Build prompt with tool definitions injected as system context
    let mut prompt = String::from("Available tools:\n");
    for tool in &req.tools {
        let _ = writeln!(
            prompt,
            "- {} ({}): {}",
            tool.function.name,
            tool.tool_type,
            tool.function.description.as_deref().unwrap_or(""),
        );
    }
    prompt.push('\n');
    prompt.push_str(&build_chat_prompt(&req.messages));

    let gen_params = GenerationParams {
        temperature: 0.0,
        top_p: 1.0,
        max_tokens: 4096,
        stop_sequences: Vec::new(),
        structured_schema: None,
    };

    let tokens = {
        let mgr = state.adapter_manager.lock().await;
        mgr.generate(&adapter_name, prompt, gen_params)
            .await
            .map_err(|e| {
                error!(error = %e, adapter = %adapter_name, "Adapter function_call gen failed");
                match e {
                    mai_adapters::errors::FrameworkError::ProcessCrashed { .. } => {
                        ApiError::AdapterCrashed(adapter_name.clone())
                    }
                    mai_adapters::errors::FrameworkError::ResponseTimeout { .. } => {
                        ApiError::RequestTimeout
                    }
                    _ => ApiError::InternalError,
                }
            })?
    };

    let generated_text: String = tokens.iter().map(|t| t.text.as_str()).collect();
    let completion_tokens = tokens.len() as u32;

    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());

    let response = ChatCompletionResponse {
        id: format!("fncall-{request_id}"),
        object: "chat.completion".to_string(),
        created: now_epoch,
        model: decision.instance_id.to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: ApiChatMessage {
                role: "assistant".to_string(),
                content: generated_text,
                name: None,
            },
            finish_reason: Some("tool_calls".to_string()),
        }],
        usage: TokenUsage {
            prompt_tokens: inference_req.estimated_tokens,
            completion_tokens,
            total_tokens: inference_req.estimated_tokens + completion_tokens,
        },
    };

    state
        .scheduler
        .release_sequence(&decision.instance_id, session_id);

    Ok(Json(response).into_response())
}

// ─── Prompt / Param Builders ──────────────────────────────────────

/// Build a single prompt string from chat messages for adapter consumption.
/// Uses a simple role-tagged format that adapters can parse or pass through.
fn build_chat_prompt(messages: &[ApiChatMessage]) -> String {
    let mut prompt = String::new();
    for msg in messages {
        prompt.push_str(&msg.role);
        prompt.push_str(": ");
        prompt.push_str(&msg.content);
        prompt.push('\n');
    }
    prompt
}

/// Map ChatCompletionRequest params into the HIL GenerationParams struct.
#[allow(clippy::manual_unwrap_or_default)]
fn explicit_stop_sequences_or_empty(stop: &Option<Vec<String>>) -> Vec<String> {
    match stop.clone() {
        Some(stop) => stop,
        None => Vec::new(),
    }
}

#[allow(clippy::cast_possible_truncation)]
fn build_generation_params(req: &ChatCompletionRequest) -> GenerationParams {
    GenerationParams {
        temperature: req.temperature.unwrap_or(0.7),
        top_p: req.top_p.unwrap_or(1.0),
        max_tokens: req.max_tokens.map_or(2048, |v| v as usize),
        stop_sequences: explicit_stop_sequences_or_empty(&req.stop),
        structured_schema: None,
    }
}

/// Build GenerationParams with a JSON schema constraint for structured output.
#[allow(clippy::cast_possible_truncation)]
fn build_structured_gen_params(req: &StructuredGenerationRequest) -> GenerationParams {
    GenerationParams {
        temperature: req.temperature.unwrap_or(0.0),
        top_p: 1.0,
        max_tokens: req.max_tokens.map_or(4096, |v| v as usize),
        stop_sequences: Vec::new(),
        structured_schema: req.response_format.json_schema.clone(),
    }
}

// ─── Validation Helpers ────────────────────────────────────────────

fn validate_chat_request(req: &ChatCompletionRequest) -> Result<(), ApiError> {
    req.validate()
        .map_err(|e| ApiError::ValidationFailed(e.to_string()))?;

    if req.messages.is_empty() {
        return Err(ApiError::ValidationFailed(
            "Messages array cannot be empty".to_string(),
        ));
    }

    if let Some(temp) = req.temperature
        && !(0.0..=2.0).contains(&temp)
    {
        return Err(ApiError::ValidationFailed(format!(
            "Temperature must be between 0.0 and 2.0, got {temp}"
        )));
    }

    if let Some(top_p) = req.top_p
        && !(0.0..=1.0).contains(&top_p)
    {
        return Err(ApiError::ValidationFailed(format!(
            "top_p must be between 0.0 and 1.0, got {top_p}"
        )));
    }

    if let Some(fp) = req.frequency_penalty
        && !(-2.0..=2.0).contains(&fp)
    {
        return Err(ApiError::ValidationFailed(format!(
            "frequency_penalty must be between -2.0 and 2.0, got {fp}"
        )));
    }

    if let Some(pp) = req.presence_penalty
        && !(-2.0..=2.0).contains(&pp)
    {
        return Err(ApiError::ValidationFailed(format!(
            "presence_penalty must be between -2.0 and 2.0, got {pp}"
        )));
    }

    Ok(())
}
// ─── Token Estimation ──────────────────────────────────────────────

/// Rough token estimate for chat requests (4 chars per token heuristic).
/// This is used for complexity assessment and Sentinel promotion, not billing.
#[allow(clippy::cast_possible_truncation)]
fn estimate_chat_tokens(req: &ChatCompletionRequest) -> u32 {
    let char_count: usize = req
        .messages
        .iter()
        .map(|m| m.content.len() + m.role.len())
        .sum();
    (char_count / 4).max(1) as u32
}

#[allow(clippy::cast_possible_truncation)]
fn estimate_messages_tokens(messages: &[ApiChatMessage]) -> u32 {
    let char_count: usize = messages
        .iter()
        .map(|m| m.content.len() + m.role.len())
        .sum();
    (char_count / 4).max(1) as u32
}

#[allow(clippy::cast_possible_truncation)]
fn estimate_text_tokens(text: &str) -> u32 {
    (text.len() / 4).max(1) as u32
}

// ─── Profile-to-Priority Mapping ───────────────────────────────────

/// Map a profile role to request priority (legacy mai-core type).
/// Admin/Adult get Normal, Teen/Child get Normal, Guest gets Low.
fn priority_from_profile(profile: &ProfileInfo) -> RequestPriority {
    use crate::types::ProfileRole;
    match profile.role {
        ProfileRole::Admin => RequestPriority::High,
        ProfileRole::Adult | ProfileRole::Teen | ProfileRole::Child => RequestPriority::Normal,
        ProfileRole::Guest => RequestPriority::Low,
    }
}

/// Map a profile role to the new scheduler priority.
fn scheduler_priority_from_profile(profile: &ProfileInfo) -> SchedulerPriority {
    use crate::types::ProfileRole;
    match profile.role {
        ProfileRole::Admin => SchedulerPriority::High,
        ProfileRole::Adult | ProfileRole::Teen | ProfileRole::Child => SchedulerPriority::Normal,
        ProfileRole::Guest => SchedulerPriority::Background,
    }
}
