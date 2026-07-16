//! Provider adapters — the code MAI never had (its "cloud" was a config label).
//!
//! One internal [`Provider`] trait in front of every model backend. Two real
//! clients implement it:
//!
//! * [`openai::OpenAiProvider`] — the OpenAI **and** local-vLLM/Ollama path.
//!   vLLM and Ollama both expose an OpenAI-compatible `/v1/chat/completions`, so
//!   a single client covers "cloud OpenAI" and "on-prem local model" by base URL
//!   — the honest way a local backend is actually reached today.
//! * [`anthropic::AnthropicProvider`] — the Anthropic `/v1/messages` path.
//!
//! The gateway speaks a **provider-neutral** request/response shape ([`CompletionRequest`]
//! / [`CompletionResponse`]); each adapter translates to/from its wire format.
//! G3 (OpenAI surface) and G4 (Anthropic surface) translate the *inbound* API to
//! this same neutral shape, so any surface can front any provider.

pub mod anthropic;
pub mod openai;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

use crate::posture::ApprovedProviderEndpoint;

/// Connect timeout for provider HTTP clients: bounds TCP+TLS establishment so a
/// dead or unroutable backend fails fast instead of hanging (audit D3).
const PROVIDER_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Idle read timeout: the maximum gap between bytes of a response. Bounds a
/// backend that connects then stalls. There is deliberately NO total request
/// `timeout` — completions stream over SSE and legitimately run long, so a total
/// timeout would truncate healthy streams; an idle timeout catches a genuine hang
/// without that. Generous enough for slow first-token / prefill latency on a large
/// local model.
const PROVIDER_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Resource and time ceilings applied to every provider response.
#[derive(Debug, Clone, Copy)]
pub struct ProviderLimits {
    pub connect_timeout: std::time::Duration,
    pub idle_timeout: std::time::Duration,
    pub total_timeout: std::time::Duration,
    pub max_headers: usize,
    pub max_header_bytes: usize,
    pub max_body_bytes: usize,
    pub max_error_body_bytes: usize,
    pub max_sse_bytes: usize,
    pub max_sse_line_bytes: usize,
    pub max_sse_frame_bytes: usize,
}

impl Default for ProviderLimits {
    fn default() -> Self {
        Self {
            connect_timeout: PROVIDER_CONNECT_TIMEOUT,
            idle_timeout: PROVIDER_READ_TIMEOUT,
            total_timeout: std::time::Duration::from_secs(15 * 60),
            max_headers: 128,
            max_header_bytes: 64 * 1024,
            max_body_bytes: 8 * 1024 * 1024,
            max_error_body_bytes: 64 * 1024,
            max_sse_bytes: 16 * 1024 * 1024,
            max_sse_line_bytes: 1024 * 1024,
            max_sse_frame_bytes: 2 * 1024 * 1024,
        }
    }
}

/// The shared provider HTTP client, with hang guards (D3): a `connect_timeout`
/// and an idle `read_timeout` bound a dead or stalled backend, while omitting a
/// total `timeout` keeps long SSE completions intact. The config is static, so
/// `build` only fails on a TLS-backend init fault (an unrecoverable deployment
/// error) — the same invariant `reqwest::Client::new` asserts internally.
pub(crate) fn build_http_client(
    endpoint: &ApprovedProviderEndpoint,
    limits: ProviderLimits,
) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(limits.connect_timeout)
        .read_timeout(limits.idle_timeout)
        .timeout(limits.total_timeout)
        // Never carry provider credentials across an upstream redirect. A
        // configured base URL is the only authorized credential destination.
        .redirect(reqwest::redirect::Policy::none());
    if let Some(host) = endpoint.dns_host() {
        builder = builder.resolve_to_addrs(host, endpoint.resolved_addrs());
    }
    builder
        .build()
        .expect("provider HTTP client config is valid")
}

/// A chat role in the neutral request model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

/// One message in a completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }
}

/// A provider-neutral completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// The upstream model id (as the chosen provider names it).
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

/// Token usage reported by the provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// A provider-neutral completion response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub model: String,
    pub content: String,
    pub usage: Usage,
    pub finish_reason: String,
}

/// One frame of a streaming completion.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamChunk {
    /// The incremental text delta (empty on a usage-only or terminal frame).
    pub delta: String,
    /// True on the final frame of the stream.
    pub done: bool,
    /// Provider-authenticated terminal reason, present only when `done` is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// Usage, when the provider reports it (usually on the terminal frame).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// A boxed stream of completion frames.
pub type ChunkStream = BoxStream<'static, Result<StreamChunk, ProviderError>>;

/// Failures reaching or decoding a provider.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// The HTTP request never completed (DNS, connect, TLS, timeout).
    #[error("transport: {0}")]
    Transport(String),
    /// The provider returned a non-2xx status.
    #[error("upstream {status}: {body}")]
    Upstream { status: u16, body: String },
    /// The provider's body could not be decoded to the expected shape.
    #[error("decode: {0}")]
    Decode(String),
    /// A configured response resource ceiling was exceeded.
    #[error("provider {resource} exceeded limit {limit}")]
    Limit {
        resource: &'static str,
        limit: usize,
    },
    /// The provider stream ended without its protocol terminal event.
    #[error("truncated provider stream: {0}")]
    Truncated(String),
}

/// The internal model-backend trait. Object-safe via `async_trait`.
#[async_trait]
pub trait Provider: Send + Sync {
    /// The provider's stable id (`"openai"`, `"anthropic"`, `"local"`, …).
    /// Borrowed, not `&'static`: a configured provider may carry a runtime name
    /// ([`OpenAiProvider`] returns a field), so literal-returning impls carry a
    /// local `unnecessary_literal_bound` allow rather than narrowing the trait.
    fn name(&self) -> &str;

    /// One-shot completion.
    async fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, ProviderError>;

    /// Streaming completion — text deltas terminated by a `done` frame.
    async fn stream(&self, req: &CompletionRequest) -> Result<ChunkStream, ProviderError>;
}

/// A name → provider lookup the gateway dispatches through.
#[derive(Default, Clone)]
pub struct Registry {
    providers: HashMap<String, Arc<dyn Provider>>,
}

impl Registry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Register a provider under its [`Provider::name`].
    pub fn register(&mut self, provider: Arc<dyn Provider>) {
        self.providers.insert(provider.name().to_string(), provider);
    }

    /// Look a provider up by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(name).cloned()
    }

    /// The registered provider names (sorted, for stable display).
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        let mut n: Vec<String> = self.providers.keys().cloned().collect();
        n.sort();
        n
    }
}

/// Turn a streaming `reqwest::Response` into a [`ChunkStream`], parsing SSE
/// `data:` frames with a provider-specific `parse` closure. `parse` returns
/// `None` to skip a frame (opening/keep-alive), `Some(Ok(chunk))` to emit, or
/// `Some(Err(..))` to surface a decode error. `event:`/`id:`/blank lines are
/// ignored — the JSON `data:` payload carries its own type discriminator.
pub(crate) fn validate_response(
    resp: &reqwest::Response,
    max_body: usize,
    limits: ProviderLimits,
) -> Result<(), ProviderError> {
    if resp.headers().len() > limits.max_headers {
        return Err(ProviderError::Limit {
            resource: "header count",
            limit: limits.max_headers,
        });
    }
    let header_bytes = resp
        .headers()
        .iter()
        .try_fold(0usize, |total, (name, value)| {
            total
                .checked_add(name.as_str().len() + value.as_bytes().len() + 4)
                .ok_or(ProviderError::Limit {
                    resource: "header bytes",
                    limit: limits.max_header_bytes,
                })
        })?;
    if header_bytes > limits.max_header_bytes {
        return Err(ProviderError::Limit {
            resource: "header bytes",
            limit: limits.max_header_bytes,
        });
    }
    if resp
        .content_length()
        .is_some_and(|length| length > max_body as u64)
    {
        return Err(ProviderError::Limit {
            resource: "declared body bytes",
            limit: max_body,
        });
    }
    Ok(())
}

async fn collect_body(
    resp: reqwest::Response,
    max: usize,
) -> Result<(Vec<u8>, bool), ProviderError> {
    let mut stream = resp.bytes_stream();
    let mut body = Vec::new();
    while let Some(next) = stream.next().await {
        let bytes = next.map_err(|error| ProviderError::Transport(error.to_string()))?;
        let remaining = max.saturating_sub(body.len());
        if bytes.len() > remaining {
            body.extend_from_slice(&bytes[..remaining]);
            return Ok((body, true));
        }
        body.extend_from_slice(&bytes);
    }
    Ok((body, false))
}

pub(crate) async fn response_json(
    resp: reqwest::Response,
    limits: ProviderLimits,
) -> Result<serde_json::Value, ProviderError> {
    let (body, truncated) = collect_body(resp, limits.max_body_bytes).await?;
    if truncated {
        return Err(ProviderError::Limit {
            resource: "body bytes",
            limit: limits.max_body_bytes,
        });
    }
    serde_json::from_slice(&body).map_err(|error| ProviderError::Decode(error.to_string()))
}

pub(crate) async fn error_body(
    resp: reqwest::Response,
    limits: ProviderLimits,
) -> Result<String, ProviderError> {
    let (body, truncated) = collect_body(resp, limits.max_error_body_bytes).await?;
    let mut text = String::from_utf8_lossy(&body).into_owned();
    if truncated {
        text.push_str(" [truncated by gateway]");
    }
    Ok(text)
}

pub(crate) fn sse_stream<F>(
    resp: reqwest::Response,
    mut parse: F,
    limits: ProviderLimits,
) -> ChunkStream
where
    F: FnMut(&str) -> Option<Result<StreamChunk, ProviderError>> + Send + 'static,
{
    let s = async_stream::stream! {
        let mut bytes = resp.bytes_stream();
        let mut buf = Vec::new();
        let mut total_bytes = 0usize;
        let mut frame_bytes = 0usize;
        while let Some(next) = bytes.next().await {
            let chunk = match next {
                Ok(b) => b,
                Err(e) => {
                    yield Err(ProviderError::Transport(e.to_string()));
                    return;
                }
            };
            total_bytes = match total_bytes.checked_add(chunk.len()) {
                Some(total) if total <= limits.max_sse_bytes => total,
                _ => {
                    yield Err(ProviderError::Limit { resource: "SSE total bytes", limit: limits.max_sse_bytes });
                    return;
                }
            };
            buf.extend_from_slice(&chunk);
            while let Some(nl) = buf.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = buf.drain(..=nl).collect();
                if line.len() > limits.max_sse_line_bytes {
                    yield Err(ProviderError::Limit { resource: "SSE line bytes", limit: limits.max_sse_line_bytes });
                    return;
                }
                frame_bytes = match frame_bytes.checked_add(line.len()) {
                    Some(total) if total <= limits.max_sse_frame_bytes => total,
                    _ => {
                        yield Err(ProviderError::Limit { resource: "SSE frame bytes", limit: limits.max_sse_frame_bytes });
                        return;
                    }
                };
                let line = match std::str::from_utf8(&line) {
                    Ok(line) => line.trim(),
                    Err(error) => {
                        yield Err(ProviderError::Decode(format!("SSE is not UTF-8: {error}")));
                        return;
                    }
                };
                if line.is_empty() {
                    frame_bytes = 0;
                    continue;
                }
                let Some(data) = line.strip_prefix("data:") else { continue };
                let data = data.trim();
                if data.is_empty() {
                    continue;
                }
                if let Some(item) = parse(data) {
                    let done = matches!(&item, Ok(c) if c.done);
                    yield item;
                    if done {
                        return;
                    }
                }
            }
            if buf.len() > limits.max_sse_line_bytes {
                yield Err(ProviderError::Limit { resource: "SSE line bytes", limit: limits.max_sse_line_bytes });
                return;
            }
        }
        yield Err(ProviderError::Truncated(
            "connection closed before a protocol terminal event".to_string(),
        ));
    };
    Box::pin(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy;
    #[async_trait]
    impl Provider for Dummy {
        #[allow(clippy::unnecessary_literal_bound)]
        fn name(&self) -> &str {
            "dummy"
        }
        async fn complete(
            &self,
            req: &CompletionRequest,
        ) -> Result<CompletionResponse, ProviderError> {
            Ok(CompletionResponse {
                model: req.model.clone(),
                content: "hi".to_string(),
                usage: Usage::default(),
                finish_reason: "stop".to_string(),
            })
        }
        async fn stream(&self, _req: &CompletionRequest) -> Result<ChunkStream, ProviderError> {
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    #[test]
    fn registry_register_get_names() {
        let mut r = Registry::new();
        r.register(Arc::new(Dummy));
        assert!(r.get("dummy").is_some());
        assert!(r.get("missing").is_none());
        assert_eq!(r.names(), vec!["dummy".to_string()]);
    }
}
