//! IPC bridge for Rust-to-Python adapter communication.
//!
//! Two protocol layers coexist in this module:
//!
//! **Legacy JSON-RPC** (being phased out):
//!   Request:  `{"id": <u64>, "method": <string>, "params": <object>}\n`
//!   Response: `{"id": <u64>, "result": <value>}\n`
//!
//! **NDJSON IPC Protocol v1.0**:
//!   Startup config:  JSON object on stdin (one line, no request_id)
//!   Handshake:       `{"type": "handshake", "adapter_name": ..., "capabilities": ...}\n`
//!   Request:         `{"request_id": "<uuid>", "type": "<method>", "payload": {}}\n`
//!   Response events: `{"request_id": "<uuid>", "type": "<event_type>", ...}\n`
//!
//! See `mai/docs/IPC-PROTOCOL.md` for the full specification.

use serde::{Deserialize, Serialize};

use mai_hil::traits::{
    AdapterCapabilities, AdapterConfig, AdapterMetrics, Embedding, FinishReason, GenerationParams,
    GenerationResult, HealthStatus, Token,
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

// ─── NDJSON IPC Protocol v1.0 ─────────────────────────────────

/// Startup configuration sent to the Python subprocess on stdin (first line).
/// This is NOT a request; it has no request_id. The runner reads it once at boot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcStartupConfig {
    pub adapter_name: String,
    pub module_path: String,
    pub entry_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
}

/// Handshake response from the Python subprocess (first stdout line).
/// Parsed once during process startup; not part of the request loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeResponse {
    /// Must be `"handshake"`.
    #[serde(rename = "type")]
    pub event_type: String,
    pub adapter_name: String,
    pub version: String,
    pub handle: String,
    pub capabilities: AdapterCapabilities,
}

/// IPC request written to the subprocess stdin (one JSON line per request).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    pub request_id: String,
    /// Method name: `inference`, `health`, `capabilities`, `shutdown`, `heartbeat`.
    #[serde(rename = "type")]
    pub request_type: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Raw NDJSON event read from subprocess stdout.
/// Every line in the request loop deserializes to this first, then the caller
/// inspects `event_type` to parse typed variants via `IpcEventKind`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcEvent {
    pub request_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    // Remaining fields are flattened so we can grab them by name.
    #[serde(flatten)]
    pub data: serde_json::Map<String, serde_json::Value>,
}

/// Typed variants parsed from `IpcEvent` based on `event_type`.
#[derive(Debug, Clone)]
pub enum IpcEventKind {
    /// Streaming token during inference.
    Token {
        text: String,
        logprob: Option<f32>,
        index: usize,
        finish_reason: Option<String>,
    },
    /// Token usage summary (emitted once after all tokens).
    Usage {
        prompt_tokens: u64,
        completion_tokens: u64,
    },
    /// Complete result for non-streaming methods (health, capabilities, heartbeat).
    Result { data: serde_json::Value },
    /// Request completed successfully.
    Done,
    /// Request failed.
    Error { code: String, message: String },
}

impl IpcEvent {
    /// Parse this raw event into a typed variant.
    /// Returns `Err` with a description if the event_type is unknown or fields are missing.
    pub fn parse(&self) -> Result<IpcEventKind, String> {
        match self.event_type.as_str() {
            "token" => {
                let text = self
                    .data
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let logprob = self
                    .data
                    .get("logprob")
                    .and_then(serde_json::Value::as_f64)
                    .map(|f| {
                        #[allow(clippy::cast_possible_truncation)]
                        let val = f as f32;
                        val
                    });
                #[allow(clippy::cast_possible_truncation)]
                let index = self
                    .data
                    .get("index")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0) as usize;
                let finish_reason = self
                    .data
                    .get("finish_reason")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from);
                Ok(IpcEventKind::Token {
                    text,
                    logprob,
                    index,
                    finish_reason,
                })
            }
            "usage" => {
                let prompt_tokens = self
                    .data
                    .get("prompt_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or("usage event missing prompt_tokens")?;
                let completion_tokens = self
                    .data
                    .get("completion_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or("usage event missing completion_tokens")?;
                Ok(IpcEventKind::Usage {
                    prompt_tokens,
                    completion_tokens,
                })
            }
            "result" => {
                let data = self
                    .data
                    .get("data")
                    .cloned()
                    .unwrap_or(serde_json::Value::Object(self.data.clone()));
                Ok(IpcEventKind::Result { data })
            }
            "done" => Ok(IpcEventKind::Done),
            "error" => {
                let code = self
                    .data
                    .get("code")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("InternalError")
                    .to_string();
                let message = self
                    .data
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown error")
                    .to_string();
                Ok(IpcEventKind::Error { code, message })
            }
            other => Err(format!("unknown IPC event type: {other}")),
        }
    }
}

/// Map an IPC error code string to the HIL `AdapterError` taxonomy.
pub fn ipc_error_to_adapter_error(code: &str, message: &str) -> mai_hil::traits::AdapterError {
    use mai_hil::traits::AdapterError;
    match code {
        "Timeout" => AdapterError::Timeout { timeout_ms: 0 },
        "OutOfMemory" => AdapterError::OutOfMemory,
        "ModelNotFound" => AdapterError::ModelNotFound {
            model: message.to_string(),
        },
        "BackendUnavailable" => AdapterError::BackendUnavailable,
        "ContextExceeded" => AdapterError::ContextExceeded { max_context: 0 },
        "RateLimited" => AdapterError::RateLimited,
        "HardwareFault" => AdapterError::HardwareFault {
            detail: message.to_string(),
        },
        "ValidationError" => AdapterError::ValidationError {
            reason: message.to_string(),
        },
        "UnsupportedOperation" => AdapterError::UnsupportedOperation {
            operation: message.to_string(),
        },
        // "BackendCrashed", "InternalError", and unknown codes
        _ => AdapterError::BackendCrashed,
    }
}

/// Inference payload for an IPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcInferencePayload {
    pub prompt: String,
    pub params: IpcInferenceParams,
    pub stream: bool,
}

/// Inference parameters within the IPC payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcInferenceParams {
    pub temperature: f32,
    pub top_p: f32,
    pub max_tokens: usize,
    #[serde(default)]
    pub stop_sequences: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_schema: Option<serde_json::Value>,
}

impl From<&GenerationParams> for IpcInferenceParams {
    fn from(p: &GenerationParams) -> Self {
        Self {
            temperature: p.temperature,
            top_p: p.top_p,
            max_tokens: p.max_tokens,
            stop_sequences: p.stop_sequences.clone(),
            structured_schema: p.structured_schema.clone(),
        }
    }
}

// ─── Legacy JSON-RPC types (retained for transition) ─────────────────────────

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
