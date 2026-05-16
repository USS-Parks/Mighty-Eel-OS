//! # `HardwareProbe` Trait
//!
//! GPU/accelerator detection, enumeration, and capability reporting.
//! This is the first trait called during boot to discover what hardware
//! is available.
//!
//! ## Contract
//!
//! - `enumerate_devices()` returns all detected compute devices
//! - `probe_capabilities(device_id)` returns a full `CapabilityDescriptor`
//! - Results are cached after first probe (hardware doesn't change at runtime)
//! - Latency: <100ms for initial probe, <1ms for cached results

// Stub: trait definition in Session 02, implementation in Session 06
