//! TTL-bounded cache of recent scheduling decisions.
//!
//! Under steady load the same (model, priority, load condition) tuple
//! produces the same placement decision over and over. Caching that decision
//! for a short window avoids re-running the full multi-factor scorer for
//! every request, while leaving room to react to state changes by
//! invalidating the cache.
//!
//! Cache key: `(model_alias, priority, load_bucket)`. The `load_bucket` is
//! a coarse-grained quantization of cluster queue depth so trivially
//! different loads still hit the same entry.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::types::{Priority, ScheduleDecision};

/// Configuration for the decision cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionCacheConfig {
    /// Time after which a cached decision is considered stale.
    #[serde(default = "default_ttl")]
    pub ttl: Duration,
    /// Quantization step for `total_queue_depth`. Two requests whose load
    /// quantizes to the same bucket reuse the same cache slot.
    #[serde(default = "default_bucket")]
    pub load_bucket_size: u32,
}

fn default_ttl() -> Duration {
    Duration::from_secs(60)
}

fn default_bucket() -> u32 {
    8
}

impl Default for DecisionCacheConfig {
    fn default() -> Self {
        Self {
            ttl: default_ttl(),
            load_bucket_size: default_bucket(),
        }
    }
}

/// Composite cache key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DecisionKey {
    /// Model alias the request asked for.
    pub model_alias: String,
    /// Request priority.
    pub priority: Priority,
    /// Quantized cluster load.
    pub load_bucket: u32,
}

impl DecisionKey {
    /// Build a key from the request inputs and the current cluster load.
    pub fn new(
        model_alias: impl Into<String>,
        priority: Priority,
        cluster_queue_depth: u32,
        bucket_size: u32,
    ) -> Self {
        Self {
            model_alias: model_alias.into(),
            priority,
            load_bucket: bucket(cluster_queue_depth, bucket_size),
        }
    }
}

fn bucket(value: u32, size: u32) -> u32 {
    value.checked_div(size).unwrap_or(value)
}

/// Cached entry: a decision plus the time it was inserted.
#[derive(Debug, Clone)]
struct CacheEntry {
    decision: ScheduleDecision,
    inserted_at: Instant,
}

/// TTL-bounded scheduling-decision cache.
#[derive(Debug)]
pub struct DecisionCache {
    config: DecisionCacheConfig,
    cache: Mutex<HashMap<DecisionKey, CacheEntry>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl DecisionCache {
    /// Build a cache with the given configuration.
    pub fn new(config: DecisionCacheConfig) -> Self {
        Self {
            config,
            cache: Mutex::new(HashMap::new()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Default cache: 60s TTL, 8-request bucket size.
    pub fn with_defaults() -> Self {
        Self::new(DecisionCacheConfig::default())
    }

    /// Lookup a key. Returns `Some` only when the entry is still within TTL.
    /// Stale entries are removed inline so they do not accumulate.
    pub fn get(&self, key: &DecisionKey) -> Option<ScheduleDecision> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(entry) = cache.get(key) {
            if entry.inserted_at.elapsed() <= self.config.ttl {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Some(entry.decision.clone());
            }
            cache.remove(key);
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insert a decision under the given key, overwriting any prior entry.
    pub fn insert(&self, key: DecisionKey, decision: ScheduleDecision) {
        self.cache.lock().unwrap().insert(
            key,
            CacheEntry {
                decision,
                inserted_at: Instant::now(),
            },
        );
    }

    /// Invalidate the entire cache. Call this on any state change that would
    /// affect placement: instance add/remove, large KV pressure swing, model
    /// (un)load.
    pub fn invalidate_all(&self) {
        self.cache.lock().unwrap().clear();
    }

    /// Number of live entries (including stale ones not yet pruned).
    pub fn len(&self) -> usize {
        self.cache.lock().unwrap().len()
    }

    /// True when the cache currently holds no entries.
    pub fn is_empty(&self) -> bool {
        self.cache.lock().unwrap().is_empty()
    }

    /// `(hits, misses)` counters.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
        )
    }

    /// Cache hit ratio in `[0.0, 1.0]`. Returns 0.0 when neither has fired.
    pub fn hit_rate(&self) -> f64 {
        let (h, m) = self.stats();
        let total = h + m;
        if total == 0 {
            0.0
        } else {
            h as f64 / total as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{GpuId, InstanceId};

    fn decision(instance: &str) -> ScheduleDecision {
        ScheduleDecision {
            instance_id: InstanceId::new(instance),
            assigned_gpus: vec![GpuId::new(0)],
            estimated_latency_ms: 50,
            placement_reason: "test".to_string(),
        }
    }

    #[test]
    fn test_bucket_quantization() {
        assert_eq!(bucket(0, 8), 0);
        assert_eq!(bucket(7, 8), 0);
        assert_eq!(bucket(8, 8), 1);
        assert_eq!(bucket(15, 8), 1);
        assert_eq!(bucket(16, 8), 2);
    }

    #[test]
    fn test_miss_then_hit_on_same_key() {
        let cache = DecisionCache::with_defaults();
        let key = DecisionKey::new("qwen3-14b", Priority::Normal, 10, 8);
        assert!(cache.get(&key).is_none());
        cache.insert(key.clone(), decision("ollama:0"));
        let hit = cache.get(&key).expect("expected cache hit");
        assert_eq!(hit.instance_id, InstanceId::new("ollama:0"));
        let (h, m) = cache.stats();
        assert_eq!((h, m), (1, 1));
    }

    #[test]
    fn test_invalidate_all_clears_entries() {
        let cache = DecisionCache::with_defaults();
        let key = DecisionKey::new("qwen3-14b", Priority::Normal, 10, 8);
        cache.insert(key.clone(), decision("ollama:0"));
        assert_eq!(cache.len(), 1);
        cache.invalidate_all();
        assert_eq!(cache.len(), 0);
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn test_close_loads_share_a_bucket() {
        let cache = DecisionCache::with_defaults();
        let k1 = DecisionKey::new("qwen3-14b", Priority::Normal, 10, 8);
        let k2 = DecisionKey::new("qwen3-14b", Priority::Normal, 14, 8);
        // 10 and 14 both quantize to bucket 1 with size 8.
        assert_eq!(k1, k2);
        cache.insert(k1.clone(), decision("ollama:0"));
        assert!(cache.get(&k2).is_some());
    }

    #[test]
    fn test_different_priority_distinguishes_keys() {
        let cache = DecisionCache::with_defaults();
        let k_normal = DecisionKey::new("qwen3-14b", Priority::Normal, 10, 8);
        let k_high = DecisionKey::new("qwen3-14b", Priority::High, 10, 8);
        cache.insert(k_normal, decision("ollama:0"));
        assert!(cache.get(&k_high).is_none());
    }

    #[test]
    fn test_stale_entry_is_pruned_on_get() {
        let cache = DecisionCache::new(DecisionCacheConfig {
            ttl: Duration::from_millis(1),
            ..DecisionCacheConfig::default()
        });
        let key = DecisionKey::new("qwen3-14b", Priority::Normal, 10, 8);
        cache.insert(key.clone(), decision("ollama:0"));
        std::thread::sleep(Duration::from_millis(5));
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_hit_rate_math() {
        let cache = DecisionCache::with_defaults();
        let key = DecisionKey::new("qwen3-14b", Priority::Normal, 10, 8);
        cache.insert(key.clone(), decision("ollama:0"));
        for _ in 0..7 {
            cache.get(&key);
        }
        cache.get(&DecisionKey::new("missing", Priority::Normal, 0, 8));
        cache.get(&DecisionKey::new("missing", Priority::Normal, 0, 8));
        cache.get(&DecisionKey::new("missing", Priority::Normal, 0, 8));
        // 7 hits, 3 misses => 0.7 hit rate, matching the spec target.
        let rate = cache.hit_rate();
        assert!((rate - 0.7).abs() < 1e-9);
    }
}
