//! Anthropic-compatible surface (G4) — the anti-lock-in signal.
//!
//! `/v1/messages` (JSON + event-typed SSE streaming). An Anthropic client (which
//! sends `x-api-key` + `anthropic-version`) repoints at the gateway with only a
//! base-URL change and gets the exact `message` shape back — the same neutral
//! backend that fronts the OpenAI surface (G3), so a customer is never locked to
//! one API dialect.

use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router, extract::State, http::HeaderMap, http::StatusCode};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::app::{AppState, authorize};
use crate::provider::{ChatMessage, ChunkStream, CompletionRequest, CompletionResponse, Role};
use crate::surface_openai::{new_id, opt_f32, opt_u32, provider_http, resolve_provider};

/// Mount the Anthropic-compatible routes over shared [`AppState`].
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/messages", post(messages))
        .with_state(state)
}

// ---- translation ---------------------------------------------------------

/// Flatten Anthropic content (a string, or an array of `{type:text,text}` blocks).
fn content_text(c: Option<&Value>) -> String {
    match c {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// The Anthropic top-level `system` (string or block array), if present.
fn system_text(body: &Value) -> Option<String> {
    match body.get("system") {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Array(_)) => {
            let s = content_text(body.get("system"));
            (!s.is_empty()).then_some(s)
        }
        _ => None,
    }
}

fn anthropic_neutral(body: &Value, upstream_model: String) -> CompletionRequest {
    let mut messages = Vec::new();
    // Anthropic carries the system prompt out-of-band; fold it back to a System turn.
    if let Some(system) = system_text(body) {
        messages.push(ChatMessage::system(system));
    }
    if let Some(arr) = body.get("messages").and_then(Value::as_array) {
        for m in arr {
            let role = match m.get("role").and_then(Value::as_str).unwrap_or("user") {
                "assistant" => Role::Assistant,
                _ => Role::User,
            };
            messages.push(ChatMessage {
                role,
                content: content_text(m.get("content")),
            });
        }
    }
    CompletionRequest {
        model: upstream_model,
        messages,
        max_tokens: opt_u32(body, "max_tokens"),
        temperature: opt_f32(body, "temperature"),
    }
}

/// Map a neutral finish reason to an Anthropic `stop_reason`.
fn stop_reason(reason: &str) -> &str {
    match reason {
        "stop" => "end_turn",
        "length" => "max_tokens",
        other => other,
    }
}

// ---- /v1/messages --------------------------------------------------------

async fn messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    // Auth + pre-flight budget (G1) — accepts x-api-key or Bearer.
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
    let neutral = anthropic_neutral(&body, target.model);
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

    let mut tokenized_spans = 0u32;
    let resp = if body.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        // Fail-closed: the streaming path does not yet tokenize egress or
        // meter/receipt per chunk. Until it does, refuse a stream that would send
        // un-tokenized sensitive spans to a cloud provider rather than leak them.
        let spans =
            crate::tokenize::egress(state.detector.as_ref(), target_cloud, &neutral.messages)
                .span_count();
        if target_cloud && spans > 0 {
            let err = json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": "streaming is unavailable for classified data over a cloud \
                                provider; retry without stream=true (the non-streaming path \
                                tokenizes egress before dispatch)",
                }
            });
            (StatusCode::BAD_REQUEST, Json(err)).into_response()
        } else {
            match provider.stream(&neutral).await {
                Ok(chunks) => messages_sse(inbound_model, chunks),
                Err(e) => provider_http(&e).into_response(),
            }
        }
    } else {
        // Tokenize sensitive spans before cloud egress (G8): the cloud provider
        // sees placeholders only; the response is detokenized inside the boundary.
        let egress =
            crate::tokenize::egress(state.detector.as_ref(), target_cloud, &neutral.messages);
        tokenized_spans = egress.span_count();
        let dispatched = CompletionRequest {
            messages: egress.messages,
            ..neutral.clone()
        };
        match provider.complete(&dispatched).await {
            Ok(mut r) => {
                r.content = crate::tokenize::restore(&r.content, &egress.map);
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
                        tokenized_spans,
                    },
                );
                // Budget decrement (G9): accrue this call's usage against the token;
                // the next resolve folds it in and rejects once a cap is reached.
                state.gateway.record_spend(
                    fabric_token::lineage_key(&ctx.token),
                    u64::from(r.usage.input_tokens) + u64::from(r.usage.output_tokens),
                    state.prices.cost(
                        &provider_name,
                        &inbound_model,
                        r.usage.input_tokens,
                        r.usage.output_tokens,
                    ),
                    0,
                );
                Json(message_json(&inbound_model, &r)).into_response()
            }
            Err(e) => provider_http(&e).into_response(),
        }
    };
    let resp = crate::route::tag_route(resp, &decision);
    let resp = crate::policy::tag_policy(resp, &policy_decision, state.mode, outcome);
    crate::tokenize::tag(resp, tokenized_spans)
}

fn message_json(model: &str, r: &CompletionResponse) -> Value {
    json!({
        "id": new_id("msg_"),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{ "type": "text", "text": r.content }],
        "stop_reason": stop_reason(&r.finish_reason),
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": r.usage.input_tokens,
            "output_tokens": r.usage.output_tokens,
        }
    })
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "SSE streams yield Result items; wrapping here keeps one typed annotation point"
)]
fn ev(name: &str, v: &Value) -> Result<Event, std::convert::Infallible> {
    Ok(Event::default().event(name).data(v.to_string()))
}

/// Re-emit a neutral [`ChunkStream`] as the Anthropic Messages SSE event sequence:
/// `message_start` → `content_block_start` → `content_block_delta`* →
/// `content_block_stop` → `message_delta` (usage) → `message_stop`.
fn messages_sse(model: String, mut chunks: ChunkStream) -> Response {
    let id = new_id("msg_");
    let stream = async_stream::stream! {
        yield ev("message_start", &json!({
            "type": "message_start",
            "message": {
                "id": id, "type": "message", "role": "assistant", "model": model,
                "content": [], "stop_reason": Value::Null, "stop_sequence": Value::Null,
                "usage": { "input_tokens": 0, "output_tokens": 0 },
            }
        }));
        yield ev("content_block_start", &json!({
            "type": "content_block_start", "index": 0,
            "content_block": { "type": "text", "text": "" }
        }));

        let mut out_tokens = 0u32;
        while let Some(frame) = chunks.next().await {
            match frame {
                Ok(c) => {
                    if let Some(u) = c.usage
                        && u.output_tokens > 0
                    {
                        out_tokens = u.output_tokens;
                    }
                    if !c.delta.is_empty() {
                        yield ev("content_block_delta", &json!({
                            "type": "content_block_delta", "index": 0,
                            "delta": { "type": "text_delta", "text": c.delta }
                        }));
                    }
                    if c.done {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        yield ev("content_block_stop", &json!({ "type": "content_block_stop", "index": 0 }));
        yield ev("message_delta", &json!({
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn", "stop_sequence": Value::Null },
            "usage": { "output_tokens": out_tokens }
        }));
        yield ev("message_stop", &json!({ "type": "message_stop" }));
    };
    Sse::new(stream).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_folds_to_a_system_turn_and_stop_maps() {
        let body = json!({
            "model": "claude",
            "max_tokens": 64,
            "system": "be terse",
            "messages": [{"role": "user", "content": "hi"}]
        });
        let req = anthropic_neutral(&body, "upstream".to_string());
        assert_eq!(req.messages.len(), 2);
        assert!(matches!(req.messages[0].role, Role::System));
        assert_eq!(req.messages[0].content, "be terse");
        assert_eq!(req.max_tokens, Some(64));
        assert_eq!(stop_reason("stop"), "end_turn");
        assert_eq!(stop_reason("length"), "max_tokens");
    }

    #[test]
    fn content_blocks_flatten() {
        let c = json!([{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]);
        assert_eq!(content_text(Some(&c)), "ab");
    }
}
