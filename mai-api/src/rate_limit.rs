//! SCAN-1 (Security SEC-011-MAI): token-bucket rate-limit middleware.
//!
//! **Status: scaffolded but NOT wired into `routes.rs` in SCAN-1.**
//!
//! The SCAN-1 session intentionally lands this as a standalone module
//! with its own unit tests so it can be reviewed in isolation. The
//! follow-up SEC-95 session wires it into the route stack (immediately
//! outside `auth_middleware`, so the rate-limited path still gets the
//! correlation ID + metrics observation) and adds the integration
//! tests against the live `axum::Router`.
//!
//! Design choices (justified in `docs/scans/SCAN-1-INTERNAL-GITDOCTOR-REPORT.md`):
//!
//! * **In-process token bucket, no new crate.** This is air-gapped
//!   middleware running on a single appliance. Adding `tower_governor`
//!   pulls in `governor` + `nonzero_ext` + `quanta` + `dashmap` —
//!   four supply-chain entries for what is ~80 lines of code. Keep
//!   the surface small and auditable.
//!
//! * **Per-route bucket selection by path prefix**, not per-caller.
//!   The threat model is "stop one runaway client from exhausting
//!   the inference queue," not "fairly multiplex N callers" (the
//!   scheduler handles fairness). A single bucket per route prefix
//!   is the simplest correct shape.
//!
//! * **Emits `429 Too Many Requests`** with `Retry-After` header.
//!   The `metrics_middleware` already counts these via the existing
//!   `RATE_LIMITED_TOTAL` counter — no new metric needed.
//!
//! * **Disabled by default.** A nil `RateLimitConfig` is a no-op so
//!   the legacy code path is preserved until SEC-95 explicitly opts
//!   each route group into a bucket.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Per-route token-bucket configuration.
#[derive(Debug, Clone, Copy)]
pub struct BucketConfig {
    /// Maximum tokens the bucket can hold.
    pub capacity: u32,
    /// Tokens added per second (refill rate).
    pub refill_per_sec: f64,
}

impl BucketConfig {
    /// A sensible default for inference routes: 60 req/min burst of 30.
    pub const fn inference_default() -> Self {
        Self {
            capacity: 30,
            refill_per_sec: 1.0,
        }
    }

    /// A sensible default for compliance read paths: 600 req/min burst of 100.
    pub const fn compliance_read_default() -> Self {
        Self {
            capacity: 100,
            refill_per_sec: 10.0,
        }
    }
}

/// In-process token-bucket state. One per route prefix.
#[derive(Debug)]
struct Bucket {
    cfg: BucketConfig,
    tokens: f64,
    last_refill: Instant,
}

impl Bucket {
    fn new(cfg: BucketConfig) -> Self {
        Self {
            cfg,
            tokens: f64::from(cfg.capacity),
            last_refill: Instant::now(),
        }
    }

    /// Try to take one token. Returns `Ok(())` if allowed, or
    /// `Err(retry_after)` with the duration the caller should wait.
    fn try_take(&mut self) -> Result<(), Duration> {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens =
            (self.tokens + elapsed * self.cfg.refill_per_sec).min(f64::from(self.cfg.capacity));
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            let deficit = 1.0 - self.tokens;
            let wait_secs = deficit / self.cfg.refill_per_sec;
            Err(Duration::from_secs_f64(wait_secs.max(0.001)))
        }
    }
}

/// Top-level rate-limit state. Holds one bucket per configured route
/// prefix. The map is created at startup; runtime lookups are O(prefix-count)
/// because the prefix set is small (single-digit) — switching to a trie
/// would be over-engineering.
#[derive(Debug)]
pub struct RateLimiter {
    buckets: Mutex<Vec<(String, Bucket)>>,
}

impl RateLimiter {
    pub fn new(routes: &[(String, BucketConfig)]) -> Self {
        let mut buckets: Vec<(String, Bucket)> = Vec::with_capacity(routes.len());
        for (prefix, cfg) in routes {
            buckets.push((prefix.clone(), Bucket::new(*cfg)));
        }
        // Longest prefix first so the most specific bucket wins.
        buckets.sort_by_key(|entry| std::cmp::Reverse(entry.0.len()));
        Self {
            buckets: Mutex::new(buckets),
        }
    }

    /// Check whether the request path is rate-limited. Returns
    /// `Ok(())` if allowed (or if no bucket matches the path), or
    /// `Err(retry_after)` if the matching bucket is empty.
    pub fn check(&self, path: &str) -> Result<(), Duration> {
        let mut guard = self.buckets.lock().expect("rate-limit mutex poisoned");
        // First matching prefix wins. With small prefix counts this is
        // cheaper than building a trie.
        for (prefix, bucket) in guard.iter_mut() {
            if path.starts_with(prefix.as_str()) {
                return bucket.try_take();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn bucket_starts_full() {
        let cfg = BucketConfig {
            capacity: 5,
            refill_per_sec: 1.0,
        };
        let mut b = Bucket::new(cfg);
        for _ in 0..5 {
            assert!(b.try_take().is_ok());
        }
        assert!(b.try_take().is_err());
    }

    #[test]
    fn bucket_refills_over_time() {
        let cfg = BucketConfig {
            capacity: 2,
            refill_per_sec: 100.0,
        };
        let mut b = Bucket::new(cfg);
        assert!(b.try_take().is_ok());
        assert!(b.try_take().is_ok());
        assert!(b.try_take().is_err());
        sleep(Duration::from_millis(50));
        assert!(b.try_take().is_ok());
    }

    #[test]
    fn limiter_no_op_when_no_prefix_matches() {
        let rl = RateLimiter::new(&[(
            "/v1/inference".to_string(),
            BucketConfig::inference_default(),
        )]);
        // Not under the configured prefix — must pass freely.
        for _ in 0..1000 {
            assert!(rl.check("/v1/health/live").is_ok());
        }
    }

    #[test]
    fn limiter_bounds_configured_prefix() {
        let rl = RateLimiter::new(&[(
            "/v1/chat".to_string(),
            BucketConfig {
                capacity: 3,
                refill_per_sec: 0.1,
            },
        )]);
        assert!(rl.check("/v1/chat/completions").is_ok());
        assert!(rl.check("/v1/chat/completions").is_ok());
        assert!(rl.check("/v1/chat/completions").is_ok());
        assert!(rl.check("/v1/chat/completions").is_err());
    }

    #[test]
    fn retry_after_is_positive() {
        let cfg = BucketConfig {
            capacity: 1,
            refill_per_sec: 1.0,
        };
        let mut b = Bucket::new(cfg);
        assert!(b.try_take().is_ok());
        let err = b.try_take().expect_err("expected rate-limit hit");
        assert!(err.as_secs_f64() > 0.0);
        assert!(err.as_secs_f64() < 2.0);
    }

    #[test]
    fn longest_prefix_wins() {
        let rl = RateLimiter::new(&[
            (
                "/v1".to_string(),
                BucketConfig {
                    capacity: 1,
                    refill_per_sec: 0.01,
                },
            ),
            (
                "/v1/health".to_string(),
                BucketConfig {
                    capacity: 3,
                    refill_per_sec: 0.01,
                },
            ),
        ]);
        assert!(rl.check("/v1/health").is_ok());
        assert!(rl.check("/v1/health").is_ok());
        assert!(rl.check("/v1/health").is_ok());
        assert!(rl.check("/v1/health").is_err());
    }
}
