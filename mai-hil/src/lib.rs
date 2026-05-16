//! # MAI Hardware Interface Layer (HIL)
//!
//! The typed interface between the MAI core kernel and hardware-specific
//! driver implementations. Adapted from the Tock kernel's HIL model.
//!
//! ## Trust Level: TRUSTED
//!
//! HIL trait definitions contain zero `unsafe` code. Driver implementations
//! (in `drivers/`) may use `unsafe` for direct hardware access. This is the
//! ONLY location in the MAI where `unsafe` is permitted.
//!
//! ## Traits
//!
//! - `HardwareProbe`: Device detection, enumeration, capability reporting
//! - `PowerStateController`: Power state transitions, current draw, thermals
//! - `MemoryManager`: VRAM allocation, OOM signaling, model memory mapping
//! - `SecureLoadContext`: TPM-attested model loading, encrypted weight transfer
//!
//! ## Drivers
//!
//! - `nvidia`: NVML-based NVIDIA GPU driver (H100, H200, RTX 5090)
//! - `amd`: ROCm-based AMD GPU driver (MI300X, RX 9090 XT)
//! - `cpu`: CPU fallback driver (AVX-512 detection)
//! - `tetramem_stub`: Future memristor interface (compiles, returns `NotImplemented`)

pub mod drivers;
pub mod traits;
