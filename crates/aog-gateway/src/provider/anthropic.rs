//! Anthropic provider — the `/v1/messages` path.
//!
//! Differs from the OpenAI shape in three ways this adapter handles: system
//! turns move to a top-level `system` field (Anthropic messages are user/assistant
//! only), `max_tokens` is mandatory (defaulted when the neutral request omits it),
//! and streaming is event-typed (`content_block_delta`, `message_delta`,
//! `message_stop`) rather than `[DONE]`-terminated.

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::posture::ApprovedProviderEndpoint;

use super::{
    ChunkStream, CompletionRequest, CompletionResponse, Provider, ProviderError, ProviderLimits,
    Role, StreamChunk, Usage, error_body, response_json, sse_stream, validate_response,
};

/// Anthropic's mandatory default when the caller sets no cap.
const DEFAULT_MAX_TOKENS: u32 = 1024;
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// An Anthropic Messages provider.
pub struct AnthropicProvider {
    endpoint: ApprovedProviderEndpoint,
    api_key: String,
    client: reqwest::Client,
    limits: ProviderLimits,
}

impl AnthropicProvider {
    /// A policy-approved, DNS-pinned Anthropic-compatible endpoint.
    #[must_use]
    pub fn new(endpoint: ApprovedProviderEndpoint, api_key: impl Into<String>) -> Self {
        Self::new_with_limits(endpoint, api_key, ProviderLimits::default())
    }

    /// Construct with explicit limits (used by adversarial provider gates).
    #[must_use]
    pub fn new_with_limits(
        endpoint: ApprovedProviderEndpoint,
        api_key: impl Into<String>,
        limits: ProviderLimits,
    ) -> Self {
        let client = super::build_http_client(&endpoint, limits);
        Self {
            endpoint,
            api_key: api_key.into(),
            client,
            limits,
        }
    }

    fn body(req: &CompletionRequest, stream: bool) -> Value {
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

    async fn post(&self, body: &Value, stream: bool) -> Result<reqwest::Response, ProviderError> {
        let url = self.endpoint.request_url("v1/messages");
        let mut rb = self
            .client
            .post(url)
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
        let max_body = if status.is_success() {
            if stream {
                self.limits.max_sse_bytes
            } else {
                self.limits.max_body_bytes
            }
        } else {
            self.limits.max_error_body_bytes
        };
        validate_response(&resp, max_body, self.limits)?;
        if !status.is_success() {
            let body = error_body(resp, self.limits).await?;
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
fn parse_sse(
    data: &str,
    finish_reason: &mut Option<String>,
) -> Option<Result<StreamChunk, ProviderError>> {
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
                finish_reason: None,
                usage: None,
            }))
        }
        "message_start" => Some(Ok(StreamChunk {
            delta: String::new(),
            done: false,
            finish_reason: None,
            usage: Some(Usage {
                input_tokens: u32_at(&v, "/message/usage/input_tokens"),
                output_tokens: 0,
            }),
        })),
        "message_delta" => {
            if let Some(reason) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                *finish_reason = Some(reason.to_string());
            }
            Some(Ok(StreamChunk {
                delta: String::new(),
                done: false,
                finish_reason: None,
                usage: Some(Usage {
                    input_tokens: 0,
                    output_tokens: u32_at(&v, "/usage/output_tokens"),
                }),
            }))
        }
        "message_stop" => Some(finish_reason.take().map_or_else(
            || {
                Err(ProviderError::Truncated(
                    "Anthropic message_stop arrived without stop_reason".to_string(),
                ))
            },
            |reason| {
                Ok(StreamChunk {
                    delta: String::new(),
                    done: true,
                    finish_reason: Some(reason),
                    usage: None,
                })
            },
        )),
        // ping, content_block_start, content_block_stop — nothing to emit.
        _ => None,
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        let resp = self.post(&Self::body(req, false), false).await?;
        let v = response_json(resp, self.limits).await?;
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
        let resp = self.post(&Self::body(req, true), true).await?;
        let mut finish_reason = None;
        Ok(sse_stream(
            resp,
            move |data| parse_sse(data, &mut finish_reason),
            self.limits,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_split_and_mandatory_max_tokens() {
        let req = CompletionRequest {
            model: "claude".into(),
            messages: vec![
                super::super::ChatMessage::system("be terse"),
                super::super::ChatMessage::user("hi"),
            ],
            max_tokens: None,
            temperature: None,
        };
        let b = AnthropicProvider::body(&req, false);
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
        let mut finish = None;
        assert!(parse_sse(r#"{"type":"ping"}"#, &mut finish).is_none());
        let d = parse_sse(
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}}"#,
            &mut finish,
        )
        .unwrap()
        .unwrap();
        assert_eq!(d.delta, "Hel");
        finish = Some("end_turn".to_string());
        assert!(
            parse_sse(r#"{"type":"message_stop"}"#, &mut finish)
                .unwrap()
                .unwrap()
                .done
        );
    }
}
