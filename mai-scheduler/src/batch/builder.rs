//! Continuous batch builder: the core orchestrator for inference batching.
//!
//! Each model instance gets one `BatchBuilder`. On every inference iteration
//! the engine calls `build_step()`, which:
//!
//! 1. Removes completed sequences from the active batch.
//! 2. Checks emergency preemption (VRAM > 95%).
//! 3. Drains the waiting queue through admission control.
//! 4. Returns a `BatchDecision` describing what changed.
//!
//! The batch builder does NOT own the KV cache manager or the GPU. It
//! receives VRAM state as input and returns decisions for the caller to
//! execute. This keeps it testable without hardware.

use std::collections::VecDeque;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::batch::admission::{AdmissionConfig, AdmissionController, AdmissionDecision};
use crate::batch::metrics::{BatchMetrics, MetricsConfig};
use crate::batch::preemption::{
    PreemptionCandidate, PreemptionConfig, PreemptionPolicy,
};
use crate::types::{Priority, SequenceId};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Top-level batch configuration. Loaded from config/batch.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    /// Maximum sequences in the active batch simultaneously.
    #[serde(default = "default_max_batch_size")]
    pub max_batch_size: u32,

    /// Maximum sequences allowed in the waiting queue before rejection.
    #[serde(default = "default_max_queue_depth")]
    pub max_queue_depth: u32,

    /// Admission control configuration.
    #[serde(default)]
    pub admission: AdmissionConfig,

    /// Preemption policy configuration.
    #[serde(default)]
    pub preemption: PreemptionConfig,

    /// Metrics collection configuration.
    #[serde(default)]
    pub metrics: MetricsConfig,
}

fn default_max_batch_size() -> u32 {
    16
}

fn default_max_queue_depth() -> u32 {
    128
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_batch_size: default_max_batch_size(),
            max_queue_depth: default_max_queue_depth(),
            admission: AdmissionConfig::default(),
            preemption: PreemptionConfig::default(),
            metrics: MetricsConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Queued request
// ---------------------------------------------------------------------------

/// A request waiting in the queue for batch admission.
#[derive(Debug, Clone)]
pub struct QueuedRequest {
    /// Sequence identifier.
    pub seq_id: SequenceId,
    /// Model this request targets (must match instance model).
    pub model: String,
    /// Estimated prompt token count.
    pub prompt_tokens: u32,
    /// Maximum tokens to generate.
    pub max_tokens: u32,
    /// Request priority.
    pub priority: Priority,
    /// Estimated KV cache bytes for this sequence.
    pub estimated_kv_bytes: u64,
    /// When this request entered the queue.
    pub enqueued_at: Instant,
}

// ---------------------------------------------------------------------------
// Active batch member
// ---------------------------------------------------------------------------

/// A sequence currently in the active batch (generating tokens).
#[derive(Debug, Clone)]
pub struct ActiveSequence {
    /// Sequence identifier.
    pub seq_id: SequenceId,
    /// Request priority.
    pub priority: Priority,
    /// Tokens generated so far.
    pub generated_tokens: u32,
    /// Maximum tokens to generate.
    pub max_tokens: u32,
    /// KV cache bytes consumed.
    pub kv_bytes: u64,
    /// Whether this sequence has finished generation.
    pub completed: bool,
}

impl ActiveSequence {
    /// Completion progress as a fraction (0.0 = just started, 1.0 = done).
    pub fn completion_progress(&self) -> f64 {
        if self.max_tokens == 0 {
            return 1.0;
        }
        (self.generated_tokens as f64 / self.max_tokens as f64).min(1.0)
    }
}

// ---------------------------------------------------------------------------
// Batch decision (output of build_step)
// ---------------------------------------------------------------------------

/// The result of a single batch build step. Tells the caller what changed.
#[derive(Debug, Clone, Default)]
pub struct BatchDecision {
    /// Sequences newly admitted to the active batch this step.
    pub admitted: Vec<SequenceId>,
    /// Sequences that completed generation and were removed.
    pub completed: Vec<SequenceId>,
    /// Sequences preempted (emergency eviction from active batch).
    pub preempted: Vec<SequenceId>,
    /// Sequences that need KV eviction before they can be admitted.
    /// The caller should evict idle KV sequences, then re-queue these.
    pub needs_eviction: Vec<SequenceId>,
    /// Current active batch size after this step.
    pub active_batch_size: u32,
    /// Current waiting queue depth after this step.
    pub waiting_queue_depth: u32,
}

// ---------------------------------------------------------------------------
// VRAM state (input to build_step)
// ---------------------------------------------------------------------------

/// Current VRAM state, provided by the caller each build step.
/// The batch builder does not directly query hardware.
#[derive(Debug, Clone)]
pub struct VramState {
    /// Current VRAM used in bytes.
    pub used_bytes: u64,
    /// Total VRAM available in bytes.
    pub total_bytes: u64,
    /// Whether the KV cache manager reports room for a new sequence.
    /// The caller checks `kv_manager.can_fit()` and passes the result.
    pub kv_can_fit: bool,
}

impl VramState {
    /// VRAM usage as a fraction (0.0..1.0).
    pub fn usage_fraction(&self) -> f64 {
        if self.total_bytes == 0 {
            return 1.0;
        }
        self.used_bytes as f64 / self.total_bytes as f64
    }
}

// ---------------------------------------------------------------------------
// BatchBuilder
// ---------------------------------------------------------------------------

/// Per-instance batch builder. Manages the active batch and waiting queue
/// for a single model instance.
pub struct BatchBuilder {
    /// Active batch: sequences currently generating tokens.
    active_batch: Vec<ActiveSequence>,

    /// Waiting queue: requests waiting for admission, ordered by priority
    /// then arrival time.
    waiting_queue: VecDeque<QueuedRequest>,

    /// Maximum batch size for this instance.
    max_batch_size: u32,

    /// Maximum queue depth.
    max_queue_depth: u32,

    /// The model this instance serves. Used for compatibility checks.
    model: String,

    /// Admission controller.
    admission: AdmissionController,

    /// Preemption policy.
    preemption: PreemptionPolicy,

    /// Per-instance metrics.
    metrics: BatchMetrics,
}

impl BatchBuilder {
    /// Create a new batch builder for an instance.
    pub fn new(model: impl Into<String>, config: BatchConfig) -> Self {
        let max_batch = config.max_batch_size;
        let max_queue = config.max_queue_depth;
        let metrics = BatchMetrics::new(max_batch, config.metrics);

        Self {
            active_batch: Vec::with_capacity(max_batch as usize),
            waiting_queue: VecDeque::with_capacity(64),
            max_batch_size: max_batch,
            max_queue_depth: max_queue,
            model: model.into(),
            admission: AdmissionController::new(config.admission),
            preemption: PreemptionPolicy::new(config.preemption),
            metrics,
        }
    }

    /// Enqueue a request for batch admission.
    ///
    /// Returns `false` if the queue is full (request should be rejected
    /// by the caller). System priority requests always enqueue.
    pub fn enqueue(&mut self, request: QueuedRequest) -> bool {
        if request.model != self.model {
            warn!(
                expected = %self.model,
                got = %request.model,
                "Model mismatch in enqueue, rejecting"
            );
            return false;
        }

        if self.waiting_queue.len() >= self.max_queue_depth as usize
            && request.priority != Priority::System
        {
            debug!(
                queue_depth = self.waiting_queue.len(),
                max = self.max_queue_depth,
                "Queue full, rejecting request"
            );
            self.metrics.record_rejections(1);
            return false;
        }

        self.waiting_queue.push_back(request);
        true
    }

    /// Mark a sequence as completed. It will be removed in the next
    /// `build_step()` call.
    pub fn mark_completed(&mut self, seq_id: SequenceId) {
        if let Some(active) = self
            .active_batch
            .iter_mut()
            .find(|s| s.seq_id == seq_id)
        {
            active.completed = true;
        }
    }

    /// Update the generated token count for an active sequence.
    pub fn update_progress(&mut self, seq_id: SequenceId, generated_tokens: u32) {
        if let Some(active) = self
            .active_batch
            .iter_mut()
            .find(|s| s.seq_id == seq_id)
        {
            active.generated_tokens = generated_tokens;
        }
    }

    /// Execute one batch build step. Called each inference iteration.
    ///
    /// This is the heart of continuous batching. The sequence:
    /// 1. Remove completed sequences.
    /// 2. Check for emergency preemption if VRAM is critical.
    /// 3. Drain waiting queue through admission control.
    /// 4. Record metrics and return what changed.
    pub fn build_step(&mut self, vram: &VramState) -> BatchDecision {
        let mut decision = BatchDecision::default();

        // --- Phase 1: Remove completed sequences ---
        let _before_count = self.active_batch.len();
        let completed: Vec<SequenceId> = self
            .active_batch
            .iter()
            .filter(|s| s.completed)
            .map(|s| s.seq_id)
            .collect();

        self.active_batch.retain(|s| !s.completed);
        decision.completed = completed;

        if !decision.completed.is_empty() {
            self.metrics
                .record_completions(decision.completed.len() as u64);
            debug!(
                removed = decision.completed.len(),
                "Completed sequences removed from batch"
            );
        }

        // --- Phase 2: Emergency preemption ---
        let vram_frac = vram.usage_fraction();
        if self.preemption.is_emergency(vram_frac) && !self.active_batch.is_empty() {
            let candidates: Vec<PreemptionCandidate> = self
                .active_batch
                .iter()
                .map(|s| PreemptionCandidate {
                    seq_id: s.seq_id,
                    priority: s.priority,
                    completion_progress: s.completion_progress(),
                    kv_bytes: s.kv_bytes,
                })
                .collect();

            // Try to free enough for at least one queued request
            let needed = self
                .waiting_queue
                .front()
                .map(|r| r.estimated_kv_bytes)
                .unwrap_or(0);

            let result = self
                .preemption
                .select_victims(vram_frac, &candidates, needed);

            if !result.victims.is_empty() {
                for victim_id in &result.victims {
                    self.active_batch.retain(|s| s.seq_id != *victim_id);
                }
                decision.preempted = result.victims;
            }
        }

        // --- Phase 3: Admit from waiting queue ---
        let mut admitted_count = 0_u64;
        let mut eviction_admission_count = 0_u64;

        // Process queue front-to-back. We use an index to avoid borrow issues.
        let mut i = 0;
        while i < self.waiting_queue.len() {
            let batch_has_room =
                (self.active_batch.len() as u32) < self.max_batch_size;

            if !batch_has_room {
                break; // Batch is full, stop trying
            }

            let request = &self.waiting_queue[i];
            let admit_decision = self.admission.check(
                vram_frac,
                request.prompt_tokens,
                request.priority,
                vram.kv_can_fit,
                batch_has_room,
            );

            match admit_decision {
                AdmissionDecision::Admit => {
                    let request = self.waiting_queue.remove(i).unwrap();
                    let wait_time = request.enqueued_at.elapsed();
                    self.metrics.record_wait_time(wait_time);

                    let seq = ActiveSequence {
                        seq_id: request.seq_id,
                        priority: request.priority,
                        generated_tokens: 0,
                        max_tokens: request.max_tokens,
                        kv_bytes: request.estimated_kv_bytes,
                        completed: false,
                    };
                    self.active_batch.push(seq);
                    decision.admitted.push(request.seq_id);
                    admitted_count += 1;
                    // Don't increment i: removal shifted elements left
                }
                AdmissionDecision::AdmitAfterEviction => {
                    let request = self.waiting_queue.remove(i).unwrap();
                    decision.needs_eviction.push(request.seq_id);
                    eviction_admission_count += 1;
                    // Don't increment i
                }
                AdmissionDecision::Reject => {
                    i += 1; // Skip, try next
                }
            }
        }

        if admitted_count > 0 {
            self.metrics.record_admissions(admitted_count);
        }
        if eviction_admission_count > 0 {
            self.metrics
                .record_eviction_admissions(eviction_admission_count);
        }

        // Record rejected count: items still in queue that weren't admitted
        // (only count those we actually evaluated and rejected this step)
        let rejected_this_step = self.waiting_queue.len() as u64;
        if rejected_this_step > 0 && admitted_count == 0 && eviction_admission_count == 0 {
            // Only record rejections if we had items but admitted none
            self.metrics.record_rejections(rejected_this_step);
        }

        // --- Phase 4: Record metrics ---
        self.metrics.record_step(self.active_batch.len() as u32);

        decision.active_batch_size = self.active_batch.len() as u32;
        decision.waiting_queue_depth = self.waiting_queue.len() as u32;

        decision
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Current active batch size.
    pub fn active_batch_size(&self) -> u32 {
        self.active_batch.len() as u32
    }

    /// Current waiting queue depth.
    pub fn waiting_queue_depth(&self) -> u32 {
        self.waiting_queue.len() as u32
    }

    /// The model this builder serves.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Maximum batch size.
    pub fn max_batch_size(&self) -> u32 {
        self.max_batch_size
    }

    /// Access the metrics tracker (for snapshot reads).
    pub fn metrics(&self) -> &BatchMetrics {
        &self.metrics
    }

    /// Access the active batch (read-only, for scoring/inspection).
    pub fn active_sequences(&self) -> &[ActiveSequence] {
        &self.active_batch
    }

    /// Check if a sequence is in the active batch.
    pub fn is_active(&self, seq_id: SequenceId) -> bool {
        self.active_batch.iter().any(|s| s.seq_id == seq_id)
    }

    /// Update admission control config at runtime.
    pub fn update_admission_config(&mut self, config: AdmissionConfig) {
        self.admission.update_config(config);
    }

    /// Update preemption config at runtime.
    pub fn update_preemption_config(&mut self, config: PreemptionConfig) {
        self.preemption.update_config(config);
    }
}

impl std::fmt::Debug for BatchBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchBuilder")
            .field("model", &self.model)
            .field("active_batch", &self.active_batch.len())
            .field("waiting_queue", &self.waiting_queue.len())
            .field("max_batch_size", &self.max_batch_size)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_builder() -> BatchBuilder {
        BatchBuilder::new("llama3-8b", BatchConfig::default())
    }

    fn make_vram(used: u64, total: u64, kv_fit: bool) -> VramState {
        VramState {
            used_bytes: used,
            total_bytes: total,
            kv_can_fit: kv_fit,
        }
    }

    fn make_request(priority: Priority, tokens: u32) -> QueuedRequest {
        QueuedRequest {
            seq_id: SequenceId::new(),
            model: "llama3-8b".to_string(),
            prompt_tokens: tokens,
            max_tokens: 2048,
            priority,
            estimated_kv_bytes: 100_000_000,
            enqueued_at: Instant::now(),
        }
    }

    #[test]
    fn test_enqueue_and_admit() {
        let mut builder = make_builder();
        let req = make_request(Priority::Normal, 256);
        let seq_id = req.seq_id;

        assert!(builder.enqueue(req));
        assert_eq!(builder.waiting_queue_depth(), 1);

        // Low VRAM pressure, KV fits -> should admit
        let vram = make_vram(4_000_000_000, 10_000_000_000, true);
        let decision = builder.build_step(&vram);

        assert_eq!(decision.admitted.len(), 1);
        assert_eq!(decision.admitted[0], seq_id);
        assert_eq!(builder.active_batch_size(), 1);
        assert_eq!(builder.waiting_queue_depth(), 0);
    }

    #[test]
    fn test_model_mismatch_rejected() {
        let mut builder = make_builder();
        let mut req = make_request(Priority::Normal, 256);
        req.model = "wrong-model".to_string();

        assert!(!builder.enqueue(req));
        assert_eq!(builder.waiting_queue_depth(), 0);
    }

    #[test]
    fn test_queue_full_rejects_non_system() {
        let config = BatchConfig {
            max_queue_depth: 2,
            ..Default::default()
        };
        let mut builder = BatchBuilder::new("llama3-8b", config);

        assert!(builder.enqueue(make_request(Priority::Normal, 256)));
        assert!(builder.enqueue(make_request(Priority::Normal, 256)));
        // Queue full
        assert!(!builder.enqueue(make_request(Priority::Normal, 256)));
        // System bypasses
        assert!(builder.enqueue(make_request(Priority::System, 256)));
    }

    #[test]
    fn test_completed_sequences_removed() {
        let mut builder = make_builder();
        let req = make_request(Priority::Normal, 256);
        let seq_id = req.seq_id;
        builder.enqueue(req);

        let vram = make_vram(4_000_000_000, 10_000_000_000, true);
        builder.build_step(&vram);
        assert_eq!(builder.active_batch_size(), 1);

        builder.mark_completed(seq_id);
        let decision = builder.build_step(&vram);

        assert_eq!(decision.completed.len(), 1);
        assert_eq!(decision.completed[0], seq_id);
        assert_eq!(builder.active_batch_size(), 0);
    }

    #[test]
    fn test_batch_size_limit() {
        let config = BatchConfig {
            max_batch_size: 2,
            ..Default::default()
        };
        let mut builder = BatchBuilder::new("llama3-8b", config);

        for _ in 0..5 {
            builder.enqueue(make_request(Priority::Normal, 256));
        }

        let vram = make_vram(4_000_000_000, 10_000_000_000, true);
        let decision = builder.build_step(&vram);

        // Only 2 admitted (max batch size)
        assert_eq!(decision.admitted.len(), 2);
        assert_eq!(builder.active_batch_size(), 2);
        assert_eq!(builder.waiting_queue_depth(), 3);
    }

    #[test]
    fn test_selective_mode_rejects_long_normal() {
        let mut builder = make_builder();
        // Long sequence, Normal priority
        builder.enqueue(make_request(Priority::Normal, 4096));

        // VRAM at 85% -> selective mode, long + Normal = reject
        let vram = make_vram(8_500_000_000, 10_000_000_000, true);
        let decision = builder.build_step(&vram);

        assert!(decision.admitted.is_empty());
        assert_eq!(builder.waiting_queue_depth(), 1);
    }

    #[test]
    fn test_selective_mode_admits_high_priority() {
        let mut builder = make_builder();
        builder.enqueue(make_request(Priority::High, 4096));

        let vram = make_vram(8_500_000_000, 10_000_000_000, true);
        let decision = builder.build_step(&vram);

        assert_eq!(decision.admitted.len(), 1);
    }

    #[test]
    fn test_eviction_required_region() {
        let mut builder = make_builder();
        builder.enqueue(make_request(Priority::Normal, 256));

        // VRAM at 92% -> eviction required
        let vram = make_vram(9_200_000_000, 10_000_000_000, false);
        let decision = builder.build_step(&vram);

        // Should go to needs_eviction path
        assert_eq!(decision.needs_eviction.len(), 1);
        assert!(decision.admitted.is_empty());
    }

    #[test]
    fn test_preemption_at_emergency() {
        let config = BatchConfig {
            max_batch_size: 4,
            ..Default::default()
        };
        let mut builder = BatchBuilder::new("llama3-8b", config);

        // Manually put sequences in active batch
        for _ in 0..3 {
            let req = make_request(Priority::Normal, 256);
            builder.enqueue(req);
        }
        let vram_ok = make_vram(4_000_000_000, 10_000_000_000, true);
        builder.build_step(&vram_ok);
        assert_eq!(builder.active_batch_size(), 3);

        // Add a waiting request, then hit emergency VRAM
        builder.enqueue(make_request(Priority::Normal, 256));
        let vram_emergency = make_vram(9_600_000_000, 10_000_000_000, false);
        let decision = builder.build_step(&vram_emergency);

        // Should have preempted at least one sequence
        assert!(
            !decision.preempted.is_empty(),
            "Expected preemption at 96% VRAM"
        );
    }

    #[test]
    fn test_completion_progress() {
        let seq = ActiveSequence {
            seq_id: SequenceId::new(),
            priority: Priority::Normal,
            generated_tokens: 500,
            max_tokens: 1000,
            kv_bytes: 100_000_000,
            completed: false,
        };
        assert!((seq.completion_progress() - 0.5).abs() < f64::EPSILON);

        let done = ActiveSequence {
            generated_tokens: 1000,
            max_tokens: 1000,
            ..seq.clone()
        };
        assert!((done.completion_progress() - 1.0).abs() < f64::EPSILON);

        let zero_max = ActiveSequence {
            max_tokens: 0,
            ..seq
        };
        assert!((zero_max.completion_progress() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_update_progress() {
        let mut builder = make_builder();
        let req = make_request(Priority::Normal, 256);
        let seq_id = req.seq_id;
        builder.enqueue(req);

        let vram = make_vram(4_000_000_000, 10_000_000_000, true);
        builder.build_step(&vram);

        builder.update_progress(seq_id, 100);
        let active = builder
            .active_sequences()
            .iter()
            .find(|s| s.seq_id == seq_id)
            .unwrap();
        assert_eq!(active.generated_tokens, 100);
    }

    #[test]
    fn test_is_active() {
        let mut builder = make_builder();
        let req = make_request(Priority::Normal, 256);
        let seq_id = req.seq_id;

        assert!(!builder.is_active(seq_id));
        builder.enqueue(req);
        assert!(!builder.is_active(seq_id)); // still in queue

        let vram = make_vram(4_000_000_000, 10_000_000_000, true);
        builder.build_step(&vram);
        assert!(builder.is_active(seq_id)); // now in active batch
    }

    #[test]
    fn test_metrics_snapshot_after_steps() {
        let mut builder = make_builder();
        for _ in 0..3 {
            builder.enqueue(make_request(Priority::Normal, 256));
        }

        let vram = make_vram(4_000_000_000, 10_000_000_000, true);
        builder.build_step(&vram);

        let snap = builder.metrics().snapshot();
        assert_eq!(snap.total_steps, 1);
        assert_eq!(snap.total_admitted, 3);
        assert!(snap.avg_batch_size > 0.0);
    }

    #[test]
    fn test_vram_state_fraction() {
        let vram = make_vram(8_000_000_000, 10_000_000_000, true);
        assert!((vram.usage_fraction() - 0.8).abs() < f64::EPSILON);

        let zero = make_vram(0, 0, false);
        assert!((zero.usage_fraction() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_empty_build_step() {
        let mut builder = make_builder();
        let vram = make_vram(4_000_000_000, 10_000_000_000, true);
        let decision = builder.build_step(&vram);

        assert!(decision.admitted.is_empty());
        assert!(decision.completed.is_empty());
        assert!(decision.preempted.is_empty());
        assert_eq!(decision.active_batch_size, 0);
    }
}
