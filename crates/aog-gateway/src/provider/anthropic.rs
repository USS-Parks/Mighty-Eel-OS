//! Anthropic provider — the `/v1/messages` path.
//!
//! Differs from the OpenAI shape in three ways this adapter handles: system
//! turns move to a top-level `system` field (Anthropic messages are user/assistant
//! only), `max_tokens` is mandatory (defaulted when the neutral request omits it),
//! and streaming is event-typed (`content_block_delta`, `message_delta`,
//! `message_stop`) rather than `[DONE]`-terminated.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{
    ChunkStream, CompletionRequest, CompletionResponse, Provider, ProviderError, Role, StreamChunk,
    Usage, sse_stream,
};

/// Anthropic's mandatory default when the caller sets no cap.
const DEFAULT_MAX_TOKENS: u32 = 1024;
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// An Anthropic Messages provider.
pub struct AnthropicProvider {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    /// Cloud Anthropic (`https://api.anthropic.com`).
    #[must_use]
    pub fn anthropic(api_key: impl Into<String>) -> Self {
        Self::new("https://api.anthropic.com", api_key)
    }

    /// An explicit base URL (e.g. a mock in tests).
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            client: reqwest::Client::new(),
        }
    }

    fn body(&self, req: &CompletionRequest, stream: bool) -> Value {
        let mut system = String::new();
        let mut messages = Vec::new();
        for m in &req.messages {
            match m.role {
                Role::System => {
                    if !system.is_empty() {
                        system.push('\n');
                    }
                    system.push_str(&m.content);
                }
                Role::User => messages.push(json!({ "role": "user", "content": m.content })),
                Role::Assistant => {
                    messages.push(json!({ "role": "assistant", "content": m.content }));
                }
            }
        }
        let mut body = json!({
            "model": req.model,
            "max_tokens": req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            "messages": messages,
            "stream": stream,
        });
        if !system.is_empty() {
            body["system"] = json!(system);
        }
        if let Some(t) = req.temperature {
            body["temperature"] = json!(t);
        }
        body
    }

    async fn post(&self, body: &Value) -> Result<reqwest::Response, ProviderError> {
        let url = format!("{}/v1/messages", self.base_url);
        let mut rb = self
            .client
            .post(&url)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(body);
        if !self.api_key.is_empty() {
            rb = rb.header("x-api-key", &self.api_key);
        }
        let resp = rb
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Upstream {
                status: status.as_u16(),
                body,
            });
        }
        Ok(resp)
    }
}

fn u32_at(v: &Value, ptr: &str) -> u32 {
    v.pointer(ptr)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}

/// Parse one Anthropic SSE `data:` payload, dispatching on the event `type`.
fn parse_sse(data: &str) -> Option<Result<StreamChunk, ProviderError>> {
    let v: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => return Some(Err(ProviderError::Decode(e.to_string()))),
    };
    match v.get("type").and_then(Value::as_str).unwrap_or_default() {
        "content_block_delta" => {
            let delta = v
                .pointer("/delta/text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            (!delta.is_empty()).then_some(Ok(StreamChunk {
                delta,
                done: false,
                usage: None,
            }))
        }
        "message_start" => Some(Ok(StreamChunk {
            delta: String::new(),
            done: false,
            usage: Some(Usage {
                input_tokens: u32_at(&v, "/message/usage/input_tokens"),
                output_tokens: 0,
            }),
        })),
        "message_delta" => Some(Ok(StreamChunk {
            delta: String::new(),
            done: false,
            usage: Some(Usage {
                input_tokens: 0,
                output_tokens: u32_at(&v, "/usage/output_tokens"),
            }),
        })),
        "message_stop" => Some(Ok(StreamChunk {
            delta: String::new(),
            done: true,
            usage: None,
        })),
        // ping, content_block_start, content_block_stop — nothing to emit.
        _ => None,
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        let resp = self.post(&self.body(req, false)).await?;
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Decode(e.to_string()))?;
        let content = v
            .get("content")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        Ok(CompletionResponse {
            model: v
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or(&req.model)
                .to_string(),
            content,
            usage: Usage {
                input_tokens: u32_at(&v, "/usage/input_tokens"),
                output_tokens: u32_at(&v, "/usage/output_tokens"),
            },
            finish_reason: v
                .get("stop_reason")
                .and_then(Value::as_str)
                .unwrap_or("end_turn")
                .to_string(),
        })
    }

    async fn stream(&self, req: &CompletionRequest) -> Result<ChunkStream, ProviderError> {
        let resp = self.post(&self.body(req, true)).await?;
        Ok(sse_stream(resp, parse_sse))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_split_and_mandatory_max_tokens() {
        let p = AnthropicProvider::new("http://x", "k");
        let req = CompletionRequest {
            model: "claude".into(),
            messages: vec![
                super::super::ChatMessage::system("be terse"),
                super::super::ChatMessage::user("hi"),
            ],
            max_tokens: None,
            temperature: None,
        };
        let b = p.body(&req, false);
        assert_eq!(b["system"], "be terse");
        assert_eq!(b["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(
            b["messages"].as_array().unwrap().len(),
            1,
            "system not in messages"
        );
        assert_eq!(b["messages"][0]["role"], "user");
    }

    #[test]
    fn sse_event_dispatch() {
        assert!(parse_sse(r#"{"type":"ping"}"#).is_none());
        let d = parse_sse(
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(d.delta, "Hel");
        assert!(
            parse_sse(r#"{"type":"message_stop"}"#)
                .unwrap()
                .unwrap()
                .done
        );
    }
}
