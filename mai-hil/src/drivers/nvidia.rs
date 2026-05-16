//! # NVIDIA CUDA Driver
//!
//! NVML-based GPU detection and management for NVIDIA hardware.
//! Supports H100 `PCIe`, H100 SXM5, H200, RTX 5090.
//!
//! ## Features
//!
//! - GPU enumeration via NVML
//! - VRAM tracking with per-model allocation accounting
//! - Power management via persistence mode and power limits
//! - Thermal monitoring with throttle event detection
//! - Feature-gated behind `--features nvidia`
//!
//! ## Unsafe Usage
//!
//! This driver uses `unsafe` for NVML FFI calls. All unsafe blocks are
//! documented with safety invariants.

// Stub: implementation in Session 06
