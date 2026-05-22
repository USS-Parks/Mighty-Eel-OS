//! Pre-Warming Strategy — speculative model loading to reduce promotion latency.
//!
//! The warmup strategy decides when to start loading the full model before
//! a promotion is explicitly triggered. This reduces the <8s target to
//! potentially <2s if the model is already warm.
//!
//! Strategies:
//! - **Adaptive**: if recent Sentinel requests are getting complex, start loading
//! - **Time-based**: during configured "likely active" hours, keep full model warm
//! - **Conservative**: only pre-warm after first promotion in a session (default)

use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::debug;

/// Pre-warming strategy selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WarmupStrategy {
    /// Only pre-warm after the first promotion in a session.
    Conservative,
    /// Pre-warm when recent requests show increasing complexity.
    Adaptive,
    /// Keep the full model warm during configured active hours.
    TimeBased,
    /// Never pre-warm (always promote cold).
    Never,
}

/// Configuration for the pre-warming subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmupConfig {
    /// Which strategy to use.
    pub strategy: WarmupStrategy,
    /// Number of consecutive complex requests before adaptive pre-warm triggers.
    pub adaptive_threshold: u32,
    /// Time window (in seconds) for the adaptive complexity check.
    pub adaptive_window_secs: u64,
    /// Start of active hours (hour of day, 0-23, UTC).
    pub active_hours_start: u8,
    /// End of active hours (hour of day, 0-23, UTC).
    pub active_hours_end: u8,
    /// How long to keep the model warm after the last promotion (seconds).
    pub keep_warm_secs: u64,
    /// Maximum time to keep pre-warming without a promotion (seconds).
    pub max_speculative_warm_secs: u64,
}

impl Default for WarmupConfig {
    fn default() -> Self {
        Self {
            strategy: WarmupStrategy::Conservative,
            adaptive_threshold: 3,
            adaptive_window_secs: 120,
            active_hours_start: 8,
            active_hours_end: 22,
            keep_warm_secs: 300,            // 5 minutes
            max_speculative_warm_secs: 600, // 10 minutes
        }
    }
}

/// Tracks request complexity over a sliding window for adaptive pre-warming.
pub struct WarmupDecider {
    config: WarmupConfig,
    /// Number of recent promotions in the adaptive window.
    recent_promotions: AtomicU32,
    /// Total requests seen since last state change.
    recent_requests: AtomicU32,
    /// Whether the full model is currently pre-warmed.
    is_warm: AtomicBool,
    /// When the current warm period started.
    warm_started: RwLock<Option<Instant>>,
    /// When the last promotion completed.
    last_promotion: RwLock<Option<Instant>>,
    /// Total pre-warm events triggered.
    total_prewarms: AtomicU64,
}

impl WarmupDecider {
    /// Create a new warmup decider with the given config.
    pub fn new(config: WarmupConfig) -> Self {
        Self {
            config,
            recent_promotions: AtomicU32::new(0),
            recent_requests: AtomicU32::new(0),
            is_warm: AtomicBool::new(false),
            warm_started: RwLock::new(None),
            last_promotion: RwLock::new(None),
            total_prewarms: AtomicU64::new(0),
        }
    }

    /// Record that a request was classified (any outcome).
    /// Used by the adaptive strategy to track request volume.
    pub fn record_request(&self) {
        self.recent_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that a promotion was triggered.
    pub fn record_promotion(&self) {
        self.recent_promotions.fetch_add(1, Ordering::Relaxed);
        *self.last_promotion.write().unwrap() = Some(Instant::now());
    }

    /// Reset the recent counters (called when adaptive window expires).
    pub fn reset_window(&self) {
        self.recent_promotions.store(0, Ordering::Relaxed);
        self.recent_requests.store(0, Ordering::Relaxed);
    }

    /// Decide whether pre-warming should start.
    ///
    /// Based on the configured strategy and current state.
    pub fn should_warm(&self) -> bool {
        if self.is_warm() {
            return false; // already warm
        }

        match self.config.strategy {
            WarmupStrategy::Never => false,
            WarmupStrategy::Conservative => {
                // Only warm if there was a previous promotion recently
                self.last_promotion
                    .read()
                    .unwrap()
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(self.config.keep_warm_secs))
            }
            WarmupStrategy::Adaptive => {
                let promos = self.recent_promotions.load(Ordering::Relaxed);
                let requests = self.recent_requests.load(Ordering::Relaxed);
                if requests == 0 {
                    return false;
                }
                // If promotion ratio exceeds threshold, consider warming
                let ratio = promos as f64 / requests as f64;
                ratio > 0.3 || promos >= self.config.adaptive_threshold
            }
            WarmupStrategy::TimeBased => {
                // Check if current hour is within active window
                // Uses a simplified approach without real timezone handling
                true // caller should gate on active hours check
            }
        }
    }

    /// Mark the full model as pre-warmed.
    /// Returns true if this was a new warm event, false if already warm.
    pub fn mark_warm(&self) -> bool {
        if self.is_warm.swap(true, Ordering::AcqRel) {
            return false; // already warm
        }
        *self.warm_started.write().unwrap() = Some(Instant::now());
        self.total_prewarms.fetch_add(1, Ordering::Relaxed);
        debug!("Pre-warm started");
        true
    }

    /// Mark the full model as no longer warm (e.g., on timeout or shutdown).
    pub fn mark_cold(&self) {
        self.is_warm.store(false, Ordering::Release);
        *self.warm_started.write().unwrap() = None;
        debug!("Pre-warm ended");
    }

    /// Whether the full model is currently pre-warmed.
    pub fn is_warm(&self) -> bool {
        self.is_warm.load(Ordering::Acquire)
    }

    /// Check if the pre-warm period has expired (speculative warm timed out).
    /// If so, marks cold and returns true.
    pub fn check_expired(&self) -> bool {
        if !self.is_warm() {
            return false;
        }
        let started = self.warm_started.read().unwrap();
        if let Some(start) = *started {
            let max_warm = Duration::from_secs(self.config.max_speculative_warm_secs);
            if start.elapsed() > max_warm {
                drop(started);
                self.mark_cold();
                return true;
            }
        }
        false
    }

    /// Check if the keep-warm period after a promotion has expired.
    pub fn keep_warm_expired(&self) -> bool {
        let last = self.last_promotion.read().unwrap();
        last.is_none_or(|t| t.elapsed() > Duration::from_secs(self.config.keep_warm_secs))
    }

    /// Check if current time is within active hours.
    /// Returns true if time-based check passes (or strategy is not time-based).
    pub fn is_active_hours(&self) -> bool {
        if self.config.strategy != WarmupStrategy::TimeBased {
            return true;
        }
        // Simplified: UTC hour check via SystemTime.
        // In production, use a timezone-aware approach.
        let now_utc_hour = {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            ((secs / 3600) % 24) as u8
        };
        now_utc_hour >= self.config.active_hours_start
            && now_utc_hour < self.config.active_hours_end
    }

    /// Total pre-warm events.
    pub fn total_prewarms(&self) -> u64 {
        self.total_prewarms.load(Ordering::Relaxed)
    }

    /// Access the current config.
    pub fn config(&self) -> &WarmupConfig {
        &self.config
    }

    /// Update the config at runtime.
    pub fn set_config(&mut self, config: WarmupConfig) {
        self.config = config;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conservative() -> WarmupDecider {
        WarmupDecider::new(WarmupConfig {
            strategy: WarmupStrategy::Conservative,
            keep_warm_secs: 3600, // 1 hour for test stability
            ..WarmupConfig::default()
        })
    }

    #[test]
    fn test_initial_state_cold() {
        let wd = conservative();
        assert!(!wd.is_warm());
        assert!(!wd.should_warm());
    }

    #[test]
    fn test_conservative_warms_after_promotion() {
        let wd = conservative();
        wd.record_promotion();
        assert!(wd.should_warm());
    }

    #[test]
    fn test_mark_warm_and_cold() {
        let wd = conservative();
        assert!(wd.mark_warm());
        assert!(wd.is_warm());
        // Second mark is no-op
        assert!(!wd.mark_warm());
        wd.mark_cold();
        assert!(!wd.is_warm());
    }

    #[test]
    fn test_adaptive_warmup_trigger() {
        let wd = WarmupDecider::new(WarmupConfig {
            strategy: WarmupStrategy::Adaptive,
            adaptive_threshold: 3,
            ..WarmupConfig::default()
        });
        // Not enough data yet
        assert!(!wd.should_warm());

        // 3 promotions out of 5 requests -> should warm
        for _ in 0..3 {
            wd.record_promotion();
        }
        wd.record_request();
        wd.record_request();
        assert!(wd.should_warm());
    }

    #[test]
    fn test_adaptive_ratio_based() {
        let wd = WarmupDecider::new(WarmupConfig {
            strategy: WarmupStrategy::Adaptive,
            adaptive_threshold: 10, // high threshold, ratio will trigger first
            ..WarmupConfig::default()
        });
        // 5 promotions out of 10 requests = 50% > 30% threshold
        for _ in 0..5 {
            wd.record_promotion();
        }
        for _ in 0..5 {
            wd.record_request();
        }
        assert!(wd.should_warm());
    }

    #[test]
    fn test_never_warmup() {
        let wd = WarmupDecider::new(WarmupConfig {
            strategy: WarmupStrategy::Never,
            ..WarmupConfig::default()
        });
        wd.record_promotion();
        assert!(!wd.should_warm());
    }

    #[test]
    fn test_check_expired() {
        let config = WarmupConfig {
            strategy: WarmupStrategy::Conservative,
            max_speculative_warm_secs: 0, // expire immediately
            ..WarmupConfig::default()
        };
        let wd = WarmupDecider::new(config);
        wd.mark_warm();
        // Should expire immediately (0 second max)
        std::thread::sleep(Duration::from_millis(1));
        assert!(wd.check_expired());
        assert!(!wd.is_warm());
    }

    #[test]
    fn test_no_expired_if_not_warm() {
        let wd = conservative();
        assert!(!wd.check_expired());
    }

    #[test]
    fn test_reset_window() {
        let wd = WarmupDecider::new(WarmupConfig {
            strategy: WarmupStrategy::Adaptive,
            ..WarmupConfig::default()
        });
        wd.record_promotion();
        wd.record_promotion();
        wd.reset_window();
        assert!(!wd.should_warm());
    }

    #[test]
    fn test_total_prewarms() {
        let wd = conservative();
        assert_eq!(wd.total_prewarms(), 0);
        wd.mark_warm();
        assert_eq!(wd.total_prewarms(), 1);
        wd.mark_cold();
        wd.mark_warm();
        assert_eq!(wd.total_prewarms(), 2);
    }

    #[test]
    fn test_keep_warm_expired() {
        let wd = conservative();
        // No previous promotion -> expired
        assert!(wd.keep_warm_expired());

        wd.record_promotion();
        // Just recorded, should not be expired
        assert!(!wd.keep_warm_expired());
    }

    #[test]
    fn test_already_warm_no_second_warm() {
        let wd = conservative();
        wd.mark_warm();
        assert!(!wd.should_warm()); // already warm
    }
}
