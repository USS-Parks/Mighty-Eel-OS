//! Sentinel Mode - Always-on low-power intelligence with promotion path.
//!
//! Sentinel mode runs a small model (e.g., Phi-4-mini, ~3.8B params) on minimal
//! resources to handle basic tasks: simple Q&A, smart home commands, calendar
//! reminders, wakeword detection. When a request exceeds Sentinel's capability,
//! it triggers promotion to Full Inference via the power state machine.
//!
//! # Architecture
//!
//! - `SentinelConfig`: per-tier configuration (model, thresholds, budgets)
//! - `RequestComplexity`: classification result from the estimator
//! - `PromotionState`: state machine for promotion lifecycle
//! - `PromotionEvent`: hook points for scheduling integration
//!
//! The estimator and runtime are pure logic in this crate. The actual
//! scheduler/power orchestration lives in `mai-scheduler::sentinel`.

use serde::{Deserialize, Serialize};

pub mod estimator;
pub mod promotion;
pub mod runtime;
pub mod warmup;

pub use estimator::{RequestComplexityEstimator, RequestFeatures, TaskKind};
pub use promotion::{PromotionFlow, PromotionFlowState, PromotionRequestId};
pub use runtime::SentinelRuntime;
pub use warmup::{WarmupDecider, WarmupStrategy};

/// Per-product-tier Sentinel configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentinelConfig {
    /// Model identifier for the sentinel model (e.g., "phi-4-mini").
    pub model: String,
    /// Maximum input tokens sentinel can handle before promotion.
    pub promote_threshold_tokens: u32,
    /// Maximum output tokens sentinel is allowed to generate.
    pub max_output_tokens: u32,
    /// VRAM budget in bytes for the sentinel model.
    pub vram_budget_bytes: u64,
    /// Whether the sentinel runs on CPU only (no GPU).
    pub cpu_only: bool,
    /// Sizing hint for the sentinel model (used for adapter selection).
    pub model_size_label: String,
}

impl Default for SentinelConfig {
    fn default() -> Self {
        Self {
            model: "phi-4-mini".to_string(),
            promote_threshold_tokens: 500,
            max_output_tokens: 256,
            vram_budget_bytes: 4 * 1_073_741_824, // 4 GiB
            cpu_only: false,
            model_size_label: "small".to_string(),
        }
    }
}

/// Pre-defined product tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProductTier {
    Scout,
    Ranger,
    PackLeader,
}

impl ProductTier {
    pub fn config(self) -> SentinelConfig {
        match self {
            Self::Scout => SentinelConfig {
                model: "phi-4-mini".to_string(),
                promote_threshold_tokens: 500,
                max_output_tokens: 256,
                vram_budget_bytes: 4 * 1_073_741_824,
                cpu_only: false,
                model_size_label: "small".to_string(),
            },
            Self::Ranger => SentinelConfig {
                model: "phi-4-mini".to_string(),
                promote_threshold_tokens: 800,
                max_output_tokens: 512,
                vram_budget_bytes: 4 * 1_073_741_824,
                cpu_only: false,
                model_size_label: "small".to_string(),
            },
            Self::PackLeader => SentinelConfig {
                model: "gemma-4-12b".to_string(),
                promote_threshold_tokens: 1200,
                max_output_tokens: 1024,
                vram_budget_bytes: 12 * 1_073_741_824,
                cpu_only: false,
                model_size_label: "medium".to_string(),
            },
        }
    }
}

/// Result of request complexity estimation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Complexity {
    /// Sentinel can handle this request directly.
    SentinelOnly,
    /// Promote to Full Inference (with reason).
    Promote(PromoteReason),
}

/// Why a request requires promotion to Full Inference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromoteReason {
    /// Input exceeds token threshold.
    TooManyTokens { input_tokens: u32, threshold: u32 },
    /// Task type requires full model.
    TaskType(crate::sentinel::estimator::TaskKind),
    /// Request explicitly specifies a large model.
    ExplicitModel { requested: String },
    /// Embedding request (sentinel doesn't do embeddings).
    Embedding,
}

/// State of the promotion lifecycle for a single request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromotionState {
    /// Request classified as needing promotion, queued.
    Queued,
    /// Power transition in progress (Sentinel -> FullInference).
    Transitioning,
    /// FullInference active, request dispatched to scheduler.
    Dispatched,
    /// Request completed on full model.
    Completed,
    /// Promotion failed; fallback to sentinel.
    Fallback,
}

/// Hook events emitted during promotion lifecycle.
/// The scheduler uses these to coordinate adapter/power actions.
#[derive(Debug, Clone)]
pub enum PromotionEvent {
    /// Promotion triggered for this request.
    PromoteRequested { request_id: String },
    /// Full model is ready, request can be dispatched.
    ModelReady { request_id: String },
    /// Promotion failed, fallback to sentinel.
    PromoteFailed { request_id: String, reason: String },
    /// Request completed (any outcome).
    RequestComplete { request_id: String },
}
