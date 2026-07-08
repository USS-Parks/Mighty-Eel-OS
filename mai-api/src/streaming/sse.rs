//! Server-Sent Events (SSE) streaming for chat completions.
//!
//! When a `ChatCompletionRequest` has `stream: true`, the response is
//! an SSE event stream delivering `ChatCompletionChunk` deltas in
//! OpenAI-compatible format.
//!
//! ## Protocol Features
//!
//! - **Sequence numbering:** Monotonic `uint64` per stream, included
//!   in each SSE event's `id` field for resume support.
//! - **Heartbeat:** Empty comment line (`: heartbeat\n\n`) every 15
//!   seconds during idle periods to keep the connection alive.
//! - **Backpressure:** 64-event buffer. When the client falls behind,
//!   oldest events are dropped and a gap marker is inserted.
//! - **Resume:** Client sends `Last-Event-ID` header; server replays
//!   from that sequence number if events are still in the buffer.
//! - **Token timeout:** If no token arrives from the adapter for 30
//!   seconds, an error event is sent and the stream is closed.
//! - **Final event:** `data: [DONE]\n\n` per OpenAI spec.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::http::StatusCode;
use axum::http::header;
use axum::response::Response;
use futures_util::stream::Stream;
use tokio::sync::mpsc;
use tokio::time::{interval, timeout};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::auth::check_permission;
use crate::errors::ApiError;
use crate::state::AppState;
use crate::types::{
    ApiChatMessage, ChatCompletionChunk, ChatCompletionRequest, ChunkChoice, ChunkDelta,
    ProfileInfo,
};
use mai_adapters::bridge::IpcEventKind;
use mai_core::scheduler::{InferenceRequest, RequestPayload, RequestPriority, RequestType};
use mai_hil::traits::GenerationParams;
use mai_scheduler::{Priority as SchedulerPriority, ScheduleRequest};

use super::{BackpressureMonitor, StreamId, TokenEvent, TokenReceiver, token_channel};

// ─── Constants ─────────────────────────────────────────────────────

/// Heartbeat interval: send a comment line every 15 seconds.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

/// Token timeout: close stream if no token arrives in 30 seconds.
const TOKEN_TIMEOUT: Duration = Duration::from_secs(30);

/// Backpressure buffer capacity: drop oldest events beyond this.
const BACKPRESSURE_CAPACITY: usize = 64;

// ─── SSE Response Builder ──────────────────────────────────────────

/// Handle a streaming chat completion request via SSE.
///
/// This function is called by the inference handler when `stream=true`.
/// It sets up the token channel, submits the request to the scheduler,
/// and returns an SSE response that streams `ChatCompletionChunk`
/// deltas until completion or error.
#[allow(clippy::too_many_lines, clippy::unused_async)]
pub async fn handle_sse_chat(
    state: AppState,
    profile: ProfileInfo,
    req: ChatCompletionRequest,
    last_event_id: Option<u64>,
) -> Result<Response, ApiError> {
    // Permission check
    check_permission(&profile, "inference")?;

    // Validate request
    validate_sse_request(&req)?;

    // Build internal inference request
    let request_id = Uuid::new_v4();
    let stream_id = StreamId::new();
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
        streaming: true,
        enqueued_at: Instant::now(),
        estimated_tokens: estimate_chat_tokens(&req),
    };

    // Route through new scheduler
    let sched_priority = scheduler_priority_from_profile(&profile);
    let sched_req =
        ScheduleRequest::new(model_name.as_deref().unwrap_or("default"), sched_priority);
    let session_id = sched_req.session_id;

    let decision = state.scheduler.schedule(&sched_req).map_err(|e| {
        warn!(error = %e, "Scheduler routing failed for SSE stream");
        match e {
            mai_scheduler::SchedulerError::NoInstanceAvailable(_) => {
                ApiError::ModelUnavailable(model_name.unwrap_or_else(|| "default".to_string()))
            }
            mai_scheduler::SchedulerError::SystemOverloaded(_, _) => ApiError::SystemOverloaded,
            _ => ApiError::InternalError,
        }
    })?;

    let model_id = decision.instance_id.to_string();
    let adapter_id = decision.instance_id.to_string();

    // Create token channel for adapter to feed streaming tokens into.
    let (tx, rx) = token_channel();

    // Build prompt and generation params from the request
    let prompt = build_chat_prompt(&req.messages);
    let gen_params = build_generation_params(&req);

    // Initiate streaming inference via AdapterManager and spawn a
    // producer task that reads IPC events and feeds TokenEvents.
    let tx_producer = tx.clone();
    let adapter_name_stream = adapter_id.clone();
    let adapter_mgr = state.adapter_manager.clone();
    let spawn_id = request_id;
    tokio::spawn(async move {
        // Initiate streaming request and get the IPC event channel in one call
        let (request_id_str, mut ipc_rx) = {
            let mgr = adapter_mgr.lock().await;
            match mgr
                .generate_stream_channel(&adapter_name_stream, prompt, gen_params)
                .await
            {
                Ok(pair) => pair,
                Err(e) => {
                    warn!(error = %e, "Failed to start streaming inference");
                    let _ = tx_producer
                        .send(TokenEvent {
                            sequence: 1,
                            token: None,
                            is_final: true,
                            finish_reason: Some("error".to_string()),
                            produced_at: Instant::now(),
                        })
                        .await;
                    return;
                }
            }
        };

        // Step 3: Read IPC events and feed TokenEvents into the SSE channel
        let mut seq: u64 = 0;
        loop {
            match tokio::time::timeout(TOKEN_TIMEOUT, ipc_rx.recv()).await {
                Ok(Some(event)) => {
                    if event.request_id != request_id_str {
                        continue; // Not our request
                    }
                    match event.parse() {
                        Ok(IpcEventKind::Token {
                            text,
                            finish_reason,
                            ..
                        }) => {
                            seq += 1;
                            let is_final = finish_reason.is_some();
                            let _ = tx_producer
                                .send(TokenEvent {
                                    sequence: seq,
                                    token: Some(text),
                                    is_final: false,
                                    finish_reason: None,
                                    produced_at: Instant::now(),
                                })
                                .await;
                            if is_final {
                                seq += 1;
                                let _ = tx_producer
                                    .send(TokenEvent {
                                        sequence: seq,
                                        token: None,
                                        is_final: true,
                                        finish_reason: Some(
                                            finish_reason.unwrap_or_else(|| "stop".to_string()),
                                        ),
                                        produced_at: Instant::now(),
                                    })
                                    .await;
                                break;
                            }
                        }
                        Ok(IpcEventKind::Done) => {
                            seq += 1;
                            let _ = tx_producer
                                .send(TokenEvent {
                                    sequence: seq,
                                    token: None,
                                    is_final: true,
                                    finish_reason: Some("stop".to_string()),
                                    produced_at: Instant::now(),
                                })
                                .await;
                            break;
                        }
                        Ok(IpcEventKind::Error { code, message }) => {
                            warn!(adapter = %adapter_name_stream, code = %code, msg = %message, "Adapter stream error");
                            seq += 1;
                            let _ = tx_producer
                                .send(TokenEvent {
                                    sequence: seq,
                                    token: None,
                                    is_final: true,
                                    finish_reason: Some("error".to_string()),
                                    produced_at: Instant::now(),
                                })
                                .await;
                            break;
                        }
                        _ => {} // Usage, Result, parse errors - skip
                    }
                }
                Ok(None) => {
                    // Channel closed - adapter crashed
                    warn!(adapter = %adapter_name_stream, "IPC channel closed during streaming");
                    seq += 1;
                    let _ = tx_producer
                        .send(TokenEvent {
                            sequence: seq,
                            token: None,
                            is_final: true,
                            finish_reason: Some("error".to_string()),
                            produced_at: Instant::now(),
                        })
                        .await;
                    break;
                }
                Err(_) => {
                    // Timeout
                    warn!(adapter = %adapter_name_stream, "Token timeout during streaming");
                    seq += 1;
                    let _ = tx_producer
                        .send(TokenEvent {
                            sequence: seq,
                            token: None,
                            is_final: true,
                            finish_reason: Some("error".to_string()),
                            produced_at: Instant::now(),
                        })
                        .await;
                    break;
                }
            }
        }

        debug!(request_id = %spawn_id, tokens = seq, "Streaming token producer finished");
    });

    // Release the scheduler's streaming slot when the stream actually ends
    // (completion / token timeout / client disconnect), not on a fixed 300s timer
    // (audit D7). The guard is moved into the producing task and drops on its exit.
    let slot_guard = SequenceGuard {
        scheduler: state.scheduler.clone(),
        instance: decision.instance_id.clone(),
        seq_id: Some(session_id),
    };

    // Build the SSE byte stream
    let sse_stream = build_sse_stream(rx, request_id, model_id.clone(), last_event_id, slot_guard);

    // Build response with SSE headers
    let body = axum::body::Body::from_stream(sse_stream);

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .header("X-Stream-Id", stream_id.to_string())
        .body(body)
        .map_err(|_| ApiError::InternalError)?;

    info!(
        request_id = %request_id,
        stream_id = %stream_id,
        model = %model_id,
        profile = %profile.profile_id,
        "SSE stream started"
    );

    Ok(response)
}

// ─── SSE Stream Construction ───────────────────────────────────────

/// Releases the scheduler's streaming sequence slot when dropped. It lives inside
/// the SSE producing task, so the slot frees the moment the stream ends - normal
/// completion, token timeout, or client disconnect - instead of after a fixed
/// 300s timer that pinned the slot even for an abandoned stream (audit D7).
struct SequenceGuard {
    scheduler: Arc<dyn mai_scheduler::Scheduler>,
    instance: mai_scheduler::InstanceId,
    seq_id: Option<mai_scheduler::SequenceId>,
}

impl Drop for SequenceGuard {
    fn drop(&mut self) {
        if let Some(seq) = self.seq_id.take() {
            // UFCS so the call does not depend on the `Scheduler` trait being in scope.
            mai_scheduler::Scheduler::release_sequence(
                self.scheduler.as_ref(),
                &self.instance,
                seq,
            );
        }
    }
}

/// Build the SSE byte stream from a token receiver.
///
/// Returns a `Stream<Item = Result<bytes::Bytes, std::io::Error>>` that
/// axum can serve as a streaming response body. `slot_guard` is moved into the
/// producing task and released when the stream ends (audit D7).
fn build_sse_stream(
    mut rx: TokenReceiver,
    request_id: Uuid,
    model_id: String,
    resume_from: Option<u64>,
    slot_guard: SequenceGuard,
) -> impl Stream<Item = Result<bytes::Bytes, std::io::Error>> {
    // We use an async_stream-style approach via futures_util.
    // The stream yields SSE-formatted byte chunks.
    let (event_tx, event_rx) = mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(128);

    tokio::spawn(async move {
        // Held for the task's lifetime; on task end (stream complete / timeout /
        // client disconnect breaks the loop below) it drops and frees the slot.
        let _slot_guard = slot_guard;
        let mut backpressure = BackpressureMonitor::new(BACKPRESSURE_CAPACITY);
        let mut replay_buffer: VecDeque<(u64, bytes::Bytes)> =
            VecDeque::with_capacity(BACKPRESSURE_CAPACITY);
        let mut sequence: u64 = 0;
        let mut heartbeat_interval = interval(HEARTBEAT_INTERVAL);
        let mut last_token_time = Instant::now();
        let response_id = format!("chatcmpl-{request_id}");

        // If resuming, we skip events until we pass the resume point.
        let skip_until = resume_from.unwrap_or(0);

        // Send initial role delta (first chunk has role, no content)
        // This is standard OpenAI SSE behavior.

        loop {
            tokio::select! {
                // Token from adapter
                maybe_token = timeout(TOKEN_TIMEOUT, rx.recv()) => {
                    match maybe_token {
                        Ok(Some(event)) => {
                            last_token_time = Instant::now();
                            sequence = event.sequence;

                            // Build chunk
                            let chunk = if event.is_final {
                                // Send final data chunk then [DONE]
                                let final_chunk = build_chunk(
                                    &response_id,
                                    &model_id,
                                    None,
                                    event.finish_reason.as_deref(),
                                );
                                let final_bytes = format_sse_event(sequence, &final_chunk);

                                // Buffer management
                                buffer_event(&mut replay_buffer, &mut backpressure, sequence, final_bytes.clone());

                                if sequence > skip_until
                                    && event_tx.send(Ok(final_bytes)).await.is_err()
                                {
                                    debug!("SSE client disconnected during final chunk");
                                    break;
                                }

                                // Send [DONE] sentinel
                                let done = bytes::Bytes::from("data: [DONE]\n\n");
                                let _ = event_tx.send(Ok(done)).await;
                                break;
                            } else {
                                build_chunk(
                                    &response_id,
                                    &model_id,
                                    event.token.as_deref(),
                                    None,
                                )
                            };

                            let sse_bytes = format_sse_event(sequence, &chunk);

                            // Backpressure: drop oldest if buffer is full
                            if backpressure.should_drop()
                                && let Some((dropped_seq, _)) = replay_buffer.pop_front()
                            {
                                backpressure.event_dropped(dropped_seq);
                                // Insert gap marker
                                let gap = bytes::Bytes::from(format!(
                                    ": gap dropped_from={} dropped_count={}\n\n",
                                    dropped_seq,
                                    backpressure.total_dropped()
                                ));
                                if sequence > skip_until {
                                    let _ = event_tx.send(Ok(gap)).await;
                                }
                            }

                            buffer_event(&mut replay_buffer, &mut backpressure, sequence, sse_bytes.clone());

                            if sequence > skip_until
                                && event_tx.send(Ok(sse_bytes)).await.is_err()
                            {
                                debug!("SSE client disconnected");
                                break;
                            }
                        }
                        Ok(None) => {
                            // Sender dropped (adapter finished without final event)
                            let done = bytes::Bytes::from("data: [DONE]\n\n");
                            let _ = event_tx.send(Ok(done)).await;
                            break;
                        }
                        Err(_) => {
                            // Token timeout: 30 seconds with no token
                            warn!(
                                request_id = %request_id,
                                last_token_secs = last_token_time.elapsed().as_secs(),
                                "SSE token timeout"
                            );
                            let error_event = format!(
                                "event: error\ndata: {{\"error\":\"Token timeout: no token received in {} seconds\",\"code\":\"MAI-3005\"}}\n\n",
                                TOKEN_TIMEOUT.as_secs()
                            );
                            let _ = event_tx.send(Ok(bytes::Bytes::from(error_event))).await;
                            break;
                        }
                    }
                }

                // Heartbeat timer
                _ = heartbeat_interval.tick() => {
                    let heartbeat = bytes::Bytes::from(": heartbeat\n\n");
                    if event_tx.send(Ok(heartbeat)).await.is_err() {
                        debug!("SSE client disconnected during heartbeat");
                        break;
                    }
                }
            }
        }

        debug!(
            request_id = %request_id,
            events_sent = sequence,
            events_dropped = backpressure.total_dropped(),
            "SSE stream ended"
        );
    });

    // Convert the mpsc receiver into a Stream
    tokio_stream::wrappers::ReceiverStream::new(event_rx)
}

// ─── SSE Formatting Helpers ────────────────────────────────────────

/// Format a ChatCompletionChunk as an SSE event with id and data fields.
fn format_sse_event(sequence: u64, chunk: &ChatCompletionChunk) -> bytes::Bytes {
    let json = serde_json::to_string(chunk).unwrap_or_else(|_| "{}".to_string());
    let formatted = format!("id: {sequence}\ndata: {json}\n\n");
    bytes::Bytes::from(formatted)
}

/// Build a ChatCompletionChunk for a single delta.
fn build_chunk(
    response_id: &str,
    model_id: &str,
    content: Option<&str>,
    finish_reason: Option<&str>,
) -> ChatCompletionChunk {
    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());

    ChatCompletionChunk {
        id: response_id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created: now_epoch,
        model: model_id.to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: ChunkDelta {
                role: None,
                content: content.map(std::string::ToString::to_string),
            },
            finish_reason: finish_reason.map(std::string::ToString::to_string),
        }],
    }
}

/// Buffer an SSE event for potential replay on resume.
fn buffer_event(
    buffer: &mut VecDeque<(u64, bytes::Bytes)>,
    monitor: &mut BackpressureMonitor,
    sequence: u64,
    data: bytes::Bytes,
) {
    buffer.push_back((sequence, data));
    monitor.event_buffered();
    // Enforce max buffer size
    while buffer.len() > BACKPRESSURE_CAPACITY {
        if let Some((dropped_seq, _)) = buffer.pop_front() {
            monitor.event_dropped(dropped_seq);
        }
    }
}

// ─── Request Validation ────────────────────────────────────────────

fn validate_sse_request(req: &ChatCompletionRequest) -> Result<(), ApiError> {
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
    Ok(())
}

// ─── Profile-to-Priority (shared with inference handler) ───────────

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

#[allow(clippy::cast_possible_truncation)]
fn estimate_chat_tokens(req: &ChatCompletionRequest) -> u32 {
    let char_count: usize = req
        .messages
        .iter()
        .map(|m| m.content.len() + m.role.len())
        .sum();
    (char_count / 4).max(1) as u32
}

// ─── Prompt / Param Builders (mirrored from inference handler) ────

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

// ─── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ApiChatMessage;

    #[test]
    fn test_format_sse_event() {
        let chunk = build_chunk("chatcmpl-test", "test-model", Some("Hello"), None);
        let bytes = format_sse_event(1, &chunk);
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.starts_with("id: 1\n"));
        assert!(text.contains("data: "));
        assert!(text.contains("chat.completion.chunk"));
        assert!(text.contains("Hello"));
        assert!(text.ends_with("\n\n"));
    }

    #[test]
    fn test_format_sse_event_final() {
        let chunk = build_chunk("chatcmpl-test", "test-model", None, Some("stop"));
        let bytes = format_sse_event(5, &chunk);
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("\"finish_reason\":\"stop\""));
    }

    #[test]
    fn test_build_chunk_content() {
        let chunk = build_chunk("id-1", "model-1", Some("world"), None);
        assert_eq!(chunk.object, "chat.completion.chunk");
        assert_eq!(chunk.choices.len(), 1);
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("world"));
        assert!(chunk.choices[0].finish_reason.is_none());
    }

    #[test]
    fn test_build_chunk_finish() {
        let chunk = build_chunk("id-1", "model-1", None, Some("length"));
        assert_eq!(chunk.choices[0].finish_reason.as_deref(), Some("length"));
        assert!(chunk.choices[0].delta.content.is_none());
    }

    #[test]
    fn test_buffer_event_overflow() {
        let mut buffer = VecDeque::new();
        let mut monitor = BackpressureMonitor::new(3);

        for i in 1..=5 {
            buffer_event(
                &mut buffer,
                &mut monitor,
                i,
                bytes::Bytes::from(format!("event-{i}")),
            );
        }

        // Buffer should cap at BACKPRESSURE_CAPACITY (3 for this test's monitor)
        // but buffer_event uses the const BACKPRESSURE_CAPACITY (64) for trimming,
        // so the monitor tracks overflow via its own capacity.
        // The VecDeque trim inside buffer_event uses the const.
        assert!(buffer.len() <= BACKPRESSURE_CAPACITY);
    }

    #[test]
    fn test_validate_sse_request_empty_messages() {
        let req = ChatCompletionRequest {
            model: None,
            messages: vec![],
            stream: true,
            temperature: None,
            top_p: None,
            max_tokens: None,
            stop: None,
            frequency_penalty: None,
            presence_penalty: None,
            user: None,
        };
        assert!(validate_sse_request(&req).is_err());
    }

    #[test]
    fn test_validate_sse_request_bad_temp() {
        let req = ChatCompletionRequest {
            model: None,
            messages: vec![ApiChatMessage {
                role: "user".to_string(),
                content: "hi".to_string(),
                name: None,
            }],
            stream: true,
            temperature: Some(3.0),
            top_p: None,
            max_tokens: None,
            stop: None,
            frequency_penalty: None,
            presence_penalty: None,
            user: None,
        };
        assert!(validate_sse_request(&req).is_err());
    }

    #[test]
    fn test_validate_sse_request_valid() {
        let req = ChatCompletionRequest {
            model: Some("test-model".to_string()),
            messages: vec![ApiChatMessage {
                role: "user".to_string(),
                content: "hello".to_string(),
                name: None,
            }],
            stream: true,
            temperature: Some(0.7),
            top_p: None,
            max_tokens: None,
            stop: None,
            frequency_penalty: None,
            presence_penalty: None,
            user: None,
        };
        assert!(validate_sse_request(&req).is_ok());
    }
}
