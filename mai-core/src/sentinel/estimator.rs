//! Request Complexity Estimator.
//!
//! Decides whether the Sentinel model can handle a request or whether it
//! should be promoted to Full Inference. The estimator must complete in
//! < 10ms to avoid adding perceptible latency to the response path.

use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{Complexity, PromoteReason, SentinelConfig};

/// Classification of the task type from request metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    /// Simple question-answering (sentinel-capable).
    SimpleQa,
    /// Smart home command (sentinel-capable).
    SmartHome,
    /// Calendar/reminder (sentinel-capable).
    Calendar,
    /// Wakeword detection (sentinel-capable).
    Wakeword,
    /// Complex reasoning or analysis (needs promotion).
    ComplexReasoning,
    /// Long-form content generation (needs promotion).
    LongForm,
    /// Multi-step tool calling (needs promotion).
    MultiStepTool,
    /// Embedding generation (needs promotion).
    Embedding,
    /// Unknown/unclassified — conservatively promote.
    Unknown,
}

/// Input features used by the estimator to classify a request.
#[derive(Debug, Clone)]
pub struct RequestFeatures {
    /// Number of input tokens (estimated or actual).
    pub input_tokens: u32,
    /// Requested model alias.
    pub requested_model: Option<String>,
    /// Whether the request is an embedding.
    pub is_embedding: bool,
    /// Detected or declared task kind.
    pub task_kind: TaskKind,
    /// Whether the request declares specific capabilities needed.
    pub requires_function_calling: bool,
    /// Optional prompt text for simple heuristics.
    pub prompt_preview: Option<String>,
}

/// The request complexity estimator.
///
/// Uses a simple rule-based classifier (not ML) to ensure < 10ms decision
/// time. The estimator checks token count, task type, model requirements,
/// and embedding requests against the SentinelConfig thresholds.
pub struct RequestComplexityEstimator {
    config: SentinelConfig,
}

impl RequestComplexityEstimator {
    /// Create a new estimator for the given Sentinel configuration.
    pub fn new(config: SentinelConfig) -> Self {
        Self { config }
    }

    /// Classify a request: can Sentinel handle it, or should we promote?
    ///
    /// Returns in < 10ms. Uses simple heuristics only:
    /// 1. Embedding requests always promote
    /// 2. Requests for specific large models promote
    /// 3. Token count above threshold promotes
    /// 4. Task type heuristics promote complex tasks
    /// 5. Everything else stays on Sentinel
    pub fn classify(&self, features: &RequestFeatures) -> Complexity {
        // 1. Embedding check (fastest path)
        if features.is_embedding {
            debug!("Sentinel: embedding request -> promote");
            return Complexity::Promote(PromoteReason::Embedding);
        }

        // 2. Explicit model check
        if let Some(ref model) = features.requested_model {
            if self.is_large_model(model) {
                debug!(model = %model, "Sentinel: explicit large model -> promote");
                return Complexity::Promote(PromoteReason::ExplicitModel {
                    requested: model.clone(),
                });
            }
        }

        // 3. Token threshold check
        if features.input_tokens > self.config.promote_threshold_tokens {
            debug!(
                tokens = features.input_tokens,
                threshold = self.config.promote_threshold_tokens,
                "Sentinel: token threshold exceeded -> promote"
            );
            return Complexity::Promote(PromoteReason::TooManyTokens {
                input_tokens: features.input_tokens,
                threshold: self.config.promote_threshold_tokens,
            });
        }

        // 4. Task type classification
        match features.task_kind {
            TaskKind::ComplexReasoning
            | TaskKind::LongForm
            | TaskKind::MultiStepTool
            | TaskKind::Embedding
            | TaskKind::Unknown => {
                debug!(task = ?features.task_kind, "Sentinel: task type requires promotion");
                return Complexity::Promote(PromoteReason::TaskType(features.task_kind));
            }
            TaskKind::SimpleQa | TaskKind::SmartHome | TaskKind::Calendar | TaskKind::Wakeword => {
                // Sentinel-capable tasks stay here
            }
        }

        // 5. Function calling on sentinel — promote if sentinel lacks it
        if features.requires_function_calling {
            debug!("Sentinel: function calling requested -> promote");
            return Complexity::Promote(PromoteReason::TaskType(features.task_kind));
        }

        debug!("Sentinel: request classified as sentinel-only");
        Complexity::SentinelOnly
    }

    /// Update the configuration (e.g., after tier change).
    pub fn set_config(&mut self, config: SentinelConfig) {
        self.config = config;
    }

    /// Access the current configuration.
    pub fn config(&self) -> &SentinelConfig {
        &self.config
    }

    /// Simple heuristic: is this model name a "large" model that needs full inference?
    fn is_large_model(&self, model: &str) -> bool {
        let lower = model.to_lowercase();
        // Large model indicators in naming conventions
        lower.contains("70b")
            || lower.contains("120b")
            || lower.contains("180b")
            || lower.contains("405b")
            || lower.contains("671b")
            || lower.contains("deepseek")
            || lower.contains("gpt-4")
            || lower.contains("claude-3")
            || lower.starts_with("qwen3-72")
            || lower.starts_with("qwen3-120")
            || lower.starts_with("llama3-70")
            || lower.starts_with("llama3-405")
            || lower.starts_with("mixtral-8x22")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_estimator() -> RequestComplexityEstimator {
        RequestComplexityEstimator::new(SentinelConfig::default())
    }

    fn simple_qa() -> RequestFeatures {
        RequestFeatures {
            input_tokens: 50,
            requested_model: None,
            is_embedding: false,
            task_kind: TaskKind::SimpleQa,
            requires_function_calling: false,
            prompt_preview: Some("What time is it?".to_string()),
        }
    }

    #[test]
    fn test_simple_question_stays_on_sentinel() {
        let est = default_estimator();
        assert_eq!(est.classify(&simple_qa()), Complexity::SentinelOnly);
    }

    #[test]
    fn test_embedding_promotes() {
        let est = default_estimator();
        let features = RequestFeatures {
            is_embedding: true,
            ..simple_qa()
        };
        assert_eq!(
            est.classify(&features),
            Complexity::Promote(PromoteReason::Embedding)
        );
    }

    #[test]
    fn test_large_model_promotes() {
        let est = default_estimator();
        let features = RequestFeatures {
            requested_model: Some("qwen3-70b".to_string()),
            ..simple_qa()
        };
        let result = est.classify(&features);
        assert!(matches!(
            result,
            Complexity::Promote(PromoteReason::ExplicitModel { .. })
        ));
    }

    #[test]
    fn test_token_threshold_promotes() {
        let est = default_estimator();
        let features = RequestFeatures {
            input_tokens: 1000,
            ..simple_qa()
        };
        let result = est.classify(&features);
        assert!(matches!(
            result,
            Complexity::Promote(PromoteReason::TooManyTokens { .. })
        ));
    }

    #[test]
    fn test_below_threshold_stays() {
        let est = default_estimator();
        let features = RequestFeatures {
            input_tokens: 100, // well below 500 default threshold
            ..simple_qa()
        };
        assert_eq!(est.classify(&features), Complexity::SentinelOnly);
    }

    #[test]
    fn test_complex_reasoning_promotes() {
        let est = default_estimator();
        let features = RequestFeatures {
            task_kind: TaskKind::ComplexReasoning,
            ..simple_qa()
        };
        let result = est.classify(&features);
        assert!(matches!(
            result,
            Complexity::Promote(PromoteReason::TaskType(_))
        ));
    }

    #[test]
    fn test_multi_step_tool_promotes() {
        let est = default_estimator();
        let features = RequestFeatures {
            requires_function_calling: true,
            ..simple_qa()
        };
        let result = est.classify(&features);
        assert!(matches!(
            result,
            Complexity::Promote(PromoteReason::TaskType(_))
        ));
    }

    #[test]
    fn test_smart_home_stays_on_sentinel() {
        let est = default_estimator();
        let features = RequestFeatures {
            task_kind: TaskKind::SmartHome,
            ..simple_qa()
        };
        assert_eq!(est.classify(&features), Complexity::SentinelOnly);
    }

    #[test]
    fn test_large_model_heuristic() {
        let est = default_estimator();
        assert!(est.is_large_model("qwen3-70b"));
        assert!(est.is_large_model("llama3-405b-instruct"));
        assert!(est.is_large_model("deepseek-v3"));
        assert!(!est.is_large_model("phi-4-mini"));
        assert!(!est.is_large_model("llama3-8b"));
        assert!(!est.is_large_model("nomic-embed-text"));
    }

    #[test]
    fn test_classify_under_10ms() {
        let est = default_estimator();
        let features = simple_qa();
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            let _ = est.classify(&features);
        }
        let avg_us = start.elapsed().as_micros() / 1000;
        assert!(
            avg_us < 100,
            "avg {avg_us} us per classify (target <10ms = 10000us)"
        );
    }

    #[test]
    fn test_unknown_task_kind_promotes() {
        let est = default_estimator();
        let features = RequestFeatures {
            task_kind: TaskKind::Unknown,
            ..simple_qa()
        };
        let result = est.classify(&features);
        assert!(matches!(result, Complexity::Promote(_)));
    }
}
