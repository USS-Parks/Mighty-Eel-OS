use async_trait::async_trait;
use crate::HilError;

/// `MemoryManager`: Interface for compute memory allocation and tracking.
/// Handles VRAM/RAM mapping, model loading offsets, and OOM prediction.
#[async_trait]
pub trait MemoryManager: Send + Sync {
    /// Allocates a contiguous memory region for a model.
    /// Returns memory handle or OOM error.
    async fn allocate_memory(&self, size_bytes: u64) -> Result<u64, HilError>;

    /// Frees previously allocated memory region.
    async fn free_memory(&self, handle: u64) -> Result<(), HilError>;

    /// Predicts if a model of `required_size` can be loaded given current utilization.
    /// Core Kernel uses this for eviction decisions before attempting load.
    async fn predict_fit(&self, required_size: u64) -> Result<bool, HilError>;

    /// Returns total and used memory for this hardware class.
    async fn get_memory_usage(&self) -> Result<(u64, u64), HilError>;
}
