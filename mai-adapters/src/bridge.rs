//! JSON-RPC IPC bridge for Rust-to-Python adapter communication.
//!
//! Protocol: newline-delimited JSON over stdin/stdout of the adapter subprocess.
//! Each message is a single JSON object terminated by `\n`.
//!
//! Request format:  `{"id": <u64>, "method": <string>, "params": <object>}\n`
//! Response format: `{"id": <u64>, "result": <value>}\n`
//! Error format:    `{"id": <u64>, "error": {"code": <string>, "detail": <string>}}\n`
//! Event format:    `{"event": <string>, "data": <object>}\n` (adapter -> manager, no id)

use serde::{Deserialize, Serialize};

use mai_hil::traits::{
    AdapterCapabilities, AdapterConfig, AdapterMetrics, Embedding, FinishReason,
    GenerationParams, GenerationResult, HealthStatus, Token,
};

/// JSON-RPC request from AdapterManager to adapter process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC successful response from adapter process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// JSON-RPC error payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: String,
    pub detail: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Streaming token event (adapter -> manager, no request id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    pub event: String,
    pub data: serde_json::Value,
}

/// Events the adapter can emit asynchronously.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum AdapterEvent {
    /// A streaming token.
    #[serde(rename = "token")]
    Token(Token),
    /// Stream completed.
    #[serde(rename = "stream_end")]
    StreamEnd { finish_reason: FinishReason },
    /// Heartbeat response.
    #[serde(rename = "heartbeat")]
    Heartbeat { timestamp_ms: u64 },
    /// Metrics report.
    #[serde(rename = "metrics")]
    Metrics(AdapterMetrics),
}

/// Methods the manager can invoke on the adapter process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterMethod {
    Initialize,
    Generate,
    GenerateBatch,
    Embed,
    HealthCheck,
    Capabilities,
    Shutdown,
    Heartbeat,
}

impl AdapterMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::Generate => "generate",
            Self::GenerateBatch => "generate_batch",
            Self::Embed => "embed",
            Self::HealthCheck => "health_check",
            Self::Capabilities => "capabilities",
            Self::Shutdown => "shutdown",
            Self::Heartbeat => "heartbeat",
        }
    }
}

/// Parameters for the initialize method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeParams {
    pub config: AdapterConfig,
}

/// Parameters for the generate method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateParams {
    pub prompt: String,
    pub params: GenerationParams,
    pub stream_id: u64,
}

/// Parameters for the generate_batch method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateBatchParams {
    pub prompts: Vec<String>,
    pub params: GenerationParams,
}

/// Parameters for the embed method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedParams {
    pub texts: Vec<String>,
}

/// Result types for deserialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResult {
    pub handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateBatchResult {
    pub results: Vec<GenerationResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedResult {
    pub embeddings: Vec<Embedding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub status: HealthStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitiesResult {
    pub capabilities: AdapterCapabilities,
}
