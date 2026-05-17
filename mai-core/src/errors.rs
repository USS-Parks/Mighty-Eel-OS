//! Error type definitions using thiserror
//!
//! All errors in mai-core use thiserror for consistent handling and
//! never leak backend-specific details to applications.

use thiserror::Error;

// Re-export module errors for unified handling
pub use crate::health::HealthError;
pub use crate::hotswap::SwapError;
pub use crate::power::PowerError;
pub use crate::registry::RegistryError;
pub use crate::scheduler::SchedulerError;

/// Top-level core error (for API boundary)
#[derive(Error, Debug)]
pub enum CoreError {
    /// Request could not be fulfilled
    #[error("Request failed: {0}")]
    RequestFailed(String),

    /// Requested model not available
    #[error("Model unavailable: {0}")]
    ModelUnavailable(String),

    /// System at capacity
    #[error("System overloaded, try again later")]
    Overloaded,

    /// Air-gap policy violation detected
    #[error("Air-gap violation: {0}")]
    AirGapViolation(String),

    /// Internal error (details logged server-side only, not exposed to clients)
    #[error("Internal error (logged): {0}")]
    Internal(String),
}

impl From<SchedulerError> for CoreError {
    fn from(err: SchedulerError) -> Self {
        tracing::error!("Scheduler error: {:?}", err);
        match err {
            SchedulerError::NoCompatibleAdapter(_) => {
                CoreError::ModelUnavailable("No compatible adapter".into())
            }
            SchedulerError::AllAdaptersBusy => CoreError::Overloaded,
            SchedulerError::Timeout(_) => CoreError::RequestFailed("Request timed out".into()),
            SchedulerError::QueueFull(_) => CoreError::Overloaded,
            _ => CoreError::Internal("Scheduler error".into()),
        }
    }
}

impl From<RegistryError> for CoreError {
    fn from(err: RegistryError) -> Self {
        tracing::error!("Registry error: {:?}", err);
        match err {
            RegistryError::ModelNotFound(m) => CoreError::ModelUnavailable(m),
            RegistryError::InsufficientVram { .. } => CoreError::Overloaded,
            _ => CoreError::Internal("Registry error".into()),
        }
    }
}

impl From<PowerError> for CoreError {
    fn from(err: PowerError) -> Self {
        tracing::error!("Power error: {:?}", err);
        CoreError::Internal(format!("Power subsystem: {}", err))
    }
}

impl From<HealthError> for CoreError {
    fn from(err: HealthError) -> Self {
        tracing::error!("Health error: {:?}", err);
        match err {
            HealthError::AirGapNonCompliant(detail) => CoreError::AirGapViolation(detail),
            _ => CoreError::Internal("Health subsystem error".into()),
        }
    }
}

impl From<SwapError> for CoreError {
    fn from(err: SwapError) -> Self {
        tracing::error!("Swap error: {:?}", err);
        CoreError::Internal(format!("Hot-swap: {}", err))
    }
}
