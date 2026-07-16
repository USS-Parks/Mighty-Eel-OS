//! G2 gate — the **same** neutral request served by **two** providers through
//! one [`Provider`] interface, **streaming included**.
//!
//! Pure local mocks of the OpenAI `/v1/chat/completions` and Anthropic
//! `/v1/messages` contracts (JSON + SSE) on ephemeral ports — no OpenBao, no
//! Docker, so this runs in the normal test lane on every push. Real-cloud runs
//! (a live OpenAI / Anthropic key) are owner-gated, mirroring the W7/W8 pattern.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aog_gateway::provider::anthropic::AnthropicProvider;
use aog_gateway::provider::openai::OpenAiProvider;
use aog_gateway::provider::{
    ChatMessage, CompletionRequest, Provider, ProviderError, ProviderLimits, Registry,
};
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::{CONTENT_TYPE, LOCATION};
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
data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
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

async fn spawn_static(path: &'static str, body: String, content_type: &'static str) -> String {
    let app = Router::new().route(
        path,
        post(move || {
            let body = body.clone();
            async move { ([(CONTENT_TYPE, content_type)], body) }
        }),
    );
    spawn(app).await
}

async fn slow_sse() -> Response {
    let stream = async_stream::stream! {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            yield Ok::<Bytes, std::convert::Infallible>(Bytes::from_static(b": keepalive\n\n"));
        }
    };
    (
        [(CONTENT_TYPE, "text/event-stream")],
        Body::from_stream(stream),
    )
        .into_response()
}

async fn header_heavy() -> Response {
    let mut response = Json(json!({
        "model": "fixture",
        "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1}
    }))
    .into_response();
    response.headers_mut().insert("x-one", "1".parse().unwrap());
    response.headers_mut().insert("x-two", "2".parse().unwrap());
    response
        .headers_mut()
        .insert("x-three", "3".parse().unwrap());
    response
}

async fn stream_error(provider: &dyn Provider, request: &CompletionRequest) -> ProviderError {
    let mut stream = provider.stream(request).await.expect("response headers");
    while let Some(frame) = stream.next().await {
        if let Err(error) = frame {
            return error;
        }
    }
    panic!("faulted provider stream ended without an error")
}

async fn redirect_to(State(location): State<String>) -> Response {
    (StatusCode::TEMPORARY_REDIRECT, [(LOCATION, location)]).into_response()
}

async fn credential_sink(State(hits): State<Arc<AtomicUsize>>) -> Response {
    hits.fetch_add(1, Ordering::SeqCst);
    StatusCode::NO_CONTENT.into_response()
}

#[tokio::test]
async fn same_request_two_providers_one_interface_with_streaming() {
    let openai_base = spawn(Router::new().route("/v1/chat/completions", post(openai_mock))).await;
    let anthropic_base = spawn(Router::new().route("/v1/messages", post(anthropic_mock))).await;

    // Two real adapters behind one Registry.
    let mut reg = Registry::new();
    reg.register(Arc::new(OpenAiProvider::new(
        "openai",
        aog_gateway::posture::ApprovedProviderEndpoint::loopback_fixture(&openai_base).unwrap(),
        "test-key",
    )));
    reg.register(Arc::new(AnthropicProvider::new(
        aog_gateway::posture::ApprovedProviderEndpoint::loopback_fixture(&anthropic_base).unwrap(),
        "test-key",
    )));
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

#[tokio::test]
async fn cross_origin_redirects_never_receive_provider_credentials() {
    let sink_hits = Arc::new(AtomicUsize::new(0));
    let sink_base = spawn(
        Router::new()
            .route("/stolen", post(credential_sink))
            .with_state(sink_hits.clone()),
    )
    .await;
    let redirect_base = spawn(
        Router::new()
            .route("/v1/chat/completions", post(redirect_to))
            .route("/v1/messages", post(redirect_to))
            .with_state(format!("{sink_base}/stolen")),
    )
    .await;
    let endpoint =
        aog_gateway::posture::ApprovedProviderEndpoint::loopback_fixture(&redirect_base).unwrap();
    let request = CompletionRequest {
        model: "fixture".to_string(),
        messages: vec![ChatMessage::user("credential leak probe")],
        max_tokens: Some(1),
        temperature: None,
    };

    let openai = OpenAiProvider::new("openai", endpoint.clone(), "openai-secret");
    let anthropic = AnthropicProvider::new(endpoint, "anthropic-secret");
    for result in [
        openai.complete(&request).await,
        anthropic.complete(&request).await,
    ] {
        assert!(
            matches!(result, Err(ProviderError::Upstream { status: 307, .. })),
            "redirect must be surfaced rather than followed: {result:?}"
        );
    }
    assert_eq!(
        sink_hits.load(Ordering::SeqCst),
        0,
        "redirect target received a request that could carry provider credentials"
    );
}

#[tokio::test]
async fn provider_bodies_sse_and_deadlines_are_bounded_and_truthful() {
    let request = CompletionRequest {
        model: "fixture".to_string(),
        messages: vec![ChatMessage::user("bounded response probe")],
        max_tokens: Some(1),
        temperature: None,
    };
    let endpoint = |base: &str| {
        aog_gateway::posture::ApprovedProviderEndpoint::loopback_fixture(base).unwrap()
    };

    let body_limits = ProviderLimits {
        max_body_bytes: 256,
        ..ProviderLimits::default()
    };
    let openai_base =
        spawn_static("/v1/chat/completions", "x".repeat(1024), "application/json").await;
    let anthropic_base = spawn_static("/v1/messages", "x".repeat(1024), "application/json").await;
    for result in [
        OpenAiProvider::new_with_limits("openai", endpoint(&openai_base), "key", body_limits)
            .complete(&request)
            .await,
        AnthropicProvider::new_with_limits(endpoint(&anthropic_base), "key", body_limits)
            .complete(&request)
            .await,
    ] {
        assert!(matches!(result, Err(ProviderError::Limit { .. })));
    }

    let header_base = spawn(Router::new().route("/v1/chat/completions", post(header_heavy))).await;
    let header_limits = ProviderLimits {
        max_headers: 2,
        ..ProviderLimits::default()
    };
    let result =
        OpenAiProvider::new_with_limits("openai", endpoint(&header_base), "key", header_limits)
            .complete(&request)
            .await;
    assert!(matches!(
        result,
        Err(ProviderError::Limit {
            resource: "header count",
            ..
        })
    ));

    let sse_limits = ProviderLimits {
        max_sse_line_bytes: 128,
        max_sse_frame_bytes: 192,
        ..ProviderLimits::default()
    };
    let long_line = spawn_static(
        "/v1/chat/completions",
        format!("data: {}", "x".repeat(512)),
        "text/event-stream",
    )
    .await;
    let error = stream_error(
        &OpenAiProvider::new_with_limits("openai", endpoint(&long_line), "key", sse_limits),
        &request,
    )
    .await;
    assert!(matches!(
        error,
        ProviderError::Limit {
            resource: "SSE line bytes",
            ..
        }
    ));

    let oversized_frame = spawn_static(
        "/v1/chat/completions",
        "event: a\nevent: b\nevent: c\nevent: d\n".to_string(),
        "text/event-stream",
    )
    .await;
    let frame_limits = ProviderLimits {
        max_sse_line_bytes: 64,
        max_sse_frame_bytes: 24,
        ..ProviderLimits::default()
    };
    let error = stream_error(
        &OpenAiProvider::new_with_limits("openai", endpoint(&oversized_frame), "key", frame_limits),
        &request,
    )
    .await;
    assert!(matches!(
        error,
        ProviderError::Limit {
            resource: "SSE frame bytes",
            ..
        }
    ));

    for body in [
        "data: not-json\n\n",
        "data: [DONE]\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
    ] {
        let base = spawn_static(
            "/v1/chat/completions",
            body.to_string(),
            "text/event-stream",
        )
        .await;
        let error = stream_error(
            &OpenAiProvider::new("openai", endpoint(&base), "key"),
            &request,
        )
        .await;
        assert!(matches!(
            error,
            ProviderError::Decode(_) | ProviderError::Truncated(_)
        ));
    }

    let anthropic_stop = spawn_static(
        "/v1/messages",
        "data: {\"type\":\"message_stop\"}\n\n".to_string(),
        "text/event-stream",
    )
    .await;
    let error = stream_error(
        &AnthropicProvider::new(endpoint(&anthropic_stop), "key"),
        &request,
    )
    .await;
    assert!(matches!(error, ProviderError::Truncated(_)));

    let slow_base = spawn(Router::new().route("/v1/chat/completions", post(slow_sse))).await;
    let deadline_limits = ProviderLimits {
        idle_timeout: std::time::Duration::from_millis(40),
        total_timeout: std::time::Duration::from_millis(75),
        ..ProviderLimits::default()
    };
    let started = std::time::Instant::now();
    let error = stream_error(
        &OpenAiProvider::new_with_limits("openai", endpoint(&slow_base), "key", deadline_limits),
        &request,
    )
    .await;
    assert!(matches!(error, ProviderError::Transport(_)));
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}
