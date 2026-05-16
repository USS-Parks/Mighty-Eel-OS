//! Core type definitions shared across mai-core modules

use uuid::Uuid;

/// Unique request identifier
pub type RequestId = Uuid;
/// Family profile identifier
pub type ProfileId = Uuid;
/// Model identifier. Format: "name:version:quantization"
pub type ModelId = String;
/// Adapter instance identifier. Format: "backend:instance"
pub type AdapterId = String;
/// GPU identifier. Format: "vendor:model:pci_addr"
pub type GpuIdentifier = String;
/// Power state transition identifier
pub type TransitionId = Uuid;

/// Common result type for core operations
pub type CoreResult<T> = Result<T, crate::CoreError>;
