//! # MAI Agent Interface (L4 Integration Surface)
//!
//! This crate defines the interfaces that L4 agent components call to
//! interact with the MAI inference layer. It does NOT implement L4 logic;
//! it provides the typed surface for:
//!
//! - **Context Management**: Multi-turn sessions, window tracking, truncation
//! - **Tool Calling**: Tool registry, function calling protocol, chain tracking
//! - **RAG Pipeline**: Embedding interface, retrieval protocol, semantic cache
//! - **Speech-to-Text**: Audio transcription handoff, streaming STT
//! - **Agentic Tasks**: Long-running task submit/poll/cancel with budgets
//!
//! # Architecture (Tock L3-L4 Boundary)
//!
//! ```text
//! L4 Agent Components (untrusted)
//!   |
//!   | calls via typed API
//!   v
//! mai-agent (this crate, trusted boundary)
//!   |
//!   | delegates to
//!   v
//! mai-core (scheduler, registry, cache)
//! mai-vault (vector store, audit, profiles)
//! ```
//!
//! # Air-Gap Safety
//!
//! All operations are local-only. No network access. Embedding vectors,
//! audio data, and tool definitions stay on-device.

pub mod types;
pub mod context;
pub mod tools;
pub mod rag;
pub mod stt;
pub mod tasks;

// Re-export primary types for convenience
pub use context::ContextManager;
pub use rag::RagPipeline;
pub use stt::SttManager;
pub use tasks::TaskManager;
pub use tools::ToolRegistry;

pub use types::{
    // Context
    ContextConfig, ContextPriority, ContextSegment, ConversationSession,
    SegmentSource, SessionId, TokenAccounting, TruncationStrategy,
    // Tools
    ToolAccessRole, ToolAuditEntry, ToolCall, ToolChain, ToolChainState,
    ToolDefinition, ToolId, ToolResult,
    // RAG
    DocumentChunk, RagConfig, RagRequest, RagResponse, RetrievalResult,
    // STT
    AudioEncoding, AudioFormat, PartialTranscription, SttConfig,
    Transcription, WordTimestamp,
    // Tasks
    AgentTaskRequest, AgentTaskResponse, AgentTaskStatus, ResourceBudget,
    ResourceBudgetRequest, TaskConfig, TaskId, TaskProgress,
    // Errors
    AgentError,
};
