//! Response Cache Layer
//!
//! Implements a per-profile LRU cache with TTL and memory budget enforcement.
//! Cache keys are blake3 hashes of (payload + model + profile + streaming flag).
//!
//! # Design Decisions
//!
//! - **Standalone module**: Does not integrate into Scheduler or HotSwapManager
//!   directly. Integration is deferred to the API server, which provides
//!   the natural request/response interception point.
//! - **Profile isolation**: Cache keys include profile_id. The `invalidate_profile`
//!   method purges all entries for a given family profile.
//! - **Model invalidation**: When a model is hot-swapped, all cached responses
//!   from that model must be purged (stale weights = stale outputs).
//! - **Determinism gate**: Only requests with `streaming: false` are cacheable.
//!   Streaming responses are delivered incrementally and caching them requires
//!   buffering the full response, which is a separate concern.
//! - **Air-gap safe**: No network access. Local memory only. Metrics never leave device.
//! - **No unsafe code**: Pure safe Rust.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use blake3::Hasher;
use tracing::{debug, trace};

use crate::scheduler::{InferenceRequest, RequestPayload};
use crate::types::{
    CacheConfig, CacheEntry, CacheKey, CacheMetrics, CachedResponse, ModelId, ProfileId,
};

/// Response cache with LRU eviction and TTL expiry.
///
/// Thread safety: This struct is NOT internally synchronized. The caller
/// is responsible for wrapping in `Arc<Mutex<_>>`
/// or `Arc<RwLock<_>>` as appropriate for their concurrency model.
pub struct ResponseCache {
    entries: HashMap<CacheKey, CacheEntry>,
    /// LRU order: front = most recently used, back = eviction candidate
    order: VecDeque<CacheKey>,
    config: CacheConfig,
    metrics: CacheMetrics,
}

impl ResponseCache {
    /// Create a new cache with the given configuration.
    pub fn new(config: CacheConfig) -> Self {
        Self {
            entries: HashMap::with_capacity(config.max_entries.min(1024)),
            order: VecDeque::with_capacity(config.max_entries.min(1024)),
            config,
            metrics: CacheMetrics::default(),
        }
    }

    /// Compute a cache key from an inference request.
    ///
    /// Key components: payload content + model + profile + streaming flag.
    /// Uses blake3 truncated to 16 bytes (128 bits) for collision resistance
    /// while keeping HashMap overhead low.
    pub fn compute_key(request: &InferenceRequest) -> CacheKey {
        let mut hasher = Hasher::new();

        // Hash the payload content
        match &request.payload {
            RequestPayload::Chat { messages } => {
                hasher.update(b"chat:");
                for msg in messages {
                    hasher.update(msg.role.as_bytes());
                    hasher.update(b":");
                    hasher.update(msg.content.as_bytes());
                    hasher.update(b"\n");
                }
            }
            RequestPayload::Completion { prompt } => {
                hasher.update(b"completion:");
                hasher.update(prompt.as_bytes());
            }
            RequestPayload::Embedding { texts } => {
                hasher.update(b"embedding:");
                for t in texts {
                    hasher.update(t.as_bytes());
                    hasher.update(b"\n");
                }
            }
        }

        // Hash model selection
        if let Some(ref model) = request.model_name {
            hasher.update(model.as_bytes());
        } else {
            hasher.update(b"__default__");
        }

        // Hash profile (ensures cross-profile isolation)
        hasher.update(request.profile_id.as_bytes());

        // Hash streaming flag (streaming responses are not cached but
        // we include it for key uniqueness)
        hasher.update(if request.streaming { b"s:1" } else { b"s:0" });

        hasher.finalize().as_bytes()[0..16].to_vec()
    }

    /// Determine if a request is eligible for caching.
    ///
    /// Requests are NOT cacheable if:
    /// - Cache is disabled in config
    /// Request is streaming
    /// - Model is in the exclude list
    /// - Request type is FunctionCall (side effects, non-deterministic)
    pub fn is_cacheable(&self, request: &InferenceRequest) -> bool {
        if !self.config.enabled {
            return false;
        }

        // Streaming responses can't be cached at this layer
        if request.streaming {
            return false;
        }

        // Function calls have side effects; never cache
        if matches!(
            request.request_type,
            crate::scheduler::RequestType::FunctionCall
        ) {
            return false;
        }

        // Check model exclusion list
        if let Some(ref model) = request.model_name
            && self.config.exclude_models.contains(model)
        {
            return false;
        }

        true
    }

    /// Look up a cached response. Returns None if not found or expired.
    ///
    /// On hit: updates LRU position and increments hit counter.
    /// On TTL expiry: removes the stale entry and returns None.
    pub fn get(&mut self, key: &CacheKey) -> Option<&CachedResponse> {
        // Check TTL first (without borrow conflict)
        let expired = self
            .entries
            .get(key)
            .is_none_or(|e| Instant::now().duration_since(e.created_at) > self.config.ttl);

        if expired {
            // Remove stale entry if it exists
            if self.entries.remove(key).is_some() {
                self.order.retain(|k| k != key);
                self.metrics.evictions += 1;
                trace!("Cache TTL expiry for key");
            }
            self.metrics.misses += 1;
            return None;
        }

        // Update access metadata
        let entry = self.entries.get_mut(key)?;
        entry.last_accessed = Instant::now();
        entry.hit_count += 1;

        // Move to front of LRU order
        self.order.retain(|k| k != key);
        self.order.push_front(key.clone());

        self.metrics.hits += 1;
        debug!(hits = self.metrics.hits, "Cache hit");

        Some(&self.entries.get(key)?.response)
    }

    /// Store a response in the cache.
    ///
    /// Skipped if: key already exists, or response is below min size threshold.
    /// After insertion, enforces memory budget and max entry limits via LRU eviction.
    pub fn put(
        &mut self,
        key: CacheKey,
        response: CachedResponse,
        model_id: &ModelId,
        profile_id: &ProfileId,
    ) {
        // Don't overwrite existing entries
        if self.entries.contains_key(&key) {
            return;
        }

        // Check minimum size threshold
        let byte_size = response.text.len() + response.finish_reason.len() + 64;
        if byte_size < self.config.min_response_bytes {
            return;
        }

        self.entries.insert(
            key.clone(),
            CacheEntry {
                response,
                created_at: Instant::now(),
                last_accessed: Instant::now(),
                hit_count: 0,
                model_id: model_id.clone(),
                profile_id: *profile_id,
                byte_size,
            },
        );
        self.order.push_front(key);

        self.enforce_limits();
    }

    /// Evict entries until memory budget and max entry count are satisfied.
    fn enforce_limits(&mut self) {
        // Evict by memory budget
        loop {
            let current_mem: usize = self.entries.values().map(|e| e.byte_size).sum();
            self.metrics.memory_used = current_mem;
            if current_mem <= self.config.max_memory_bytes {
                break;
            }
            if let Some(oldest_key) = self.order.pop_back() {
                self.entries.remove(&oldest_key);
                self.metrics.evictions += 1;
            } else {
                break;
            }
        }

        // Evict by entry count
        while self.entries.len() > self.config.max_entries {
            if let Some(oldest_key) = self.order.pop_back() {
                self.entries.remove(&oldest_key);
                self.metrics.evictions += 1;
            } else {
                break;
            }
        }

        self.metrics.entry_count = self.entries.len();
    }

    /// Purge all cache entries for a specific model.
    ///
    /// Called when a model is hot-swapped (stale weights = stale outputs).
    /// Returns the number of entries purged.
    pub fn invalidate_model(&mut self, model_id: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, entry| entry.model_id != model_id);
        self.order.retain(|k| self.entries.contains_key(k));
        let purged = before - self.entries.len();
        if purged > 0 {
            debug!(model_id, purged, "Cache invalidated for model");
        }
        self.metrics.evictions += purged as u64;
        self.metrics.entry_count = self.entries.len();
        purged
    }

    /// Purge all cache entries for a specific family profile.
    ///
    /// Called when a profile is deleted or privacy-purged.
    /// Returns the number of entries purged.
    pub fn invalidate_profile(&mut self, profile_id: &ProfileId) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|_, entry| &entry.profile_id != profile_id);
        self.order.retain(|k| self.entries.contains_key(k));
        let purged = before - self.entries.len();
        if purged > 0 {
            debug!(?profile_id, purged, "Cache invalidated for profile");
        }
        self.metrics.evictions += purged as u64;
        self.metrics.entry_count = self.entries.len();
        purged
    }

    /// Get current cache metrics. Local-only, never transmitted.
    pub fn metrics(&self) -> &CacheMetrics {
        &self.metrics
    }

    /// Clear all cache entries.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.metrics.entry_count = 0;
        self.metrics.memory_used = 0;
    }

    /// Current number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{ChatMessage, RequestPriority, RequestType};
    use std::time::Duration;
    use uuid::Uuid;

    fn make_request(prompt: &str, model: Option<&str>, profile: ProfileId) -> InferenceRequest {
        InferenceRequest {
            id: Uuid::new_v4(),
            profile_id: profile,
            model_name: model.map(|s| s.to_string()),
            request_type: RequestType::Chat,
            payload: RequestPayload::Chat {
                messages: vec![ChatMessage {
                    role: "user".into(),
                    content: prompt.into(),
                }],
            },
            priority: RequestPriority::Normal,
            timeout: Duration::from_secs(30),
            streaming: false,
            enqueued_at: Instant::now(),
            estimated_tokens: 100,
        }
    }

    #[test]
    fn test_cache_hit_returns_stored_response() {
        let config = CacheConfig::default();
        let mut cache = ResponseCache::new(config);
        let profile = Uuid::new_v4();
        let req = make_request("hello world", Some("model1"), profile);

        let key = ResponseCache::compute_key(&req);
        cache.put(
            key.clone(),
            CachedResponse {
                text: "Hello! How can I help?".into(),
                tokens_used: 6,
                finish_reason: "stop".into(),
            },
            &"model1".to_string(),
            &profile,
        );

        let result = cache.get(&key);
        assert!(result.is_some());
        assert_eq!(result.unwrap().text, "Hello! How can I help?");
        assert_eq!(cache.metrics().hits, 1);
    }

    #[test]
    fn test_cache_miss_increments_counter() {
        let mut cache = ResponseCache::new(CacheConfig::default());
        let key = vec![0u8; 16];
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.metrics().misses, 1);
    }

    #[test]
    fn test_streaming_requests_not_cacheable() {
        let cache = ResponseCache::new(CacheConfig::default());
        let mut req = make_request("test", None, Uuid::new_v4());
        req.streaming = true;
        assert!(!cache.is_cacheable(&req));
    }

    #[test]
    fn test_function_call_not_cacheable() {
        let cache = ResponseCache::new(CacheConfig::default());
        let mut req = make_request("test", None, Uuid::new_v4());
        req.request_type = RequestType::FunctionCall;
        assert!(!cache.is_cacheable(&req));
    }

    #[test]
    fn test_excluded_model_not_cacheable() {
        let config = CacheConfig {
            exclude_models: vec!["unstable-model".to_string()],
            ..CacheConfig::default()
        };
        let cache = ResponseCache::new(config);
        let req = make_request("test", Some("unstable-model"), Uuid::new_v4());
        assert!(!cache.is_cacheable(&req));
    }

    #[test]
    fn test_ttl_expiration() {
        let config = CacheConfig {
            ttl: Duration::from_millis(50),
            ..CacheConfig::default()
        };
        let mut cache = ResponseCache::new(config);
        let profile = Uuid::new_v4();
        let req = make_request("ttl test", Some("m"), profile);
        let key = ResponseCache::compute_key(&req);

        cache.put(
            key.clone(),
            CachedResponse {
                text: "response that will expire".into(),
                tokens_used: 4,
                finish_reason: "stop".into(),
            },
            &"m".to_string(),
            &profile,
        );

        // Should hit immediately
        assert!(cache.get(&key).is_some());

        // Wait for TTL
        std::thread::sleep(Duration::from_millis(60));

        // Should miss after expiry
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.metrics().evictions, 1);
    }

    #[test]
    fn test_model_invalidation() {
        let mut cache = ResponseCache::new(CacheConfig::default());
        let profile = Uuid::new_v4();

        let req1 = make_request("q1", Some("model_a"), profile);
        let req2 = make_request("q2", Some("model_b"), profile);
        let key1 = ResponseCache::compute_key(&req1);
        let key2 = ResponseCache::compute_key(&req2);

        cache.put(
            key1.clone(),
            CachedResponse {
                text: "from model_a".into(),
                tokens_used: 3,
                finish_reason: "stop".into(),
            },
            &"model_a".to_string(),
            &profile,
        );
        cache.put(
            key2.clone(),
            CachedResponse {
                text: "from model_b".into(),
                tokens_used: 3,
                finish_reason: "stop".into(),
            },
            &"model_b".to_string(),
            &profile,
        );

        assert_eq!(cache.len(), 2);
        let purged = cache.invalidate_model("model_a");
        assert_eq!(purged, 1);
        assert!(cache.get(&key1).is_none());
        assert!(cache.get(&key2).is_some());
    }

    #[test]
    fn test_profile_invalidation() {
        let mut cache = ResponseCache::new(CacheConfig::default());
        let profile_a = Uuid::new_v4();
        let profile_b = Uuid::new_v4();

        let req1 = make_request("q1", Some("m"), profile_a);
        let req2 = make_request("q2", Some("m"), profile_b);
        let key1 = ResponseCache::compute_key(&req1);
        let key2 = ResponseCache::compute_key(&req2);

        cache.put(
            key1.clone(),
            CachedResponse {
                text: "for profile a".into(),
                tokens_used: 3,
                finish_reason: "stop".into(),
            },
            &"m".to_string(),
            &profile_a,
        );
        cache.put(
            key2.clone(),
            CachedResponse {
                text: "for profile b".into(),
                tokens_used: 3,
                finish_reason: "stop".into(),
            },
            &"m".to_string(),
            &profile_b,
        );

        let purged = cache.invalidate_profile(&profile_a);
        assert_eq!(purged, 1);
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&key2).is_some());
    }

    #[test]
    fn test_memory_budget_eviction() {
        let config = CacheConfig {
            max_memory_bytes: 300,
            min_response_bytes: 10,
            ..CacheConfig::default()
        };
        let mut cache = ResponseCache::new(config);
        let profile = Uuid::new_v4();

        // Each entry is ~80+ bytes (text + overhead). Fill past budget.
        for i in 0..10 {
            let req = make_request(&format!("query number {i}"), Some("m"), profile);
            let key = ResponseCache::compute_key(&req);
            cache.put(
                key,
                CachedResponse {
                    text: format!("response for query number {i}"),
                    tokens_used: 5,
                    finish_reason: "stop".into(),
                },
                &"m".to_string(),
                &profile,
            );
        }

        // Memory budget should have forced evictions
        let mem: usize = cache.entries.values().map(|e| e.byte_size).sum();
        assert!(mem <= 300, "Memory budget violated: {mem} > 300");
        assert!(cache.metrics().evictions > 0);
    }

    #[test]
    fn test_duplicate_key_not_overwritten() {
        let mut cache = ResponseCache::new(CacheConfig::default());
        let profile = Uuid::new_v4();
        let req = make_request("same prompt", Some("m"), profile);
        let key = ResponseCache::compute_key(&req);

        cache.put(
            key.clone(),
            CachedResponse {
                text: "first response".into(),
                tokens_used: 2,
                finish_reason: "stop".into(),
            },
            &"m".to_string(),
            &profile,
        );
        cache.put(
            key.clone(),
            CachedResponse {
                text: "second response".into(),
                tokens_used: 2,
                finish_reason: "stop".into(),
            },
            &"m".to_string(),
            &profile,
        );

        let cached = cache.get(&key).unwrap();
        assert_eq!(cached.text, "first response");
    }

    #[test]
    fn test_below_min_size_not_cached() {
        let config = CacheConfig {
            min_response_bytes: 200,
            ..CacheConfig::default()
        };
        let mut cache = ResponseCache::new(config);
        let profile = Uuid::new_v4();
        let key = vec![1u8; 16];

        cache.put(
            key.clone(),
            CachedResponse {
                text: "tiny".into(), // 4 bytes + 4 + 64 = 72 < 200
                tokens_used: 1,
                finish_reason: "stop".into(),
            },
            &"m".to_string(),
            &profile,
        );

        assert!(cache.is_empty());
    }

    #[test]
    fn test_clear_empties_cache() {
        let mut cache = ResponseCache::new(CacheConfig::default());
        let profile = Uuid::new_v4();
        let key = vec![1u8; 16];

        cache.put(
            key,
            CachedResponse {
                text: "something worth caching here".into(),
                tokens_used: 5,
                finish_reason: "stop".into(),
            },
            &"m".to_string(),
            &profile,
        );

        assert!(!cache.is_empty());
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.metrics().entry_count, 0);
    }

    #[test]
    fn test_key_includes_profile_isolation() {
        let profile_a = Uuid::new_v4();
        let profile_b = Uuid::new_v4();

        let req_a = make_request("same prompt", Some("same_model"), profile_a);
        let req_b = make_request("same prompt", Some("same_model"), profile_b);

        let key_a = ResponseCache::compute_key(&req_a);
        let key_b = ResponseCache::compute_key(&req_b);

        // Same prompt + model but different profiles must produce different keys
        assert_ne!(key_a, key_b);
    }
}
