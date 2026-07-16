//! OpenAI-compatible provider — the OpenAI **and** local vLLM/Ollama path.
//!
//! One client, two roles, chosen by `base_url`:
//! * `https://api.openai.com` → cloud OpenAI.
//! * `http://127.0.0.1:11434` (Ollama) / a vLLM server → the **local** backend.
//!
//! Both expose the identical `/v1/chat/completions` contract (JSON + `text/event-stream`),
//! so "route to a local model" is just an [`OpenAiProvider`] with a local base
//! URL and the name `"local"`.

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::posture::ApprovedProviderEndpoint;

use super::{
    ChunkStream, CompletionRequest, CompletionResponse, Provider, ProviderError, ProviderLimits,
    StreamChunk, Usage, error_body, response_json, sse_stream, validate_response,
};

/// An OpenAI-compatible chat provider.
pub struct OpenAiProvider {
    name: String,
    endpoint: ApprovedProviderEndpoint,
    api_key: String,
    client: reqwest::Client,
    limits: ProviderLimits,
}

impl OpenAiProvider {
    /// A local OpenAI-compatible backend (vLLM/Ollama), named `"local"`.
    #[must_use]
    pub fn local(endpoint: ApprovedProviderEndpoint) -> Self {
        Self::new("local", endpoint, String::new())
    }

    /// A named provider at a policy-approved, DNS-pinned endpoint.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        endpoint: ApprovedProviderEndpoint,
        api_key: impl Into<String>,
    ) -> Self {
        Self::new_with_limits(name, endpoint, api_key, ProviderLimits::default())
    }

    /// Construct with explicit limits (used by adversarial provider gates).
    #[must_use]
    pub fn new_with_limits(
        name: impl Into<String>,
        endpoint: ApprovedProviderEndpoint,
        api_key: impl Into<String>,
        limits: ProviderLimits,
    ) -> Self {
        let client = super::build_http_client(&endpoint, limits);
        Self {
            name: name.into(),
            endpoint,
            api_key: api_key.into(),
            client,
            limits,
        }
    }

    fn body(req: &CompletionRequest, stream: bool) -> Value {
        let mut body = json!({
            "model": req.model,
            "messages": req.messages,
            "stream": stream,
        });
        if let Some(m) = req.max_tokens {
            body["max_tokens"] = json!(m);
        }
        if let Some(t) = req.temperature {
            body["temperature"] = json!(t);
        }
        if stream {
            // Ask compatible servers to emit a final usage frame.
            body["stream_options"] = json!({ "include_usage": true });
        }
        body
    }

    async fn post(&self, body: &Value, stream: bool) -> Result<reqwest::Response, ProviderError> {
        let url = self.endpoint.request_url("v1/chat/completions");
        let mut rb = self.client.post(url).json(body);
        if !self.api_key.is_empty() {
            rb = rb.header("authorization", format!("Bearer {}", self.api_key));
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

fn openai_usage(v: &Value) -> Option<Usage> {
    let u = v.get("usage")?;
    if u.is_null() {
        return None;
    }
    Some(Usage {
        input_tokens: u32_at(u, "/prompt_tokens"),
        output_tokens: u32_at(u, "/completion_tokens"),
    })
}

/// Parse one OpenAI SSE `data:` payload into a [`StreamChunk`].
fn parse_sse(
    data: &str,
    finish_reason: &mut Option<String>,
) -> Option<Result<StreamChunk, ProviderError>> {
    if data == "[DONE]" {
        return Some(finish_reason.take().map_or_else(
            || {
                Err(ProviderError::Truncated(
                    "OpenAI [DONE] arrived without finish_reason".to_string(),
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
        ));
    }
    let v: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => return Some(Err(ProviderError::Decode(e.to_string()))),
    };
    let delta = v
        .pointer("/choices/0/delta/content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let usage = openai_usage(&v);
    let reason = v
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(reason) = &reason {
        *finish_reason = Some(reason.clone());
    }
    // Skip the opening role-only frame (no delta, no usage, not finished).
    if delta.is_empty() && usage.is_none() && reason.is_none() {
        return None;
    }
    Some(Ok(StreamChunk {
        delta,
        done: false,
        finish_reason: None,
        usage,
    }))
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        let resp = self.post(&Self::body(req, false), false).await?;
        let v = response_json(resp, self.limits).await?;
        Ok(CompletionResponse {
            model: v
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or(&req.model)
                .to_string(),
            content: v
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            usage: openai_usage(&v).unwrap_or_default(),
            finish_reason: v
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                .unwrap_or("stop")
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
    fn done_sentinel_and_role_frame() {
        // [DONE] → terminal frame.
        let mut finish = Some("stop".to_string());
        let done = parse_sse("[DONE]", &mut finish).unwrap().unwrap();
        assert!(done.done);
        // opening role-only frame is skipped.
        assert!(parse_sse(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#, &mut None).is_none());
        // a content delta is emitted.
        let c = parse_sse(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#, &mut None)
            .unwrap()
            .unwrap();
        assert_eq!(c.delta, "Hel");
        assert!(!c.done);
    }

    #[test]
    fn usage_frame_parsed() {
        let u = openai_usage(&json!({"usage":{"prompt_tokens":7,"completion_tokens":3}})).unwrap();
        assert_eq!(
            u,
            Usage {
                input_tokens: 7,
                output_tokens: 3
            }
        );
    }
}
