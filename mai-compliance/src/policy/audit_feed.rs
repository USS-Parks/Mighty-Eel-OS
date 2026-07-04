//! Compliance audit feed.
//!
//! [`AuditFeed`] is an in-process broadcast channel for compliance
//! events. The dashboard subscribes to it; tests subscribe
//! to it; the audit log subscribes to it. The feed is
//! intentionally transport-free — exposing it over an HTTP SSE
//! endpoint is the responsibility of `mai-api`, which can wrap a
//! subscriber and forward events as `text/event-stream` lines.
//!
//! Backpressure: each subscriber holds a bounded ring buffer (default
//! 256 events). If a subscriber's buffer fills, the *oldest* events
//! are dropped and a `drops` counter increments. Producers are never
//! blocked. This matches the standard "lossy fan-out" pattern used
//! everywhere else in MAI's observability stack (see `mai-metrics`).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::composer::{AggregateDecision, ModuleId};

/// Default per-subscriber buffer depth.
pub const DEFAULT_BUFFER_CAPACITY: usize = 256;

/// One event surfaced on the feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FeedEvent {
    /// A composer evaluation completed.
    DecisionMade {
        /// Wall-clock event time, milliseconds since the Unix epoch.
        timestamp_unix_ms: u64,
        /// Request id from the originating
        /// [`super::bundle::RequestMetadata`].
        request_id: String,
        /// Tenant the request was submitted under.
        tenant_id: String,
        /// The composer's verdict.
        decision: AggregateDecision,
    },
    /// A policy configuration changed (reload, update, etc.).
    PolicyChanged {
        /// Wall-clock event time, milliseconds since the Unix epoch.
        timestamp_unix_ms: u64,
        /// Tenant whose policy changed, or `None` for global changes.
        tenant_id: Option<String>,
        /// Short description of the change for the dashboard
        /// (e.g. `"reload"`, `"template:healthcare"`).
        summary: String,
    },
    /// A module was enabled or disabled at runtime.
    ModuleStateChanged {
        /// Wall-clock event time, milliseconds since the Unix epoch.
        timestamp_unix_ms: u64,
        /// Module whose state flipped.
        module: ModuleId,
        /// New enabled state.
        enabled: bool,
    },
    /// A decision was non-allow; surfaced separately so dashboards
    /// can render the "violations" counter without scanning every
    /// `DecisionMade` event.
    ViolationDetected {
        /// Wall-clock event time, milliseconds since the Unix epoch.
        timestamp_unix_ms: u64,
        /// Request id from the originating
        /// [`super::bundle::RequestMetadata`].
        request_id: String,
        /// Tenant the request was submitted under.
        tenant_id: String,
        /// The composer's verdict (always with `allowed = false`).
        decision: AggregateDecision,
    },
}

impl FeedEvent {
    /// Wire-format kind identifier (matches the serde tag).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::DecisionMade { .. } => "decision_made",
            Self::PolicyChanged { .. } => "policy_changed",
            Self::ModuleStateChanged { .. } => "module_state_changed",
            Self::ViolationDetected { .. } => "violation_detected",
        }
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[derive(Debug)]
struct SubscriberInner {
    buffer: Mutex<VecDeque<FeedEvent>>,
    capacity: usize,
    drops: Mutex<u64>,
}

/// Handle held by a subscriber. Drop it to unsubscribe.
#[derive(Debug, Clone)]
pub struct FeedSubscriber {
    inner: Arc<SubscriberInner>,
}

impl FeedSubscriber {
    /// Drain all buffered events in arrival order.
    pub fn drain(&self) -> Vec<FeedEvent> {
        let mut guard = self.inner.buffer.lock().expect("subscriber poisoned");
        guard.drain(..).collect()
    }

    /// Pop the oldest buffered event, if any.
    pub fn pop(&self) -> Option<FeedEvent> {
        let mut guard = self.inner.buffer.lock().expect("subscriber poisoned");
        guard.pop_front()
    }

    /// Number of events currently buffered.
    pub fn len(&self) -> usize {
        self.inner.buffer.lock().expect("subscriber poisoned").len()
    }

    /// `true` if no events are buffered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Count of events dropped due to backpressure since subscribe.
    pub fn drop_count(&self) -> u64 {
        *self.inner.drops.lock().expect("subscriber poisoned")
    }

    /// Configured buffer capacity.
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }
}

#[derive(Debug, Default)]
struct FeedState {
    subscribers: Vec<Weak<SubscriberInner>>,
}

/// Lossy in-process broadcast channel for compliance events.
#[derive(Debug, Default, Clone)]
pub struct AuditFeed {
    state: Arc<Mutex<FeedState>>,
}

impl AuditFeed {
    /// Build an empty feed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe with the default buffer capacity.
    pub fn subscribe(&self) -> FeedSubscriber {
        self.subscribe_with_capacity(DEFAULT_BUFFER_CAPACITY)
    }

    /// Subscribe with a custom buffer capacity. `capacity = 0` is
    /// treated as `1` so producers never deadlock.
    pub fn subscribe_with_capacity(&self, capacity: usize) -> FeedSubscriber {
        let cap = capacity.max(1);
        let inner = Arc::new(SubscriberInner {
            buffer: Mutex::new(VecDeque::with_capacity(cap.min(64))),
            capacity: cap,
            drops: Mutex::new(0),
        });
        let mut guard = self.state.lock().expect("audit feed poisoned");
        guard.subscribers.push(Arc::downgrade(&inner));
        FeedSubscriber { inner }
    }

    /// Number of live (non-dropped) subscribers.
    pub fn subscriber_count(&self) -> usize {
        let guard = self.state.lock().expect("audit feed poisoned");
        guard
            .subscribers
            .iter()
            .filter(|w| w.strong_count() > 0)
            .count()
    }

    /// Publish a [`FeedEvent::DecisionMade`] (and a
    /// [`FeedEvent::ViolationDetected`] when `decision.allowed` is
    /// false).
    pub fn publish_decision(
        &self,
        request_id: impl Into<String>,
        tenant_id: impl Into<String>,
        decision: AggregateDecision,
    ) {
        let request_id = request_id.into();
        let tenant_id = tenant_id.into();
        let now = now_unix_ms();
        let denied = !decision.allowed;
        self.publish(&FeedEvent::DecisionMade {
            timestamp_unix_ms: now,
            request_id: request_id.clone(),
            tenant_id: tenant_id.clone(),
            decision: decision.clone(),
        });
        if denied {
            self.publish(&FeedEvent::ViolationDetected {
                timestamp_unix_ms: now,
                request_id,
                tenant_id,
                decision,
            });
        }
    }

    /// Publish a [`FeedEvent::PolicyChanged`].
    pub fn publish_policy_change(&self, tenant_id: Option<String>, summary: impl Into<String>) {
        self.publish(&FeedEvent::PolicyChanged {
            timestamp_unix_ms: now_unix_ms(),
            tenant_id,
            summary: summary.into(),
        });
    }

    /// Publish a [`FeedEvent::ModuleStateChanged`].
    pub fn publish_module_state(&self, module: ModuleId, enabled: bool) {
        self.publish(&FeedEvent::ModuleStateChanged {
            timestamp_unix_ms: now_unix_ms(),
            module,
            enabled,
        });
    }

    /// Publish any [`FeedEvent`]. Lossy: full subscribers drop the
    /// oldest event and increment their `drops` counter.
    pub fn publish(&self, event: &FeedEvent) {
        let live: Vec<Arc<SubscriberInner>> = {
            let mut guard = self.state.lock().expect("audit feed poisoned");
            // Drop expired Weak references in the same pass.
            guard.subscribers.retain(|w| w.strong_count() > 0);
            guard.subscribers.iter().filter_map(Weak::upgrade).collect()
        };
        for sub in live {
            let mut buf = sub.buffer.lock().expect("subscriber poisoned");
            if buf.len() >= sub.capacity {
                buf.pop_front();
                let mut d = sub.drops.lock().expect("subscriber poisoned");
                *d = d.saturating_add(1);
            }
            buf.push_back(event.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::composer::Destination;

    fn allow_decision() -> AggregateDecision {
        AggregateDecision {
            allowed: true,
            route: Some(Destination::Cloud),
            flags: Vec::new(),
            reasons: Vec::new(),
            modules_applied: vec![ModuleId::Hipaa],
        }
    }

    fn deny_decision() -> AggregateDecision {
        AggregateDecision {
            allowed: false,
            route: Some(Destination::Local),
            flags: Vec::new(),
            reasons: Vec::new(),
            modules_applied: vec![ModuleId::Hipaa],
        }
    }

    #[test]
    fn subscribers_receive_published_decisions() {
        let feed = AuditFeed::new();
        let sub = feed.subscribe();
        feed.publish_decision("req-1", "t-1", allow_decision());
        let events = sub.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind(), "decision_made");
    }

    #[test]
    fn deny_decision_also_emits_violation() {
        let feed = AuditFeed::new();
        let sub = feed.subscribe();
        feed.publish_decision("req-1", "t-1", deny_decision());
        let events = sub.drain();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind(), "decision_made");
        assert_eq!(events[1].kind(), "violation_detected");
    }

    #[test]
    fn multiple_subscribers_each_receive_events() {
        let feed = AuditFeed::new();
        let a = feed.subscribe();
        let b = feed.subscribe();
        feed.publish_policy_change(Some("t-1".into()), "reload");
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a.pop().unwrap().kind(), "policy_changed");
        assert_eq!(b.pop().unwrap().kind(), "policy_changed");
    }

    #[test]
    fn drop_count_increments_when_buffer_overflows() {
        let feed = AuditFeed::new();
        let sub = feed.subscribe_with_capacity(2);
        for _ in 0..5 {
            feed.publish_module_state(ModuleId::Hipaa, true);
        }
        // Capacity is 2 → 3 drops.
        assert_eq!(sub.len(), 2);
        assert_eq!(sub.drop_count(), 3);
    }

    #[test]
    fn subscriber_count_drops_when_handle_dropped() {
        let feed = AuditFeed::new();
        let sub = feed.subscribe();
        assert_eq!(feed.subscriber_count(), 1);
        drop(sub);
        // Trigger Weak-pruning by publishing.
        feed.publish_module_state(ModuleId::Hipaa, false);
        assert_eq!(feed.subscriber_count(), 0);
    }

    #[test]
    fn zero_capacity_is_clamped_to_one() {
        let feed = AuditFeed::new();
        let sub = feed.subscribe_with_capacity(0);
        assert_eq!(sub.capacity(), 1);
        feed.publish_module_state(ModuleId::Itar, true);
        feed.publish_module_state(ModuleId::Itar, false);
        assert_eq!(sub.len(), 1);
        assert_eq!(sub.drop_count(), 1);
    }
}
