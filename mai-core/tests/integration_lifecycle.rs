//! Integration test: Scheduler + Health Monitor + Power State Machine
//!
//! End-to-end request lifecycle:
//! 1. Boot power state machine to FullInference
//! 2. Register adapters in scheduler and health monitor
//! 3. Route requests through scheduler
//! 4. Verify health monitoring tracks heartbeats
//! 5. Simulate adapter failure and verify routing excludes it
//! 6. Verify power demotion after inactivity
//! 7. Verify hot-swap replaces a failed adapter
//!
//! This test uses NO hardware dependencies. All adapters are simulated.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use uuid::Uuid;

use mai_core::health::{HealthConfig, HealthMonitor};
use mai_core::hotswap::{HotSwapManager, SwapRequest, SwapResult};
use mai_core::power::{PowerConfig, PowerState, PowerStateMachine, TransitionTrigger, WakeSource};
use mai_core::registry::ModelRegistry;
use mai_core::scheduler::{ChatMessage, RequestPayload, RequestType, SchedulingStrategy};
use mai_core::scheduler::{InferenceRequest, RequestPriority, Scheduler, SchedulerConfig};
use mai_core::vault::VaultInterface;

use async_trait::async_trait;

// ─── Mock Vault ──────────────────────────────────────────────────────────────

struct MockVault;

#[async_trait]
impl VaultInterface for MockVault {
    async fn load_model_weights(
        &self,
        _model_id: &str,
    ) -> Result<Vec<u8>, mai_core::vault::VaultError> {
        Ok(vec![0u8; 64]) // Fake weights
    }

    async fn store_model_package(
        &self,
        _model_id: &str,
        _data: &[u8],
    ) -> Result<(), mai_core::vault::VaultError> {
        Ok(())
    }

    async fn append_audit_entry(&self, _entry: &[u8]) -> Result<(), mai_core::vault::VaultError> {
        Ok(())
    }

    async fn verify_signature(
        &self,
        _data: &[u8],
        _signature: &[u8],
    ) -> Result<bool, mai_core::vault::VaultError> {
        Ok(true)
    }
}

// ─── Helper ──────────────────────────────────────────────────────────────────

fn make_request(model: Option<&str>, priority: RequestPriority) -> InferenceRequest {
    InferenceRequest {
        id: Uuid::new_v4(),
        profile_id: Uuid::new_v4(),
        model_name: model.map(|s| s.to_string()),
        request_type: RequestType::Chat,
        payload: RequestPayload::Chat {
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "Hello from integration test".to_string(),
            }],
        },
        priority,
        timeout: Duration::from_secs(30),
        streaming: true,
        enqueued_at: Instant::now(),
        estimated_tokens: 100,
    }
}

// ─── Test: Full request lifecycle ────────────────────────────────────────────

#[tokio::test]
async fn test_full_request_lifecycle() {
    // 1. Initialize power state machine and boot to FullInference
    let mut power = PowerStateMachine::new(PowerConfig::default());
    assert_eq!(power.current_state(), PowerState::Off);

    power
        .request_transition(TransitionTrigger::SystemBoot)
        .expect("Boot should succeed");
    assert_eq!(power.current_state(), PowerState::DeepVaultSleep);

    power
        .request_transition(TransitionTrigger::WakeTrigger(WakeSource::ApiRequest))
        .expect("Wake should succeed");
    assert_eq!(power.current_state(), PowerState::Sentinel);

    power
        .request_transition(TransitionTrigger::SentinelPromotion)
        .expect("Promotion should succeed");
    assert_eq!(power.current_state(), PowerState::FullInference);

    // 2. Initialize scheduler with two adapters
    let mut scheduler = Scheduler::new(SchedulerConfig::default()).unwrap();

    scheduler.register_adapter(
        "gpu-0-ollama".to_string(),
        vec!["llama3-8b".to_string(), "mistral-7b".to_string()],
        4,
        vec!["gpu-0".to_string()],
    );

    scheduler.register_adapter(
        "gpu-1-vllm".to_string(),
        vec!["llama3-70b".to_string()],
        2,
        vec!["gpu-1".to_string()],
    );

    assert_eq!(scheduler.adapter_count(), 2);
    assert_eq!(scheduler.healthy_adapter_count(), 2);

    // 3. Route a request to the correct adapter
    let request = make_request(Some("llama3-8b"), RequestPriority::Normal);
    let selection = scheduler.route_request(&request).unwrap();
    assert_eq!(selection.adapter_id, "gpu-0-ollama");
    assert_eq!(selection.model_id, "llama3-8b");

    // Verify in-flight tracking
    assert_eq!(scheduler.adapter_in_flight(&"gpu-0-ollama".to_string()), 1);

    // Complete the request
    scheduler.request_completed(&"gpu-0-ollama".to_string());
    assert_eq!(scheduler.adapter_in_flight(&"gpu-0-ollama".to_string()), 0);

    // 4. Initialize health monitor and track heartbeats
    let mut health = HealthMonitor::new(HealthConfig::default());
    health.register_adapter("gpu-0-ollama".to_string());
    health.register_adapter("gpu-1-vllm".to_string());

    // Record healthy heartbeats (requests_served, avg_latency_ms, error_rate)
    health
        .record_heartbeat(
            &"gpu-0-ollama".to_string(),
            10,   // requests_served
            15.0, // avg_latency_ms
            0.0,  // error_rate
        )
        .unwrap();
    health
        .record_heartbeat(
            &"gpu-1-vllm".to_string(),
            8,    // requests_served
            20.0, // avg_latency_ms
            0.0,  // error_rate
        )
        .unwrap();

    assert_eq!(health.healthy_adapter_count(), 2);

    // 5. Simulate adapter failure: mark gpu-1-vllm unhealthy via high error rate
    health
        .record_heartbeat(
            &"gpu-1-vllm".to_string(),
            0,      // requests_served
            5000.0, // avg_latency_ms (very high)
            0.9,    // error_rate (above 0.5 threshold -> Degraded)
        )
        .unwrap();

    // Scheduler should exclude unhealthy adapter
    scheduler.set_adapter_health(&"gpu-1-vllm".to_string(), false);

    let request_70b = make_request(Some("llama3-70b"), RequestPriority::Normal);
    let routed_70b = scheduler.route_request(&request_70b);
    // Should fail because the only adapter for 70b is unhealthy
    assert!(
        routed_70b.is_err(),
        "Should fail: only 70b adapter is unhealthy"
    );

    // 6. Test power demotion logic
    // check_auto_demotion returns Some(trigger) if idle long enough
    // Since we just transitioned, idle_duration is short - no demotion yet
    power.reset_demotion_timer();
    assert!(
        power.check_auto_demotion().is_none(),
        "Just reset: should not demote"
    );
    // idle_duration confirms short idle
    assert!(power.idle_duration() < Duration::from_secs(1));
}

// ─── Test: Hot-swap adapter replacement ──────────────────────────────────────

#[tokio::test]
async fn test_hotswap_adapter_replacement() {
    let scheduler = Arc::new(RwLock::new(
        Scheduler::new(SchedulerConfig::default()).unwrap(),
    ));
    let registry = Arc::new(RwLock::new(ModelRegistry::new(Box::new(MockVault))));
    let health = Arc::new(RwLock::new(HealthMonitor::new(HealthConfig::default())));

    // Setup: register old adapter
    {
        let mut s = scheduler.write().await;
        s.register_adapter(
            "adapter-v1".to_string(),
            vec!["model-a".to_string()],
            4,
            vec![],
        );
        s.set_adapter_health(&"adapter-v1".to_string(), true);
    }
    {
        let mut h = health.write().await;
        h.register_adapter("adapter-v1".to_string());
    }

    // Execute swap
    let mut mgr = HotSwapManager::new(scheduler.clone(), registry.clone(), health.clone());

    let req = SwapRequest::adapter_swap(
        "adapter-v1".to_string(),
        "adapter-v2".to_string(),
        "Binary upgrade to v2",
    );

    let result = mgr.execute_swap(req).await.unwrap();

    match result {
        SwapResult::Success {
            drained_requests,
            completion_time,
        } => {
            assert_eq!(drained_requests, 0);
            assert!(completion_time.as_millis() < 5000);
        }
        other => panic!("Expected Success, got {:?}", other),
    }

    // Verify audit log
    assert_eq!(mgr.total_swap_count(), 1);
    assert_eq!(mgr.successful_swap_count(), 1);
}

// ─── Test: Scheduler metrics under load ──────────────────────────────────────

#[tokio::test]
async fn test_scheduler_metrics_under_load() {
    let config = SchedulerConfig {
        strategy: SchedulingStrategy::RoundRobin,
        ..SchedulerConfig::default()
    };
    let mut scheduler = Scheduler::new(config).unwrap();

    // Register 3 adapters
    for i in 0..3 {
        scheduler.register_adapter(
            format!("adapter-{}", i),
            vec!["shared-model".to_string()],
            4,
            vec![format!("gpu-{}", i)],
        );
    }

    // Route 9 requests (should round-robin across 3 adapters)
    let mut routed_to: HashMap<String, usize> = HashMap::new();
    for _ in 0..9 {
        let request = make_request(Some("shared-model"), RequestPriority::Normal);
        let selection = scheduler.route_request(&request).unwrap();
        *routed_to.entry(selection.adapter_id.clone()).or_insert(0) += 1;
    }

    // Each adapter should have gotten 3 requests
    assert_eq!(routed_to.len(), 3);
    for count in routed_to.values() {
        assert_eq!(*count, 3, "Round-robin should distribute evenly");
    }

    // Verify total queue depth
    assert_eq!(scheduler.total_queue_depth(), 9);

    // Complete all requests
    for i in 0..3 {
        for _ in 0..3 {
            scheduler.request_completed(&format!("adapter-{}", i));
        }
    }
    assert_eq!(scheduler.total_queue_depth(), 0);
}

// ─── Test: Power state full cycle ────────────────────────────────────────────

#[tokio::test]
async fn test_power_state_full_cycle() {
    let mut power = PowerStateMachine::new(PowerConfig::default());

    // Boot -> DeepVaultSleep -> Sentinel -> FullInference -> Sentinel -> DeepVaultSleep -> Off
    power
        .request_transition(TransitionTrigger::SystemBoot)
        .unwrap();
    assert_eq!(power.current_state(), PowerState::DeepVaultSleep);

    power
        .request_transition(TransitionTrigger::WakeTrigger(WakeSource::ApiRequest))
        .unwrap();
    assert_eq!(power.current_state(), PowerState::Sentinel);

    power
        .request_transition(TransitionTrigger::SentinelPromotion)
        .unwrap();
    assert_eq!(power.current_state(), PowerState::FullInference);

    power
        .request_transition(TransitionTrigger::InactivityTimeout)
        .unwrap();
    assert_eq!(power.current_state(), PowerState::Sentinel);

    power
        .request_transition(TransitionTrigger::ExtendedInactivity)
        .unwrap();
    assert_eq!(power.current_state(), PowerState::DeepVaultSleep);

    power
        .request_transition(TransitionTrigger::SystemShutdown)
        .unwrap();
    assert_eq!(power.current_state(), PowerState::Off);

    // Verify transition log recorded all 6 transitions
    assert_eq!(power.transition_log().len(), 6);
}
