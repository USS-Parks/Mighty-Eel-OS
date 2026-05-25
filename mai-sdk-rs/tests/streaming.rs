//! J-17 integration tests for the `mai-sdk-rs` SSE streaming surface.
//!
//! Targets:
//! - `MaiClient::chat_stream` (POST /v1/chat/completions with stream=true,
//!   buffered SSE response parsed into a `ChatStreamHandle`)
//! - `MaiClient::chat_stream_resume` (J-17: same surface, with the
//!   `Last-Event-ID` header set so the server can skip already-seen events)
//! - `ChatStreamHandle::next_chunk` / `last_event_id` semantics
//!
//! Each test stands up an in-process `wiremock::MockServer` that returns
//! a canned SSE response body, then exercises the SDK against it.

use std::collections::HashMap;
use std::time::Duration;

use mai_sdk_rs::*;
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ─── Helpers ───────────────────────────────────────────────────────

fn client_for(server: &MockServer) -> MaiClient {
    MaiClient::new(MaiClientConfig {
        base_url: server.uri(),
        api_key: Some("test-key".to_string()),
        profile_id: String::new(),
        priority: RequestPriority::Normal,
        timeout: Duration::from_secs(2),
        extra_headers: HashMap::new(),
    })
    .expect("client construction must succeed when api_key is set")
}

fn chat_request() -> ChatCompletionRequest {
    ChatCompletionRequest {
        model: "phi-4-mini".to_string(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "Tell me something".to_string(),
            tool_call_id: None,
        }],
        max_tokens: None,
        temperature: None,
        top_p: None,
        stop: None,
        stream: false,
    }
}

/// Build a canonical SSE chunk JSON for one delta of token text.
fn chunk_json(id: &str, content: &str) -> String {
    format!(
        r#"{{"id":"{id}","object":"chat.completion.chunk","created":1,"model":"phi-4-mini","choices":[{{"index":0,"delta":{{"role":"assistant","content":"{content}"}},"finish_reason":null}}]}}"#,
    )
}

/// Build a full SSE response body from a list of (event_id, content) pairs,
/// terminated with the canonical `data: [DONE]` sentinel.
fn sse_body(events: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (id, content) in events {
        out.push_str(&format!("id: {id}\n"));
        out.push_str(&format!("data: {}\n", chunk_json(id, content)));
        out.push('\n');
    }
    out.push_str("data: [DONE]\n\n");
    out
}

// ─── chat_stream happy path (3 tests) ──────────────────────────────

#[tokio::test]
async fn chat_stream_yields_events_in_order() {
    let server = MockServer::start().await;
    let body = sse_body(&[("evt-1", "Hel"), ("evt-2", "lo "), ("evt-3", "world")]);
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("X-IM-Auth-Token", "test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let mut handle = client_for(&server)
        .chat_stream(chat_request())
        .await
        .unwrap();
    let mut texts: Vec<String> = Vec::new();
    while let Some(chunk) = handle.next_chunk().await.unwrap() {
        let delta = chunk.choices[0].delta.content.clone().unwrap_or_default();
        texts.push(delta);
    }
    assert_eq!(texts, vec!["Hel", "lo ", "world"]);
    assert_eq!(handle.last_event_id(), Some("evt-3"));
    let none = handle.next_chunk().await.unwrap();
    assert!(none.is_none(), "stream must be exhausted after drain");
}

#[tokio::test]
async fn chat_stream_captures_last_event_id() {
    let server = MockServer::start().await;
    let body = sse_body(&[("evt-42", "a"), ("evt-43", "b")]);
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let handle = client_for(&server)
        .chat_stream(chat_request())
        .await
        .unwrap();
    assert_eq!(
        handle.last_event_id(),
        Some("evt-43"),
        "last_event_id should reflect the FINAL id seen during parse"
    );
    // The id is captured eagerly during parsing — it must be available
    // before any next_chunk() call.
    drop(handle);
    let handle2 = client_for(&server)
        .chat_stream(chat_request())
        .await
        .unwrap();
    assert!(handle2.last_event_id().is_some());
    assert!(!handle2.last_event_id().unwrap().is_empty());
}

#[tokio::test]
async fn chat_stream_handles_done_only_body() {
    // A response with only the [DONE] terminator (no real events) is a
    // valid SSE response indicating the model produced no tokens. The
    // SDK must yield zero chunks without error.
    let server = MockServer::start().await;
    let body = "data: [DONE]\n\n".to_string();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let mut handle = client_for(&server)
        .chat_stream(chat_request())
        .await
        .unwrap();
    let first = handle.next_chunk().await.unwrap();
    assert!(first.is_none(), "DONE-only body should yield zero chunks");
    assert_eq!(handle.last_event_id(), None);
    assert!(handle.next_chunk().await.unwrap().is_none());
}

// ─── chat_stream error paths (2 tests) ─────────────────────────────

#[tokio::test]
async fn chat_stream_non_2xx_maps_to_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({
            "type": "overloaded",
            "code": "MAI-5003",
            "message": "All adapters busy; retry shortly",
            "retry_after_seconds": 5_u64,
            "request_id": null,
        })))
        .mount(&server)
        .await;

    let err = match client_for(&server).chat_stream(chat_request()).await {
        Ok(_) => panic!("503 must surface as Err, got Ok"),
        Err(e) => e,
    };
    match err {
        SdkError::Api(api) => {
            assert_eq!(api.error_type, MaiErrorType::Overloaded);
            assert_eq!(api.code, "MAI-5003");
            assert!(api.message.contains("retry"));
            assert_eq!(api.retry_after_seconds, Some(5));
        }
        other => panic!("expected SdkError::Api, got {other:?}"),
    }
}

#[tokio::test]
async fn chat_stream_malformed_sse_chunk_returns_deserialization_error() {
    // Server returns SSE-shaped events but the JSON in each data: line is
    // garbage. The handle should error during construction (from_sse_body)
    // rather than yield a malformed chunk to the caller.
    let server = MockServer::start().await;
    let body = "id: evt-1\ndata: { not valid json\n\ndata: [DONE]\n\n".to_string();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let err = match client_for(&server).chat_stream(chat_request()).await {
        Ok(_) => panic!("malformed SSE payload must surface as Err, got Ok"),
        Err(e) => e,
    };
    match err {
        SdkError::Deserialization(msg) => {
            assert!(!msg.is_empty());
            assert!(
                msg.contains("SSE") || msg.contains("invalid"),
                "msg should mention the SSE parse: {msg}"
            );
            assert!(msg.len() < 2000);
        }
        other => panic!("expected SdkError::Deserialization, got {other:?}"),
    }
}

// ─── chat_stream_resume (J-17) (2 tests) ───────────────────────────

#[tokio::test]
async fn chat_stream_resume_sends_last_event_id_header() {
    // The resume primitive must put the Last-Event-ID header on the wire
    // so the server can skip events the client already received.
    let server = MockServer::start().await;
    let body = sse_body(&[("evt-4", "world"), ("evt-5", "!")]);
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Last-Event-ID", "evt-3"))
        .and(header_exists("X-IM-Auth-Token"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let mut handle = client_for(&server)
        .chat_stream_resume(chat_request(), "evt-3")
        .await
        .expect("resume must succeed when server accepts the Last-Event-ID");
    let first = handle
        .next_chunk()
        .await
        .unwrap()
        .expect("at least one chunk");
    assert_eq!(first.id, "evt-4");
    let second = handle.next_chunk().await.unwrap().expect("second chunk");
    assert_eq!(second.id, "evt-5");
    assert_eq!(handle.last_event_id(), Some("evt-5"));
}

#[tokio::test]
async fn chat_stream_resume_propagates_non_2xx_errors() {
    // A resume that the server rejects (e.g. the requested last_event_id
    // has rolled off the server's buffer) must surface as SdkError::Api
    // just like a non-resume non-2xx response.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Last-Event-ID", "evt-stale"))
        .respond_with(ResponseTemplate::new(410).set_body_json(serde_json::json!({
            "type": "validation_error",
            "code": "MAI-4006",
            "message": "Last-Event-ID is too old to resume",
            "retry_after_seconds": null,
            "request_id": null,
        })))
        .mount(&server)
        .await;

    let err = match client_for(&server)
        .chat_stream_resume(chat_request(), "evt-stale")
        .await
    {
        Ok(_) => panic!("410 on resume must surface as Err, got Ok"),
        Err(e) => e,
    };
    match err {
        SdkError::Api(api) => {
            assert_eq!(api.error_type, MaiErrorType::ValidationError);
            assert_eq!(api.code, "MAI-4006");
            assert!(api.message.to_lowercase().contains("last-event-id"));
        }
        other => panic!("expected SdkError::Api, got {other:?}"),
    }
}
