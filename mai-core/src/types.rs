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

// Cache types ---

use std::time::{Duration, Instant};

/// Cache key: truncated blake3 hash of request parameters
pub type CacheKey = Vec<u8>;

/// Configuration for the response cache layer
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Whether caching is enabled
    pub enabled: bool,
    /// Maximum number of cached entries
    pub max_entries: usize,
    /// Maximum memory budget in bytes
    pub max_memory_bytes: usize,
    /// Time-to-live for cache entries
    pub ttl: Duration,
    /// Optional per-profile entry limit
    pub per_profile_limit: Option<usize>,
    /// Models excluded from caching (e.g., non-deterministic)
    pub exclude_models: Vec<ModelId>,
    /// Minimum response length (bytes) to bother caching
    pub min_response_bytes: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: 10_000,
            max_memory_bytes: 256 * 1024 * 1024, // 256 MiB
            ttl: Duration::from_secs(3600),      // 1 hour
            per_profile_limit: None,
            exclude_models: vec![],
            min_response_bytes: 40, // ~10 tokens * 4 bytes
        }
    }
}

/// A single cached response entry
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// The cached response data
    pub response: CachedResponse,
    /// When this entry was created
    pub created_at: Instant,
    /// When this entry was last accessed
    pub last_accessed: Instant,
    /// Number of cache hits on this entry
    pub hit_count: u64,
    /// Model that generated this response
    pub model_id: ModelId,
    /// Profile that owns this cache entry
    pub profile_id: ProfileId,
    /// Estimated byte size of this entry (for memory budget)
    pub byte_size: usize,
}

/// The actual cached response payload
#[derive(Debug, Clone, Default)]
pub struct CachedResponse {
    /// Response text
    pub text: String,
    /// Tokens consumed generating this response
    pub tokens_used: usize,
    /// Finish reason from the backend
    pub finish_reason: String,
}

/// Local-only cache performance metrics (never transmitted off-device)
#[derive(Debug, Clone, Default)]
pub struct CacheMetrics {
    /// Total cache hits
    pub hits: u64,
    /// Total cache misses
    pub misses: u64,
    /// Total evictions (LRU + TTL)
    pub evictions: u64,
    /// Current memory used (bytes)
    pub memory_used: usize,
    /// Current entry count
    pub entry_count: usize,
}
