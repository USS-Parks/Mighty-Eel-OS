//! # `MemoryManager` Trait
//!
//! VRAM allocation, model memory mapping, OOM signaling, and shared
//! memory management between adapters for hybrid scheduling.
//!
//! ## Contract
//!
//! - `allocate(bytes)` reserves VRAM for a model load
//! - `deallocate(allocation_id)` releases VRAM
//! - `available_bytes()` returns unallocated VRAM
//! - `oom_signal()` returns an async channel that fires on OOM
//! - All allocations are tracked per-model for clean eviction

// Stub: trait definition in Session 02, implementation in Session 06
