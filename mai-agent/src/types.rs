//! Agent interface types shared across all mai-agent modules.
//!
//! These types define the L4 integration surface: the structures that
//! L4 agent components (RAG pipeline, tool router, task orchestrator,
//! speech-to-text) use to communicate with the MAI inference layer.
//!
//! # Air-Gap Safety
//!
//! All types are local-only. No network serialization targets.
//! Embedding vectors stay on-device. Audit entries stay on-device.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use mai_core::types::{ModelId, ProfileId, RequestId};

// ============================================================================
// Type Aliases
// ============================================================================

/// Unique conversation session identifier
pub type SessionId = Uuid;
/// Unique tool definition identifier
pub type ToolId = String;
/// Unique agentic task identifier
pub type TaskId = Uuid;

// ============================================================================
// Context Management Types
// ============================================================================

/// Configuration for context window management.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    /// Maximum context window size in tokens (model-dependent default)
    pub max_context_tokens: u32,
    /// Reserved tokens for system prompt (not truncatable)
    pub system_prompt_reserve: u32,
    /// Reserved tokens for RAG injection
    pub rag_context_reserve: u32,
    /// Reserved tokens for generation output
    pub generation_reserve: u32,
    /// Truncation strategy when context exceeds window
    pub truncation_strategy: TruncationStrategy,
    /// Maximum number of conversation turns to retain
    pub max_turns: usize,
    /// Session TTL before automatic cleanup
    pub session_ttl: Duration,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 8192,
            system_prompt_reserve: 512,
            rag_context_reserve: 2048,
            generation_reserve: 1024,
            truncation_strategy: TruncationStrategy::OldestFirst,
            max_turns: 50,
            session_ttl: Duration::from_secs(3600), // 1 hour
        }
    }
}

/// Strategy for truncating context when it exceeds the model's window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TruncationStrategy {
    /// Remove oldest conversation turns first (preserve recent context)
    OldestFirst,
    /// Remove middle turns, keep first (system) and recent turns
    MiddleOut,
    /// Remove turns with lowest relevance scores (requires embedding)
    RelevanceScored,
    /// Hard cutoff: drop everything beyond the token limit
    HardCutoff,
}

/// Priority levels for context segments during truncation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ContextPriority {
    /// System prompt: never truncated
    System = 4,
    /// Tool results from current chain: high priority
    ToolResult = 3,
    /// RAG-injected context: medium-high priority
    RagContext = 2,
    /// Recent conversation turns: medium priority
    RecentTurn = 1,
    /// Older conversation turns: lowest priority, truncated first
    OlderTurn = 0,
}

/// A single segment within the assembled context window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSegment {
    /// Segment identifier for tracking
    pub id: String,
    /// Priority level (determines truncation order)
    pub priority: ContextPriority,
    /// The text content of this segment
    pub content: String,
    /// Estimated token count for this segment
    pub token_count: u32,
    /// Source of this segment
    pub source: SegmentSource,
    /// Timestamp when this segment was added
    pub added_at: u64,
}

/// Source classification for context segments.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SegmentSource {
    /// System prompt (static or per-profile)
    SystemPrompt,
    /// User message in conversation
    UserMessage,
    /// Assistant response in conversation
    AssistantMessage,
    /// RAG retrieval result injected into context
    RagRetrieval,
    /// Tool/function call result
    ToolResult,
    /// Family memory context (from FamilyVault)
    FamilyMemory,
}

/// Token usage tracking for a conversation session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenAccounting {
    /// Total tokens consumed by system prompt
    pub system_tokens: u32,
    /// Total tokens consumed by RAG context
    pub rag_tokens: u32,
    /// Total tokens consumed by conversation history
    pub history_tokens: u32,
    /// Total tokens consumed by tool results
    pub tool_tokens: u32,
    /// Remaining tokens available for generation
    pub available_tokens: u32,
    /// Total tokens consumed across all turns in this session
    pub cumulative_tokens: u64,
}

/// A multi-turn conversation session with context tracking.
#[derive(Debug, Clone)]
pub struct ConversationSession {
    /// Unique session identifier
    pub id: SessionId,
    /// Owning family profile
    pub profile_id: ProfileId,
    /// Context window configuration
    pub config: ContextConfig,
    /// Ordered context segments (system prompt, history, RAG, tools)
    pub segments: Vec<ContextSegment>,
    /// Current token accounting
    pub token_accounting: TokenAccounting,
    /// Number of conversation turns completed
    pub turn_count: u32,
    /// When this session was created
    pub created_at: Instant,
    /// When this session was last active
    pub last_active: Instant,
    /// Model used for this session (may change on promotion)
    pub model_id: Option<ModelId>,
}

// ============================================================================
// Tool Calling Types
// ============================================================================

/// Schema definition for a tool that L4 registers with the MAI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique tool identifier (e.g., "homebase.lights.set")
    pub id: ToolId,
    /// Human-readable tool name
    pub name: String,
    /// Description of what this tool does (fed to model)
    pub description: String,
    /// JSON Schema defining the tool's input parameters
    pub parameters_schema: serde_json::Value,
    /// JSON Schema defining the tool's return type
    pub return_schema: Option<serde_json::Value>,
    /// Whether this tool has side effects (affects caching)
    pub has_side_effects: bool,
    /// Maximum execution duration before timeout
    pub timeout: Duration,
    /// Required profile role to invoke this tool
    pub required_role: ToolAccessRole,
    /// Whether this tool can be called in parallel with others
    pub supports_parallel: bool,
}

/// Minimum profile role required to invoke a tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolAccessRole {
    /// Any profile including guest
    Guest = 0,
    /// Child and above
    Child = 1,
    /// Teen and above
    Teen = 2,
    /// Parent and above
    Parent = 3,
    /// Admin only
    Admin = 4,
}

/// A function call detected in model output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique call identifier (for tracking in multi-step chains)
    pub call_id: String,
    /// Tool being called
    pub tool_id: ToolId,
    /// Parsed arguments (must validate against tool's parameters_schema)
    pub arguments: serde_json::Value,
    /// Position in a multi-step chain (0-indexed)
    pub chain_step: u32,
    /// Whether this call is part of a parallel batch
    pub parallel_group: Option<String>,
}

/// Result from L4 executing a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Matching call_id from the ToolCall
    pub call_id: String,
    /// Tool that was called
    pub tool_id: ToolId,
    /// Whether the tool succeeded
    pub success: bool,
    /// Result payload (fed back to model as context)
    pub output: serde_json::Value,
    /// Error message if tool failed
    pub error: Option<String>,
    /// Execution duration
    pub duration_ms: u64,
}

/// State of a multi-step tool chain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolChainState {
    /// Waiting for model to generate next step
    AwaitingModel,
    /// Tool calls dispatched, waiting for L4 to return results
    AwaitingExecution,
    /// All steps complete, final response ready
    Complete,
    /// Chain aborted due to error or budget exhaustion
    Aborted { reason: String },
}

/// Tracks the state of a multi-step tool calling chain.
#[derive(Debug, Clone)]
pub struct ToolChain {
    /// Chain identifier (same as parent request_id)
    pub request_id: RequestId,
    /// Session this chain belongs to
    pub session_id: SessionId,
    /// Completed steps with their call/result pairs
    pub completed_steps: Vec<(ToolCall, ToolResult)>,
    /// Pending tool calls awaiting execution
    pub pending_calls: Vec<ToolCall>,
    /// Current chain state
    pub state: ToolChainState,
    /// Maximum steps allowed in this chain
    pub max_steps: u32,
    /// Total tokens consumed by tool interactions
    pub tokens_consumed: u64,
}

/// Audit entry for tool call operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolAuditEntry {
    /// When the tool was called
    pub timestamp: u64,
    /// Profile that initiated the chain
    pub profile_id: String,
    /// Tool that was called
    pub tool_id: ToolId,
    /// Call identifier
    pub call_id: String,
    /// Whether the tool succeeded
    pub success: bool,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
    /// Chain step number
    pub chain_step: u32,
    /// Error detail if failed
    pub error: Option<String>,
    /// Session context
    pub session_id: String,
}

// ============================================================================
// RAG Pipeline Types
// ============================================================================

/// Configuration for the RAG pipeline interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagConfig {
    /// Maximum batch size for embedding requests
    pub max_batch_size: usize,
    /// Default number of retrieval results
    pub default_top_k: usize,
    /// Minimum similarity score for retrieval results
    pub similarity_threshold: f32,
    /// Maximum chunk size in tokens for embedding
    pub max_chunk_tokens: usize,
    /// Semantic cache TTL
    pub semantic_cache_ttl: Duration,
    /// Semantic cache similarity threshold (for cache hit)
    pub semantic_cache_threshold: f32,
    /// Whether semantic caching is enabled
    pub semantic_cache_enabled: bool,
    /// Collection name prefix for profile isolation
    pub collection_prefix: String,
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 64,
            default_top_k: 5,
            similarity_threshold: 0.7,
            max_chunk_tokens: 512,
            semantic_cache_ttl: Duration::from_secs(1800), // 30 minutes
            semantic_cache_threshold: 0.95,
            semantic_cache_enabled: true,
            collection_prefix: "mai_rag".to_string(),
        }
    }
}

/// A document chunk prepared for embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentChunk {
    /// Source document identifier
    pub document_id: String,
    /// Chunk index within the document
    pub chunk_index: u32,
    /// Text content of this chunk
    pub text: String,
    /// Optional metadata (source path, page number, etc.)
    pub metadata: HashMap<String, String>,
}

/// A retrieval result from the RAG pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalResult {
    /// Document chunk that matched
    pub chunk: DocumentChunk,
    /// Similarity score (0.0 to 1.0 for cosine)
    pub score: f32,
    /// Embedding vector (optional, for re-ranking)
    pub embedding: Option<Vec<f32>>,
}

/// Request for RAG-augmented generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagRequest {
    /// The user's query text
    pub query: String,
    /// Profile making the request
    pub profile_id: String,
    /// Session for context continuity
    pub session_id: Option<String>,
    /// Collection to search (profile-scoped)
    pub collection: Option<String>,
    /// Number of results to retrieve
    pub top_k: Option<usize>,
    /// Minimum similarity threshold override
    pub min_score: Option<f32>,
}

/// Response from the RAG pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagResponse {
    /// Retrieved document chunks with scores
    pub retrieved: Vec<RetrievalResult>,
    /// Assembled augmented prompt (query + retrieved context)
    pub augmented_prompt: String,
    /// Whether the result came from semantic cache
    pub cache_hit: bool,
    /// Token count of the augmented prompt
    pub augmented_token_count: u32,
}

// ============================================================================
// Speech-to-Text Types
// ============================================================================

/// Configuration for the STT handoff interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttConfig {
    /// Default STT model (e.g., "whisper-large-v3")
    pub default_model: String,
    /// Audio sample rate in Hz
    pub sample_rate: u32,
    /// Audio channels (1 = mono, 2 = stereo)
    pub channels: u8,
    /// Audio bit depth (16 = PCM 16-bit)
    pub bit_depth: u8,
    /// Maximum audio duration in seconds per request
    pub max_duration_secs: u32,
    /// Whether to enable streaming transcription
    pub streaming_enabled: bool,
    /// Silence detection threshold (milliseconds of silence = end of utterance)
    pub silence_threshold_ms: u32,
    /// Language hint (ISO 639-1, None = auto-detect)
    pub language_hint: Option<String>,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            default_model: "whisper-large-v3".to_string(),
            sample_rate: 16_000,    // 16 kHz (Whisper native)
            channels: 1,            // Mono
            bit_depth: 16,          // PCM 16-bit
            max_duration_secs: 300, // 5 minutes
            streaming_enabled: true,
            silence_threshold_ms: 1500,
            language_hint: None,
        }
    }
}

/// Audio format metadata for STT input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFormat {
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Number of channels
    pub channels: u8,
    /// Bits per sample
    pub bit_depth: u8,
    /// Encoding format
    pub encoding: AudioEncoding,
}

/// Supported audio encoding formats.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AudioEncoding {
    /// Raw PCM (little-endian signed integers)
    Pcm,
    /// Ogg/Opus compressed
    Opus,
    /// FLAC lossless
    Flac,
    /// WAV container (PCM payload)
    Wav,
}

/// A partial (interim) transcription result during streaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialTranscription {
    /// Current best-guess text
    pub text: String,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f32,
    /// Detected language (ISO 639-1)
    pub language: Option<String>,
    /// Whether this is marked as final by the STT model
    pub is_final: bool,
    /// Timestamp offset in the audio stream (milliseconds)
    pub offset_ms: u64,
    /// Duration of transcribed segment (milliseconds)
    pub duration_ms: u64,
}

/// Final transcription result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcription {
    /// Full transcribed text
    pub text: String,
    /// Detected language (ISO 639-1)
    pub language: String,
    /// Overall confidence score
    pub confidence: f32,
    /// Total audio duration in milliseconds
    pub duration_ms: u64,
    /// Word-level timestamps (if available)
    pub words: Vec<WordTimestamp>,
    /// Model used for transcription
    pub model: String,
}

/// Word-level timestamp from STT output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordTimestamp {
    /// The word
    pub word: String,
    /// Start time in milliseconds
    pub start_ms: u64,
    /// End time in milliseconds
    pub end_ms: u64,
    /// Confidence for this word
    pub confidence: f32,
}

// ============================================================================
// Agentic Task Types
// ============================================================================

/// Configuration for agentic task management.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskConfig {
    /// Maximum concurrent agentic tasks per profile
    pub max_concurrent_per_profile: usize,
    /// Default token budget per task
    pub default_token_budget: u64,
    /// Default maximum tool calls per task
    pub default_max_tool_calls: u32,
    /// Default task timeout
    pub default_timeout: Duration,
    /// Maximum task timeout (hard cap)
    pub max_timeout: Duration,
    /// Progress reporting interval
    pub progress_interval: Duration,
}

impl Default for TaskConfig {
    fn default() -> Self {
        Self {
            max_concurrent_per_profile: 3,
            default_token_budget: 100_000,
            default_max_tool_calls: 50,
            default_timeout: Duration::from_secs(300), // 5 minutes
            max_timeout: Duration::from_secs(3600),    // 1 hour
            progress_interval: Duration::from_secs(5),
        }
    }
}

/// Resource budget for a single agentic task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceBudget {
    /// Maximum tokens this task may consume
    pub max_tokens: u64,
    /// Tokens consumed so far
    pub tokens_used: u64,
    /// Maximum tool calls allowed
    pub max_tool_calls: u32,
    /// Tool calls made so far
    pub tool_calls_used: u32,
    /// Maximum wall-clock duration
    pub max_duration: Duration,
    /// When the task started
    pub started_at: u64,
}

impl ResourceBudget {
    /// Check if any resource limit has been exceeded.
    pub fn is_exhausted(&self) -> bool {
        self.tokens_used >= self.max_tokens || self.tool_calls_used >= self.max_tool_calls
    }

    /// Remaining tokens in budget.
    pub fn tokens_remaining(&self) -> u64 {
        self.max_tokens.saturating_sub(self.tokens_used)
    }

    /// Remaining tool calls in budget.
    pub fn tool_calls_remaining(&self) -> u32 {
        self.max_tool_calls.saturating_sub(self.tool_calls_used)
    }
}

/// Current state of an agentic task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentTaskStatus {
    /// Task is queued, waiting to start
    Pending,
    /// Task is actively running inference/tool calls
    Running,
    /// Task is waiting for tool execution results
    AwaitingToolResults,
    /// Task completed successfully
    Completed,
    /// Task failed with error
    Failed { reason: String },
    /// Task was cancelled by user or system
    Cancelled { reason: String },
    /// Task exceeded resource budget
    BudgetExhausted { resource: String },
}

/// A progress update from a running agentic task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProgress {
    /// Task identifier
    pub task_id: String,
    /// Current status
    pub status: AgentTaskStatus,
    /// Human-readable progress message
    pub message: String,
    /// Percentage complete (0-100, best-effort estimate)
    pub percent_complete: Option<u8>,
    /// Current step in the task plan
    pub current_step: u32,
    /// Total estimated steps (may change as task progresses)
    pub total_steps: Option<u32>,
    /// Intermediate result (partial output so far)
    pub intermediate_result: Option<String>,
    /// Current resource usage
    pub budget: ResourceBudget,
}

/// Request to submit a new agentic task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTaskRequest {
    /// Task description / instruction
    pub instruction: String,
    /// Profile submitting the task
    pub profile_id: String,
    /// Session for context continuity (optional)
    pub session_id: Option<String>,
    /// Tools available for this task (subset of registered tools)
    pub available_tools: Option<Vec<ToolId>>,
    /// Custom resource budget (None = use defaults)
    pub budget: Option<ResourceBudgetRequest>,
    /// Model preference (None = scheduler picks)
    pub model: Option<String>,
    /// Whether to stream progress updates
    pub stream_progress: bool,
}

/// Resource budget overrides in a task request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceBudgetRequest {
    /// Maximum tokens (capped at config max)
    pub max_tokens: Option<u64>,
    /// Maximum tool calls (capped at config max)
    pub max_tool_calls: Option<u32>,
    /// Maximum duration in seconds (capped at config max)
    pub max_duration_secs: Option<u64>,
}

/// Response when an agentic task completes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTaskResponse {
    /// Task identifier
    pub task_id: String,
    /// Final status
    pub status: AgentTaskStatus,
    /// Final result text
    pub result: Option<String>,
    /// All tool calls made during the task
    pub tool_calls: Vec<ToolAuditEntry>,
    /// Final resource usage
    pub budget: ResourceBudget,
    /// Total wall-clock duration in milliseconds
    pub duration_ms: u64,
    /// Number of inference calls made
    pub inference_count: u32,
}

// ============================================================================
// Agent Error Types
// ============================================================================

/// Errors from the agent interface layer.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("Session expired: {0}")]
    SessionExpired(String),

    #[error("Context window exceeded: {used} tokens used, {max} available")]
    ContextOverflow { used: u32, max: u32 },

    #[error("Tool not registered: {0}")]
    ToolNotRegistered(String),

    #[error("Tool access denied: {tool_id} requires role {required:?}, profile has {actual:?}")]
    ToolAccessDenied {
        tool_id: ToolId,
        required: ToolAccessRole,
        actual: ToolAccessRole,
    },

    #[error("Tool chain exceeded max steps: {max}")]
    ChainStepLimitExceeded { max: u32 },

    #[error("Tool execution failed: {0}")]
    ToolExecutionFailed(String),

    #[error("Invalid tool arguments: {0}")]
    InvalidToolArguments(String),

    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Task budget exhausted: {resource}")]
    BudgetExhausted { resource: String },

    #[error("Too many concurrent tasks for profile: {count}/{max}")]
    TooManyConcurrentTasks { count: usize, max: usize },

    #[error("Task cancelled: {0}")]
    TaskCancelled(String),

    #[error("Audio format unsupported: {0}")]
    UnsupportedAudioFormat(String),

    #[error("Audio duration exceeded: {duration_ms}ms > {max_ms}ms")]
    AudioDurationExceeded { duration_ms: u64, max_ms: u64 },

    #[error("Audio buffer byte cap exceeded: {bytes} bytes > {max_bytes}")]
    AudioBytesExceeded { bytes: usize, max_bytes: usize },

    #[error("Malformed audio frame: {0}")]
    MalformedAudioFrame(String),

    #[error("STT model not available: {0}")]
    SttModelUnavailable(String),

    #[error("RAG collection not found: {0}")]
    CollectionNotFound(String),

    #[error("Embedding dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch { expected: usize, actual: usize },

    #[error("Scheduler error: {0}")]
    SchedulerError(String),

    #[error("Vault error: {0}")]
    VaultError(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

// ============================================================================
// Convenience conversions
// ============================================================================

impl From<mai_core::vault::VaultError> for AgentError {
    fn from(e: mai_core::vault::VaultError) -> Self {
        AgentError::VaultError(e.to_string())
    }
}

impl From<mai_core::scheduler::SchedulerError> for AgentError {
    fn from(e: mai_core::scheduler::SchedulerError) -> Self {
        AgentError::SchedulerError(e.to_string())
    }
}
