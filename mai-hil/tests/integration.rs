//! Integration tests for MAI HIL drivers.
//!
//! These tests exercise real hardware detection and are gated behind the
//! `integration` feature flag. They require:
//! - Linux system (for procfs/sysfs access)
//! - Optionally: NVIDIA GPU with driver installed (for nvidia tests)
//! - Optionally: AMD GPU with rocm-smi installed (for amd tests)
//!
//! Run with: `cargo test --features integration -p mai-hil`
//! Run CPU-only: `cargo test --features integration -p mai-hil -- cpu`

#![cfg(feature = "integration")]

use mai_hil::drivers::CpuDriver;
use mai_hil::traits::{HardwareProbe, MemoryManager, PowerState, PowerStateController};

// ============================================================================
// CPU Driver Integration Tests (always pass on Linux)
// ============================================================================

#[tokio::test]
async fn cpu_discover_devices_returns_valid_descriptor() {
    let driver = CpuDriver::new();
    let devices = driver.discover_devices().await;
    assert!(devices.is_ok(), "CPU discovery must succeed on Linux: {:?}", devices.err());

    let descriptors = devices.unwrap();
    assert_eq!(descriptors.len(), 1, "CPU driver should report exactly one device");

    let desc = &descriptors[0];
    assert!(!desc.model_name.is_empty(), "CPU model name must not be empty");
    assert_ne!(desc.model_name, "Unknown CPU", "Should detect actual CPU model");
    assert!(desc.total_memory_bytes > 0, "Total memory must be > 0");
    assert!(
        desc.total_memory_bytes > 512 * 1024 * 1024,
        "System should have > 512MB RAM, got {} bytes",
        desc.total_memory_bytes
    );
    assert!(
        !desc.compute_capabilities.is_empty(),
        "CPU must report at least CPUFallback compute type"
    );
}

#[tokio::test]
async fn cpu_thermal_returns_reasonable_value() {
    let driver = CpuDriver::new();
    let temp = driver.get_thermal_state("cpu0").await;

    // thermal_zone0 may not exist in all environments (containers, VMs)
    match temp {
        Ok(t) => {
            assert!(t > 0.0, "Temperature must be positive, got {t}");
            assert!(t < 120.0, "Temperature must be < 120C, got {t}");
        }
        Err(e) => {
            // Acceptable: thermal zone not available in container/VM
            let msg = format!("{e}");
            assert!(
                msg.contains("unreadable") || msg.contains("Unavailable"),
                "Unexpected error type: {e}"
            );
        }
    }
}

#[tokio::test]
async fn cpu_memory_usage_consistent() {
    let driver = CpuDriver::new();
    let usage = driver.get_memory_usage().await;
    assert!(usage.is_ok(), "Memory usage query failed: {:?}", usage.err());

    let (total, used) = usage.unwrap();
    assert!(total > 0, "Total memory must be > 0");
    assert!(used <= total, "Used ({used}) must be <= total ({total})");
    assert!(used > 0, "Used memory should be > 0 on a running system");
}

#[tokio::test]
async fn cpu_predict_fit_reasonable() {
    let driver = CpuDriver::new();

    // 1 byte should always fit
    let fits_small = driver.predict_fit(1).await.unwrap();
    assert!(fits_small, "1 byte should always fit in memory");

    // u64::MAX should never fit
    let fits_huge = driver.predict_fit(u64::MAX).await.unwrap();
    assert!(!fits_huge, "u64::MAX bytes should never fit");
}

#[tokio::test]
async fn cpu_power_state_transitions() {
    let driver = CpuDriver::new();

    // Test all valid transitions
    for target_state in [
        PowerState::DeepVaultSleep,
        PowerState::Sentinel,
        PowerState::FullInference,
        PowerState::ThermalThrottle,
        PowerState::Off,
    ] {
        let result = driver.set_power_state(target_state.clone()).await;
        assert!(result.is_ok(), "Transition to {:?} failed", target_state);

        let current = driver.get_power_state().await.unwrap();
        assert_eq!(current, target_state, "Power state mismatch after transition");
    }
}

#[tokio::test]
async fn cpu_thermal_limit_bounds() {
    let driver = CpuDriver::new();

    // Valid range
    assert!(driver.set_thermal_limit(80.0).await.is_ok());
    assert!(driver.set_thermal_limit(30.0).await.is_ok());
    assert!(driver.set_thermal_limit(105.0).await.is_ok());

    // Invalid: too high
    assert!(driver.set_thermal_limit(106.0).await.is_err());
    // Invalid: too low
    assert!(driver.set_thermal_limit(29.0).await.is_err());
}

#[tokio::test]
async fn cpu_allocate_memory_within_bounds() {
    let driver = CpuDriver::new();

    // Allocate a small amount (should succeed)
    let result = driver.allocate_memory(1024).await;
    assert!(result.is_ok(), "Small allocation failed: {:?}", result.err());

    // Allocate more than total (should fail with OOM)
    let (total, _) = driver.get_memory_usage().await.unwrap();
    let result = driver.allocate_memory(total + 1).await;
    assert!(result.is_err(), "Over-allocation should fail");
}

// ============================================================================
// NVIDIA Driver Integration Tests (require real GPU + nvidia feature)
// ============================================================================

#[cfg(feature = "nvidia")]
mod nvidia_integration {
    use mai_hil::drivers::NvidiaDriver;
    use mai_hil::traits::{HardwareProbe, MemoryManager, PowerState, PowerStateController};

    /// Helper: skip test if no NVIDIA GPU is available
    fn skip_if_no_gpu() -> Option<NvidiaDriver> {
        NvidiaDriver::new(0).ok()
    }

    #[tokio::test]
    async fn nvidia_discover_devices() {
        let Some(driver) = skip_if_no_gpu() else {
            eprintln!("SKIP: No NVIDIA GPU available");
            return;
        };

        let devices = driver.discover_devices().await.unwrap();
        assert!(!devices.is_empty(), "NVIDIA driver should find at least one GPU");

        let desc = &devices[0];
        assert!(!desc.model_name.is_empty());
        assert!(desc.total_memory_bytes > 0, "GPU must report VRAM > 0");
        assert!(!desc.compute_capabilities.is_empty());
        assert!(!desc.driver_version.is_empty());
        assert!(desc.tdp_watts > 0, "TDP should be reported");

        eprintln!(
            "Found GPU: {} ({} MB VRAM, driver {})",
            desc.model_name,
            desc.total_memory_bytes / (1024 * 1024),
            desc.driver_version
        );
    }

    #[tokio::test]
    async fn nvidia_thermal_reading() {
        let Some(driver) = skip_if_no_gpu() else {
            eprintln!("SKIP: No NVIDIA GPU available");
            return;
        };

        let temp = driver.get_thermal_state("gpu0").await.unwrap();
        assert!(temp > 0.0, "GPU temp must be > 0C, got {temp}");
        assert!(temp < 100.0, "GPU temp must be < 100C (idle), got {temp}");
        eprintln!("GPU temperature: {temp}C");
    }

    #[tokio::test]
    async fn nvidia_memory_usage() {
        let Some(driver) = skip_if_no_gpu() else {
            eprintln!("SKIP: No NVIDIA GPU available");
            return;
        };

        let (total, used) = driver.get_memory_usage().await.unwrap();
        assert!(total > 0, "Total VRAM must be > 0");
        assert!(used <= total, "Used VRAM ({used}) must be <= total ({total})");
        eprintln!(
            "VRAM: {} MB used / {} MB total",
            used / (1024 * 1024),
            total / (1024 * 1024)
        );
    }

    #[tokio::test]
    async fn nvidia_predict_fit() {
        let Some(driver) = skip_if_no_gpu() else {
            eprintln!("SKIP: No NVIDIA GPU available");
            return;
        };

        // 1 byte should fit
        assert!(driver.predict_fit(1).await.unwrap());
        // More than total VRAM should not fit
        let (total, _) = driver.get_memory_usage().await.unwrap();
        assert!(!driver.predict_fit(total + 1).await.unwrap());
    }

    #[tokio::test]
    async fn nvidia_power_state_transitions() {
        let Some(driver) = skip_if_no_gpu() else {
            eprintln!("SKIP: No NVIDIA GPU available");
            return;
        };

        driver.set_power_state(PowerState::Sentinel).await.unwrap();
        assert_eq!(driver.get_power_state().await.unwrap(), PowerState::Sentinel);

        driver.set_power_state(PowerState::FullInference).await.unwrap();
        assert_eq!(driver.get_power_state().await.unwrap(), PowerState::FullInference);
    }
}

// ============================================================================
// AMD Driver Integration Tests (require rocm-smi installed)
// ============================================================================

mod amd_integration {
    use mai_hil::drivers::AmdDriver;
    use mai_hil::traits::{HardwareProbe, PowerState, PowerStateController};

    /// Helper: skip test if rocm-smi is not available
    async fn skip_if_no_rocm() -> bool {
        tokio::process::Command::new("rocm-smi")
            .arg("--version")
            .output()
            .await
            .is_err()
    }

    #[tokio::test]
    async fn amd_discover_devices_or_graceful_error() {
        if skip_if_no_rocm().await {
            eprintln!("SKIP: rocm-smi not available");
            return;
        }

        let driver = AmdDriver::new(0);
        let result = driver.discover_devices().await;

        // On a system with rocm-smi but no AMD GPU, we expect a structured error
        match result {
            Ok(devices) => {
                assert!(!devices.is_empty());
                let desc = &devices[0];
                assert!(!desc.model_name.is_empty());
                eprintln!("Found AMD GPU: {}", desc.model_name);
            }
            Err(e) => {
                // Graceful failure is acceptable (no AMD GPU present)
                let msg = format!("{e}");
                assert!(
                    msg.contains("not found") || msg.contains("Unavailable") || msg.contains("error"),
                    "Unexpected error format: {e}"
                );
                eprintln!("AMD GPU not present (expected): {e}");
            }
        }
    }

    #[tokio::test]
    async fn amd_power_state_transitions() {
        // Power state is logical-only, doesn't require real AMD hardware
        let driver = AmdDriver::new(0);
        driver.set_power_state(PowerState::Sentinel).await.unwrap();
        assert_eq!(driver.get_power_state().await.unwrap(), PowerState::Sentinel);
    }
}

// ============================================================================
// HardwareEvent Serialization Tests
// ============================================================================

mod event_serialization {
    use mai_hil::traits::{HardwareEvent, PowerState};

    #[test]
    fn hardware_event_roundtrip_json() {
        let events = vec![
            HardwareEvent::DeviceAdded {
                device_id: "gpu-0-nvidia-h100".to_string(),
            },
            HardwareEvent::DeviceRemoved {
                device_id: "gpu-1-amd-mi300x".to_string(),
            },
            HardwareEvent::ThermalStateChange {
                temperature: 72.5,
                device_id: "gpu-0".to_string(),
            },
            HardwareEvent::PowerStateTransitionRequested {
                from: PowerState::DeepVaultSleep,
                to: PowerState::Sentinel,
            },
        ];

        for event in &events {
            let json = serde_json::to_string(event)
                .expect("HardwareEvent must serialize to JSON");
            assert!(!json.is_empty());

            let deserialized: HardwareEvent = serde_json::from_str(&json)
                .expect("HardwareEvent must deserialize from JSON");

            // Re-serialize to verify round-trip consistency
            let json2 = serde_json::to_string(&deserialized).unwrap();
            assert_eq!(json, json2, "Round-trip serialization mismatch");
        }
    }

    #[test]
    fn hardware_event_audit_log_format() {
        // Verify the serialized format is human-readable for audit purposes
        let event = HardwareEvent::PowerStateTransitionRequested {
            from: PowerState::Sentinel,
            to: PowerState::FullInference,
        };

        let json = serde_json::to_string_pretty(&event).unwrap();
        assert!(json.contains("PowerStateTransitionRequested"));
        assert!(json.contains("Sentinel"));
        assert!(json.contains("FullInference"));
    }
}
