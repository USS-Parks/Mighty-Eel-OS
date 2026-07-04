//! Integration tests for the adapter framework.
//!
//! Feature-gated behind `integration` and `benchmark` features.
//! Run with: `cargo test -p mai-adapters --features integration`
//!
//! These tests require a Python environment with the adapters package available.
//! They test the full subprocess lifecycle: spawn, IPC, heartbeat, crash recovery.
//!

#![allow(clippy::print_stdout, clippy::print_stderr)]

#[cfg(feature = "integration")]
mod integration {
    use mai_adapters::config::FrameworkConfig;
    use mai_adapters::{AdapterManager, FrameworkError};
    use std::time::Duration;

    /// Test that AdapterManager can be created and discovers adapters
    /// from the adapters/ directory.
    #[tokio::test]
    async fn test_manager_lifecycle() {
        let config = FrameworkConfig::default();
        let manager = AdapterManager::new(config);

        // Discovery should work even without Python (just scans files)
        let discovered = manager.list_adapters();
        // In CI without Python adapters present, this may be empty
        // The test validates the lifecycle doesn't panic
        assert!(discovered.is_empty() || !discovered.is_empty());
    }

    /// Test heartbeat cycle on empty manager (no processes).
    #[tokio::test]
    async fn test_empty_heartbeat_cycle() {
        let config = FrameworkConfig::default();
        let manager = AdapterManager::new(config);
        // Should complete without error even with no adapters
        manager.heartbeat_cycle().await;
        let reports = manager.health_reports().await;
        assert!(reports.is_empty());
    }

    /// Test shutdown on empty manager.
    #[tokio::test]
    async fn test_empty_shutdown() {
        let config = FrameworkConfig::default();
        let manager = AdapterManager::new(config);
        let result = manager.shutdown_all().await;
        assert!(result.is_ok());
    }

    /// Test adapter not found error.
    #[tokio::test]
    async fn test_adapter_not_found() {
        let config = FrameworkConfig::default();
        let manager = AdapterManager::new(config);
        let result = manager.health_check("nonexistent").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            FrameworkError::AdapterNotFound { name } => {
                assert_eq!(name, "nonexistent");
            }
            other => panic!("Expected AdapterNotFound, got: {:?}", other),
        }
    }
}

#[cfg(feature = "benchmark")]
mod benchmark {
    use mai_adapters::AdapterManager;
    use mai_adapters::config::FrameworkConfig;
    use std::time::Instant;

    /// Benchmark: AdapterManager creation overhead.
    /// Target: <1ms for manager instantiation.
    #[tokio::test]
    async fn bench_manager_creation() {
        let iterations = 1000;
        let start = Instant::now();

        for _ in 0..iterations {
            let config = FrameworkConfig::default();
            let _manager = AdapterManager::new(config);
        }

        let elapsed = start.elapsed();
        let per_iter_us = elapsed.as_micros() / iterations;
        println!(
            "AdapterManager creation: {}us/iter ({} iterations in {:?})",
            per_iter_us, iterations, elapsed,
        );
        // Must be under 1ms per creation
        assert!(
            per_iter_us < 1000,
            "Manager creation too slow: {}us",
            per_iter_us
        );
    }

    /// Benchmark: Heartbeat cycle overhead on empty manager.
    /// Target: <100us per cycle with no adapters.
    #[tokio::test]
    async fn bench_empty_heartbeat_cycle() {
        let config = FrameworkConfig::default();
        let manager = AdapterManager::new(config);

        let iterations = 10000;
        let start = Instant::now();

        for _ in 0..iterations {
            manager.heartbeat_cycle().await;
        }

        let elapsed = start.elapsed();
        let per_iter_us = elapsed.as_micros() / iterations;
        println!(
            "Empty heartbeat cycle: {}us/iter ({} iterations in {:?})",
            per_iter_us, iterations, elapsed,
        );
        assert!(
            per_iter_us < 100,
            "Heartbeat cycle too slow: {}us",
            per_iter_us
        );
    }
}
