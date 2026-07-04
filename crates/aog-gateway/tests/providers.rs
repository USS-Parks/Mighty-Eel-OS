//! G2 gate — the **same** neutral request served by **two** providers through
//! one [`Provider`] interface, **streaming included**.
//!
//! Pure local mocks of the OpenAI `/v1/chat/completions` and Anthropic
//! `/v1/messages` contracts (JSON + SSE) on ephemeral ports — no OpenBao, no
//! Docker, so this runs in the normal test lane on every push. Real-cloud runs
//! (a live OpenAI / Anthropic key) are owner-gated, mirroring the W7/W8 pattern.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;

use aog_gateway::provider::anthropic::AnthropicProvider;
use aog_gateway::provider::openai::OpenAiProvider;
use aog_gateway::provider::{ChatMessage, CompletionRequest, Registry};
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::net::TcpListener;

const OPENAI_SSE: &str = "\
data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"from \"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"OpenAI\"}}]}\n\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":4}}\n\n\
data: [DONE]\n\n";

const ANTHROPIC_SSE: &str = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":6,\"output_tokens\":1}}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello \"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"from \"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Anthropic\"}}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";

fn sse(body: &'static str) -> Response {
    ([(CONTENT_TYPE, "text/event-stream")], body).into_response()
}

async fn openai_mock(Json(body): Json<Value>) -> Response {
    if body["stream"].as_bool().unwrap_or(false) {
        sse(OPENAI_SSE)
    } else {
        Json(json!({
            "model": "gpt-4o-mini",
            "choices": [{"message": {"content": "Hello from OpenAI"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 4}
        }))
        .into_response()
    }
}

async fn anthropic_mock(Json(body): Json<Value>) -> Response {
    if body["stream"].as_bool().unwrap_or(false) {
        sse(ANTHROPIC_SSE)
    } else {
        Json(json!({
            "model": "claude-3-5-sonnet",
            "content": [{"type": "text", "text": "Hello from Anthropic"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 6, "output_tokens": 4}
        }))
        .into_response()
    }
}

async fn spawn(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

#[tokio::test]
async fn same_request_two_providers_one_interface_with_streaming() {
    let openai_base = spawn(Router::new().route("/v1/chat/completions", post(openai_mock))).await;
    let anthropic_base = spawn(Router::new().route("/v1/messages", post(anthropic_mock))).await;

    // Two real adapters behind one Registry.
    let mut reg = Registry::new();
    reg.register(Arc::new(OpenAiProvider::new(
        "openai",
        openai_base,
        "test-key",
    )));
    reg.register(Arc::new(AnthropicProvider::new(anthropic_base, "test-key")));
    assert_eq!(
        reg.names(),
        vec!["anthropic".to_string(), "openai".to_string()]
    );

    // ONE neutral request, served by BOTH.
    let req = CompletionRequest {
        model: "does-not-matter-to-the-mock".to_string(),
        messages: vec![ChatMessage::user("Hi")],
        max_tokens: Some(64),
        temperature: Some(0.2),
    };

    for (name, expect, in_tok) in [
        ("openai", "Hello from OpenAI", 5u32),
        ("anthropic", "Hello from Anthropic", 6u32),
    ] {
        let p = reg.get(name).expect("provider registered");

        // One-shot.
        let resp = p.complete(&req).await.expect("complete");
        assert_eq!(resp.content, expect, "{name} one-shot content");
        assert_eq!(resp.usage.input_tokens, in_tok, "{name} input usage");
        assert_eq!(resp.usage.output_tokens, 4, "{name} output usage");

        // Streaming: deltas reassemble to the identical text, terminated by done.
        let mut stream = p.stream(&req).await.expect("stream");
        let mut text = String::new();
        let mut saw_done = false;
        let mut stream_out_tokens = 0;
        while let Some(frame) = stream.next().await {
            let c = frame.expect("frame");
            text.push_str(&c.delta);
            if let Some(u) = c.usage
                && u.output_tokens > 0
            {
                stream_out_tokens = u.output_tokens;
            }
            if c.done {
                saw_done = true;
            }
        }
        assert_eq!(text, expect, "{name} stream reassembles to the same text");
        assert!(saw_done, "{name} stream terminates with a done frame");
        assert_eq!(stream_out_tokens, 4, "{name} stream reported output usage");
    }

    eprintln!(
        "G2 gate PASSED: one Provider interface, two backends (openai + anthropic), one-shot + streaming"
    );
}
