//! Hardware driver implementations for MAI HIL.
//!
//! Each driver implements the HIL traits:
//! - `HardwareProbe`: device discovery and thermal monitoring
//! - `PowerStateController`: power state management
//! - `MemoryManager`: VRAM/RAM allocation and tracking
//!
//! Drivers are feature-gated to avoid unnecessary dependencies:
//! - `nvidia` feature enables the NVIDIA CUDA driver (nvml-wrapper)
//! - AMD and CPU drivers are always available (use CLI/sysfs)

pub mod cpu;
pub use cpu::CpuDriver;

#[cfg(feature = "nvidia")]
pub mod nvidia;
#[cfg(feature = "nvidia")]
pub use nvidia::NvidiaDriver;

pub mod amd;
pub use amd::AmdDriver;

/// Helper: parse a key=value line from /proc-style files
pub(crate) fn parse_proc_line(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let parts: Vec<&str> = line.splitn(2, ':').collect();
    if parts.len() == 2 {
        Some((parts[0].trim(), parts[1].trim()))
    } else {
        None
    }
}

/// Helper: extract numeric value from "1234 kB" style strings
pub(crate) fn parse_memory_value(value: &str) -> Option<u64> {
    let value = value.trim();
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }
    parts[0].parse::<u64>().ok()
}
