//! # `PowerStateController` Trait
//!
//! Power state transition requests, current draw reporting, wake latency
//! guarantees, and thermal throttle signals.
//!
//! ## Contract
//!
//! - `transition(from, to)` requests a power state change on hardware
//! - `current_power_watts()` returns real-time power draw
//! - `wake_latency_ms(target_state)` returns guaranteed wake time
//! - `thermal_state()` returns current thermal status
//! - Transitions are async (GPU power changes take milliseconds)

// Stub: trait definition in Session 02, implementation in Session 06
