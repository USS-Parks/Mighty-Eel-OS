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

use super::{
    ChunkStream, CompletionRequest, CompletionResponse, Provider, ProviderError, StreamChunk,
    Usage, sse_stream,
};

/// An OpenAI-compatible chat provider.
pub struct OpenAiProvider {
    name: String,
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiProvider {
    /// Cloud OpenAI (`https://api.openai.com`), named `"openai"`.
    #[must_use]
    pub fn openai(api_key: impl Into<String>) -> Self {
        Self::new("openai", "https://api.openai.com", api_key)
    }

    /// A local OpenAI-compatible backend (vLLM/Ollama), named `"local"`.
    #[must_use]
    pub fn local(base_url: impl Into<String>) -> Self {
        Self::new("local", base_url, String::new())
    }

    /// A named provider at an explicit base URL (e.g. an Azure OpenAI endpoint,
    /// or a mock in tests).
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            client: super::build_http_client(),
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

    async fn post(&self, body: &Value) -> Result<reqwest::Response, ProviderError> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let mut rb = self.client.post(&url).json(body);
        if !self.api_key.is_empty() {
            rb = rb.header("authorization", format!("Bearer {}", self.api_key));
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
fn parse_sse(data: &str) -> Option<Result<StreamChunk, ProviderError>> {
    if data == "[DONE]" {
        return Some(Ok(StreamChunk {
            delta: String::new(),
            done: true,
            usage: None,
        }));
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
    let finished = v
        .pointer("/choices/0/finish_reason")
        .is_some_and(|r| !r.is_null());
    // Skip the opening role-only frame (no delta, no usage, not finished).
    if delta.is_empty() && usage.is_none() && !finished {
        return None;
    }
    Some(Ok(StreamChunk {
        delta,
        done: false,
        usage,
    }))
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        let resp = self.post(&Self::body(req, false)).await?;
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Decode(e.to_string()))?;
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
        let resp = self.post(&Self::body(req, true)).await?;
        Ok(sse_stream(resp, parse_sse))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_sentinel_and_role_frame() {
        // [DONE] → terminal frame.
        let done = parse_sse("[DONE]").unwrap().unwrap();
        assert!(done.done);
        // opening role-only frame is skipped.
        assert!(parse_sse(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#).is_none());
        // a content delta is emitted.
        let c = parse_sse(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#)
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
