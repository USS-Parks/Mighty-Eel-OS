//! Promotion Flow — orchestrates the transition from Sentinel to Full Inference.
//!
//! When the estimator decides a request needs promotion, the promotion flow:
//! 1. Queues the request (does not drop it)
//! 2. Signals the power state machine (Sentinel -> FullInference)
//! 3. Returns a placeholder response ("Processing your request...")
//! 4. Once Full model is available: dequeues and routes through scheduler
//! 5. If promotion fails: Sentinel attempts the request with a disclaimer
//!
//! Target latency: <8 seconds from trigger to first full-model token.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::{PromoteReason, PromotionEvent};

/// Unique identifier for a queued promotion request.
pub type PromotionRequestId = uuid::Uuid;

/// A request queued during promotion.
#[derive(Debug, Clone)]
pub struct QueuedPromotionRequest {
    pub id: PromotionRequestId,
    pub original_session_id: String,
    pub model_alias: String,
    pub input_tokens: u32,
    pub reason: PromoteReason,
    pub queued_at: Instant,
    pub placeholder_sent: bool,
}

impl QueuedPromotionRequest {
    pub fn new(model_alias: String, input_tokens: u32, reason: PromoteReason) -> Self {
        Self {
            id: PromotionRequestId::new_v4(),
            original_session_id: String::new(),
            model_alias,
            input_tokens,
            reason,
            queued_at: Instant::now(),
            placeholder_sent: false,
        }
    }
}

/// Configuration for the promotion flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionConfig {
    /// Target latency from trigger to first full-model token.
    pub target_latency: Duration,
    /// Timeout for the full promotion process (power transition + model load).
    pub promotion_timeout: Duration,
    /// Placeholder response text sent to the client during promotion.
    pub placeholder: String,
    /// Maximum number of queued promotion requests.
    pub max_queue_depth: usize,
}

impl Default for PromotionConfig {
    fn default() -> Self {
        Self {
            target_latency: Duration::from_secs(8),
            promotion_timeout: Duration::from_secs(30),
            placeholder: "Processing your request...".to_string(),
            max_queue_depth: 32,
        }
    }
}

/// The promotion flow state machine.
///
/// Manages the lifecycle of a single promotion episode (batch of requests
/// that triggered promotion while transitioning from Sentinel to Full).
///
/// State machine:
///   Idle -> Promoting -> Ready -> Active -> Idle
///              |                      |
///              v                      v
///           Failed                 Idle (on completion)
///
/// - Idle: Sentinel is active, no promotion in progress
/// - Promoting: power transition in progress (Sentinel -> FullInference)
/// - Ready: full model available, queued requests can be dispatched
/// - Active: at least one request dispatched to full model
/// - Failed: promotion failed, all queued requests get fallback
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromotionFlowState {
    Idle,
    Promoting,
    Ready,
    Active,
    Failed,
}

/// Orchestrates promotion of requests from Sentinel to Full Inference.
///
/// Thread-safe: uses `Mutex` for queue access and state transitions.
pub struct PromotionFlow {
    state: Mutex<PromotionFlowState>,
    queue: Mutex<VecDeque<QueuedPromotionRequest>>,
    config: PromotionConfig,
    promotion_started: Mutex<Option<Instant>>,
    total_promotions: Mutex<u64>,
    total_fallbacks: Mutex<u64>,
}

impl PromotionFlow {
    /// Create a new promotion flow with the given config.
    pub fn new(config: PromotionConfig) -> Self {
        Self {
            state: Mutex::new(PromotionFlowState::Idle),
            queue: Mutex::new(VecDeque::new()),
            config,
            promotion_started: Mutex::new(None),
            total_promotions: Mutex::new(0),
            total_fallbacks: Mutex::new(0),
        }
    }

    /// Queue a request for promotion. Returns the queued request.
    ///
    /// If the queue is full, returns None (caller should handle overflow).
    /// If already promoting or ready, enqueues without re-triggering.
    pub fn queue_request(
        &self,
        model_alias: String,
        input_tokens: u32,
        reason: PromoteReason,
    ) -> Option<QueuedPromotionRequest> {
        let mut queue = self.queue.lock().unwrap();
        if queue.len() >= self.config.max_queue_depth {
            warn!(
                max_depth = self.config.max_queue_depth,
                "Promotion queue full, dropping request"
            );
            return None;
        }
        let req = QueuedPromotionRequest::new(model_alias, input_tokens, reason);
        queue.push_back(req.clone());
        debug!(request_id = %req.id, queue_depth = queue.len(), "Request queued for promotion");
        Some(req)
    }

    /// Start the promotion process. Called when the first request is queued
    /// and the flow is in Idle state. Returns events for the caller to act on.
    ///
    /// Call this after queueing the first request that triggered promotion.
    pub fn start_promotion(&self) -> Vec<PromotionEvent> {
        let mut state = self.state.lock().unwrap();
        if *state != PromotionFlowState::Idle {
            return vec![]; // already in progress
        }
        *state = PromotionFlowState::Promoting;
        *self.promotion_started.lock().unwrap() = Some(Instant::now());
        info!("Promotion flow started: Sentinel -> FullInference");

        let queue = self.queue.lock().unwrap();
        let events: Vec<PromotionEvent> = queue
            .iter()
            .map(|req| PromotionEvent::PromoteRequested {
                request_id: req.id.to_string(),
            })
            .collect();
        events
    }

    /// Mark the full model as ready. Queued requests can now be dispatched.
    pub fn mark_ready(&self) -> Vec<PromotionEvent> {
        let mut state = self.state.lock().unwrap();
        *state = PromotionFlowState::Ready;
        info!("Promotion flow: full model ready");

        let queue = self.queue.lock().unwrap();
        let events: Vec<PromotionEvent> = queue
            .iter()
            .map(|req| PromotionEvent::ModelReady {
                request_id: req.id.to_string(),
            })
            .collect();
        events
    }

    /// Record that a dispatched request has completed.
    pub fn record_completed(&self, request_id: &str) -> Vec<PromotionEvent> {
        let mut queue = self.queue.lock().unwrap();
        let before = queue.len();
        queue.retain(|r| r.id.to_string() != request_id);
        if queue.len() < before {
            debug!(
                request_id = request_id,
                remaining = queue.len(),
                "Promotion request completed"
            );
        }

        // If queue is empty and state was Active, transition to Idle
        if queue.is_empty() {
            let mut state = self.state.lock().unwrap();
            if *state == PromotionFlowState::Active {
                *state = PromotionFlowState::Idle;
                let mut count = self.total_promotions.lock().unwrap();
                *count += 1;
                info!("Promotion flow complete, returning to Idle");
            }
        }

        vec![PromotionEvent::RequestComplete {
            request_id: request_id.to_string(),
        }]
    }

    /// Mark the promotion as having dispatched to full model.
    pub fn mark_active(&self) {
        let mut state = self.state.lock().unwrap();
        if *state == PromotionFlowState::Ready {
            *state = PromotionFlowState::Active;
        }
    }

    /// Mark the promotion as failed. All queued requests should fall back.
    pub fn mark_failed(&self, reason: String) -> Vec<PromotionEvent> {
        let mut state = self.state.lock().unwrap();
        *state = PromotionFlowState::Failed;
        warn!(reason = %reason, "Promotion flow failed");

        let queue = self.queue.lock().unwrap();
        let events: Vec<PromotionEvent> = queue
            .iter()
            .map(|req| PromotionEvent::PromoteFailed {
                request_id: req.id.to_string(),
                reason: reason.clone(),
            })
            .collect();

        let mut count = self.total_fallbacks.lock().unwrap();
        *count += 1;

        events
    }

    /// Drain all queued requests (for fallback). Returns the drained requests.
    pub fn drain_queue(&self) -> Vec<QueuedPromotionRequest> {
        let mut queue = self.queue.lock().unwrap();
        let drained: Vec<QueuedPromotionRequest> = queue.drain(..).collect();
        drained
    }

    /// Take all queued requests for dispatch (moves them out).
    pub fn take_queue(&self) -> Vec<QueuedPromotionRequest> {
        let mut queue = self.queue.lock().unwrap();
        queue.drain(..).collect()
    }

    /// Current queue depth.
    pub fn queue_depth(&self) -> usize {
        self.queue.lock().unwrap().len()
    }

    /// Current state of the promotion flow.
    pub fn state(&self) -> PromotionFlowState {
        *self.state.lock().unwrap()
    }

    /// Check if the promotion has timed out.
    pub fn check_timeout(&self) -> bool {
        let started = self.promotion_started.lock().unwrap();
        if let Some(start) = *started {
            if start.elapsed() > self.config.promotion_timeout {
                return true;
            }
        }
        false
    }

    /// Time elapsed since promotion started.
    pub fn promotion_elapsed(&self) -> Option<Duration> {
        self.promotion_started
            .lock()
            .unwrap()
            .map(|start| start.elapsed())
    }

    /// Total completed promotions (reset on Idle transitions).
    pub fn total_promotions(&self) -> u64 {
        *self.total_promotions.lock().unwrap()
    }

    /// Total fallback events.
    pub fn total_fallbacks(&self) -> u64 {
        *self.total_fallbacks.lock().unwrap()
    }

    /// Whether the flow is in the process of promoting (not yet ready).
    pub fn is_promoting(&self) -> bool {
        matches!(*self.state.lock().unwrap(), PromotionFlowState::Promoting)
    }

    /// Whether the full model is ready for dispatch.
    pub fn is_ready(&self) -> bool {
        matches!(
            *self.state.lock().unwrap(),
            PromotionFlowState::Ready | PromotionFlowState::Active
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_flow() -> PromotionFlow {
        PromotionFlow::new(PromotionConfig::default())
    }

    fn sample_request(flow: &PromotionFlow) -> QueuedPromotionRequest {
        flow.queue_request(
            "lamprey/fast".to_string(),
            1000,
            PromoteReason::TooManyTokens {
                input_tokens: 1000,
                threshold: 500,
            },
        )
        .unwrap()
    }

    #[test]
    fn test_initial_state_idle() {
        let flow = default_flow();
        assert_eq!(flow.state(), PromotionFlowState::Idle);
        assert_eq!(flow.queue_depth(), 0);
    }

    #[test]
    fn test_queue_request() {
        let flow = default_flow();
        let req = sample_request(&flow);
        assert_eq!(flow.queue_depth(), 1);
        assert_eq!(req.input_tokens, 1000);
    }

    #[test]
    fn test_start_promotion() {
        let flow = default_flow();
        sample_request(&flow);
        let events = flow.start_promotion();
        assert_eq!(flow.state(), PromotionFlowState::Promoting);
        assert!(!events.is_empty());
    }

    #[test]
    fn test_start_promotion_idempotent() {
        let flow = default_flow();
        sample_request(&flow);
        let _ = flow.start_promotion();
        // Second call while Promoting should return empty
        let events = flow.start_promotion();
        assert!(events.is_empty());
    }

    #[test]
    fn test_mark_ready() {
        let flow = default_flow();
        sample_request(&flow);
        flow.start_promotion();
        let events = flow.mark_ready();
        assert_eq!(flow.state(), PromotionFlowState::Ready);
        assert!(!events.is_empty());
    }

    #[test]
    fn test_full_cycle() {
        let flow = default_flow();
        let req = sample_request(&flow);

        flow.start_promotion();
        assert!(flow.is_promoting());

        flow.mark_ready();
        assert!(flow.is_ready());

        flow.mark_active();
        assert_eq!(flow.state(), PromotionFlowState::Active);

        flow.record_completed(&req.id.to_string());
        assert_eq!(flow.state(), PromotionFlowState::Idle);
        assert_eq!(flow.queue_depth(), 0);
    }

    #[test]
    fn test_mark_failed() {
        let flow = default_flow();
        sample_request(&flow);
        flow.start_promotion();
        let events = flow.mark_failed("GPU error".to_string());
        assert_eq!(flow.state(), PromotionFlowState::Failed);
        assert!(!events.is_empty());
        assert_eq!(flow.total_fallbacks(), 1);
    }

    #[test]
    fn test_drain_queue() {
        let flow = default_flow();
        sample_request(&flow);
        sample_request(&flow);
        assert_eq!(flow.queue_depth(), 2);
        let drained = flow.drain_queue();
        assert_eq!(drained.len(), 2);
        assert_eq!(flow.queue_depth(), 0);
    }

    #[test]
    fn test_queue_overflow() {
        let config = PromotionConfig {
            max_queue_depth: 2,
            ..PromotionConfig::default()
        };
        let flow = PromotionFlow::new(config);
        assert!(
            flow.queue_request("a".to_string(), 10, PromoteReason::Embedding)
                .is_some()
        );
        assert!(
            flow.queue_request("b".to_string(), 10, PromoteReason::Embedding)
                .is_some()
        );
        assert!(
            flow.queue_request("c".to_string(), 10, PromoteReason::Embedding)
                .is_none()
        );
    }

    #[test]
    fn test_check_timeout() {
        let config = PromotionConfig {
            promotion_timeout: Duration::from_millis(1),
            ..PromotionConfig::default()
        };
        let flow = PromotionFlow::new(config);
        sample_request(&flow);
        flow.start_promotion();
        std::thread::sleep(Duration::from_millis(5));
        assert!(flow.check_timeout());

        // No timeout when not promoting
        let flow2 = default_flow();
        assert!(!flow2.check_timeout());
    }

    #[test]
    fn test_multiple_requests_completed_individually() {
        let flow = default_flow();
        let req_a = sample_request(&flow);
        let req_b = flow
            .queue_request("b".to_string(), 10, PromoteReason::Embedding)
            .unwrap();

        flow.start_promotion();
        flow.mark_ready();
        flow.mark_active();

        flow.record_completed(&req_a.id.to_string());
        assert_eq!(flow.state(), PromotionFlowState::Active); // still active
        assert_eq!(flow.queue_depth(), 1);

        flow.record_completed(&req_b.id.to_string());
        assert_eq!(flow.state(), PromotionFlowState::Idle); // all done
    }

    #[test]
    fn test_promotion_elapsed() {
        let flow = default_flow();
        assert!(flow.promotion_elapsed().is_none());
        sample_request(&flow);
        flow.start_promotion();
        assert!(flow.promotion_elapsed().is_some());
    }
}
