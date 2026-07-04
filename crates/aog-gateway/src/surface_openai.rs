//! OpenAI-compatible surface (G3).
//!
//! `/v1/chat/completions` (JSON + SSE streaming), `/v1/models`, `/v1/completions`
//! (legacy text, mapped to a single-message chat), `/v1/embeddings` (501 — the
//! `Provider` trait is chat-only; an embeddings backend is a follow-on). Every
//! call authorizes the virtual key + runs the pre-flight budget check (G1), maps
//! the requested model to a provider (G2), and translates the neutral response
//! back to OpenAI's exact wire shape — so an off-the-shelf OpenAI client repoints
//! at the gateway with only a base-URL change.

use std::sync::Arc;

use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router, extract::State, http::HeaderMap, http::StatusCode};
use chrono::Utc;
use futures::StreamExt;
use serde_json::{Value, json};

use crate::app::{AppState, Target, authorize};
use crate::provider::{
    ChatMessage, ChunkStream, CompletionRequest, CompletionResponse, Provider, ProviderError, Role,
};

/// Mount the OpenAI-compatible routes over shared [`AppState`].
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/completions", post(completions))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/models", get(list_models))
        .route("/v1/usage", get(usage))
        .with_state(state)
}

// ---- translation helpers -------------------------------------------------

fn message_content(c: Option<&Value>) -> String {
    match c {
        Some(Value::String(s)) => s.clone(),
        // vision / multi-part content: concatenate the text parts.
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn neutral_messages(body: &Value) -> Vec<ChatMessage> {
    body.get("messages")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|m| {
                    let role = match m.get("role").and_then(Value::as_str).unwrap_or("user") {
                        "system" | "developer" => Role::System,
                        "assistant" => Role::Assistant,
                        _ => Role::User,
                    };
                    ChatMessage {
                        role,
                        content: message_content(m.get("content")),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn opt_u32(body: &Value, key: &str) -> Option<u32> {
    body.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
}

#[allow(clippy::cast_possible_truncation)]
pub(crate) fn opt_f32(body: &Value, key: &str) -> Option<f32> {
    body.get(key).and_then(Value::as_f64).map(|f| f as f32)
}

pub(crate) fn new_id(prefix: &str) -> String {
    format!("{prefix}{}", Utc::now().timestamp_micros())
}

pub(crate) fn created() -> i64 {
    Utc::now().timestamp()
}

pub(crate) fn provider_http(e: &ProviderError) -> (StatusCode, String) {
    match e {
        ProviderError::Upstream { status, body } => (
            StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY),
            body.clone(),
        ),
        ProviderError::Transport(m) | ProviderError::Decode(m) => {
            (StatusCode::BAD_GATEWAY, m.clone())
        }
    }
}

/// Resolve an inbound model id to (target, provider), or an HTTP error.
pub(crate) fn resolve_provider(
    state: &AppState,
    model: &str,
) -> Result<(Target, Arc<dyn Provider>), (StatusCode, String)> {
    let target = state
        .models
        .resolve(model)
        .cloned()
        .ok_or((StatusCode::NOT_FOUND, format!("unknown model: {model}")))?;
    let provider = state.registry.get(&target.provider).ok_or((
        StatusCode::BAD_GATEWAY,
        format!("provider not registered: {}", target.provider),
    ))?;
    Ok((target, provider))
}

fn neutral_from(body: &Value, upstream_model: String) -> CompletionRequest {
    CompletionRequest {
        model: upstream_model,
        messages: neutral_messages(body),
        max_tokens: opt_u32(body, "max_tokens"),
        temperature: opt_f32(body, "temperature"),
    }
}

// ---- /v1/chat/completions ------------------------------------------------

async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    // Auth + pre-flight budget (G1).
    let ctx = match authorize(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let inbound_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let (target, provider) = match resolve_provider(&state, &inbound_model) {
        Ok(pair) => pair,
        Err(e) => return e.into_response(),
    };
    let target_cloud = crate::policy::target_is_cloud(&target);
    let provider_name = target.provider.clone();
    let workflow_id = crate::meter::workflow_from(&headers);
    let neutral = neutral_from(&body, target.model);
    let query = crate::route::query_text(&neutral.messages);

    // Classify + route (G5).
    let decision = crate::route::classify_and_route(
        state.router.as_ref(),
        &query,
        crate::route::estimate_tokens(neutral.max_tokens, &query),
        &ctx.tenant_id,
        ctx.token.roles.first().map_or("user", String::as_str),
    );
    // Policy + modes (G6): shadow logs, report-only flags, enforce blocks.
    let (policy_decision, outcome) =
        match crate::policy::gate(&state, target_cloud, &query, &decision) {
            Ok(pair) => pair,
            Err(blocked) => return blocked,
        };

    let resp = if body.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        match provider.stream(&neutral).await {
            Ok(chunks) => chat_sse(inbound_model, chunks),
            Err(e) => provider_http(&e).into_response(),
        }
    } else {
        match provider.complete(&neutral).await {
            Ok(r) => {
                // Metering + receipt (G7): every non-stream completion is receipted.
                crate::meter::record(
                    &state.receipts,
                    &state.prices,
                    &crate::meter::Completion {
                        ctx: &ctx,
                        provider: &provider_name,
                        model: &inbound_model,
                        route: &decision,
                        allowed_cloud: policy_decision.allowed_cloud,
                        usage: r.usage,
                        workflow_id: workflow_id.clone(),
                    },
                );
                Json(chat_completion_json(&inbound_model, &r)).into_response()
            }
            Err(e) => provider_http(&e).into_response(),
        }
    };
    let resp = crate::route::tag_route(resp, &decision);
    crate::policy::tag_policy(resp, &policy_decision, state.mode, &outcome)
}

/// `GET /v1/usage` — aog-meter aggregates (per tenant/provider/model/task) +
/// the receipt-chain head + a live chain-verify. Authenticated.
async fn usage(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(e) = authorize(&state, &headers).await {
        return e.into_response();
    }
    let (aggregates, head, verified) = {
        let led = state.receipts.lock().expect("receipt ledger lock");
        (led.aggregate(), led.head_hex(), led.verify())
    };
    Json(json!({
        "aggregates": aggregates,
        "chain_head": head,
        "chain_verified": verified,
    }))
    .into_response()
}

fn chat_completion_json(model: &str, r: &CompletionResponse) -> Value {
    let total = u64::from(r.usage.input_tokens) + u64::from(r.usage.output_tokens);
    json!({
        "id": new_id("chatcmpl-"),
        "object": "chat.completion",
        "created": created(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": r.content },
            "finish_reason": r.finish_reason,
        }],
        "usage": {
            "prompt_tokens": r.usage.input_tokens,
            "completion_tokens": r.usage.output_tokens,
            "total_tokens": total,
        }
    })
}

fn chunk_json(id: &str, ts: i64, model: &str, delta: Value, finish: Option<&str>) -> String {
    json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": ts,
        "model": model,
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }]
    })
    .to_string()
}

/// Re-emit a neutral [`ChunkStream`] as OpenAI `chat.completion.chunk` SSE frames,
/// opening with a role delta and closing with a `finish_reason:"stop"` frame then
/// the literal `data: [DONE]` an OpenAI client waits for.
fn chat_sse(model: String, mut chunks: ChunkStream) -> Response {
    let id = new_id("chatcmpl-");
    let ts = created();
    let stream = async_stream::stream! {
        yield Ok::<Event, std::convert::Infallible>(
            Event::default().data(chunk_json(&id, ts, &model, json!({ "role": "assistant" }), None)),
        );
        let mut closed = false;
        while let Some(frame) = chunks.next().await {
            match frame {
                Ok(c) => {
                    if !c.delta.is_empty() {
                        yield Ok(Event::default()
                            .data(chunk_json(&id, ts, &model, json!({ "content": c.delta }), None)));
                    }
                    if c.done {
                        yield Ok(Event::default()
                            .data(chunk_json(&id, ts, &model, json!({}), Some("stop"))));
                        closed = true;
                    }
                }
                // A mid-stream provider error ends the stream; the client sees a
                // truncated-but-well-formed SSE close.
                Err(_) => break,
            }
        }
        if !closed {
            yield Ok(Event::default().data(chunk_json(&id, ts, &model, json!({}), Some("stop"))));
        }
        yield Ok(Event::default().data("[DONE]"));
    };
    Sse::new(stream).into_response()
}

// ---- /v1/models ----------------------------------------------------------

async fn list_models(State(state): State<AppState>) -> Json<Value> {
    let data: Vec<Value> = state
        .models
        .model_ids()
        .into_iter()
        .map(|id| json!({ "id": id, "object": "model", "created": 0, "owned_by": "aog" }))
        .collect();
    Json(json!({ "object": "list", "data": data }))
}

// ---- /v1/completions (legacy) --------------------------------------------

async fn completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(e) = authorize(&state, &headers).await {
        return e.into_response();
    }
    let inbound_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let (target, provider) = match resolve_provider(&state, &inbound_model) {
        Ok(pair) => pair,
        Err(e) => return e.into_response(),
    };
    let prompt = match body.get("prompt") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    let neutral = CompletionRequest {
        model: target.model,
        messages: vec![ChatMessage::user(prompt)],
        max_tokens: opt_u32(&body, "max_tokens"),
        temperature: opt_f32(&body, "temperature"),
    };
    match provider.complete(&neutral).await {
        Ok(r) => {
            let total = u64::from(r.usage.input_tokens) + u64::from(r.usage.output_tokens);
            Json(json!({
                "id": new_id("cmpl-"),
                "object": "text_completion",
                "created": created(),
                "model": inbound_model,
                "choices": [{ "text": r.content, "index": 0, "finish_reason": r.finish_reason }],
                "usage": {
                    "prompt_tokens": r.usage.input_tokens,
                    "completion_tokens": r.usage.output_tokens,
                    "total_tokens": total,
                }
            }))
            .into_response()
        }
        Err(e) => provider_http(&e).into_response(),
    }
}

// ---- /v1/embeddings ------------------------------------------------------

async fn embeddings() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "error": {
                "message": "embeddings are not yet wired in the AOG gateway (the Provider trait is chat-only); an embeddings backend is a follow-on",
                "type": "not_implemented",
            }
        })),
    )
        .into_response()
}
