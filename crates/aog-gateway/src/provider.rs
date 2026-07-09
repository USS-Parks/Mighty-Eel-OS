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

/// The shared provider HTTP client, with hang guards (D3): a `connect_timeout`
/// and an idle `read_timeout` bound a dead or stalled backend, while omitting a
/// total `timeout` keeps long SSE completions intact. The config is static, so
/// `build` only fails on a TLS-backend init fault (an unrecoverable deployment
/// error) — the same invariant `reqwest::Client::new` asserts internally.
pub(crate) fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(PROVIDER_CONNECT_TIMEOUT)
        .read_timeout(PROVIDER_READ_TIMEOUT)
        .build()
        .expect("provider HTTP client config (timeouts only) is valid")
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
pub(crate) fn sse_stream<F>(resp: reqwest::Response, parse: F) -> ChunkStream
where
    F: Fn(&str) -> Option<Result<StreamChunk, ProviderError>> + Send + 'static,
{
    let s = async_stream::stream! {
        let mut bytes = resp.bytes_stream();
        let mut buf = String::new();
        while let Some(next) = bytes.next().await {
            let chunk = match next {
                Ok(b) => b,
                Err(e) => {
                    yield Err(ProviderError::Transport(e.to_string()));
                    return;
                }
            };
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(nl) = buf.find('\n') {
                let line: String = buf.drain(..=nl).collect();
                let line = line.trim();
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
        }
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
