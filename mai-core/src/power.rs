//! # Power State Machine
//!
//! Controls device power state transitions between Off, `DeepVaultSleep`,
//! Sentinel, `FullInference`, and `ThermalThrottle`.
//!
//! ## States
//!
//! | State           | GPU-era Power | QM-era Power |
//! |-----------------|---------------|--------------|
//! | Off             | 0W            | 0W           |
//! | `DeepVaultSleep`  | ~2W           | ~1W          |
//! | Sentinel        | ~8W           | ~3W          |
//! | `FullInference`   | ~350W         | ~15W         |
//! | `ThermalThrottle` | Variable      | N/A          |
//!
//! ## Auto-Demotion
//!
//! - `FullInference` -> Sentinel: 12 minutes idle (configurable)
//! - Sentinel -> `DeepVaultSleep`: 2 hours idle (configurable)
//!
//! ## Sovereignty Signal
//!
//! The 48-month plan calls this "a sovereignty signal." A home AI that
//! draws 2W in sleep feels like an appliance. One that draws 350W
//! continuously feels like a liability.

// Stub: implementation in Session 07
