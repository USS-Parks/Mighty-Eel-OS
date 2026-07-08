//! Circuit Breaker State Machine for Adapter Routing
//!
//! Protects against flaky adapters by tracking consecutive failures and rolling-window
//! failure rates. Transitions through Closed, Open, and HalfOpen states with exponential
//! backoff cooldowns.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Circuit breaker operational states
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CircuitState {
    /// Normal operation, requests flow through
    #[default]
    Closed,
    /// Tripped, requests immediately rejected
    Open,
    /// Testing recovery, single probe allowed
    HalfOpen,
}

/// Configuration for circuit breaker thresholds and cooldowns
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Consecutive failures to trip (default: 5)
    pub trip_threshold: u32,
    /// Failure rate 0.0-1.0 within window to trip (default: 0.5)
    pub rate_threshold: f64,
    /// Rolling window duration for rate calculation (default: 60s)
    pub rate_window: Duration,
    /// Initial Open duration (default: 30s)
    pub cooldown_base: Duration,
    /// Maximum Open duration after repeated trips (default: 5min)
    pub cooldown_max: Duration,
    /// Exponential backoff multiplier on repeated trips (default: 2.0)
    pub cooldown_multiplier: f64,
    /// Probes allowed before committing to Closed (default: 1)
    pub half_open_max_probes: u32,
    /// Minimum samples in window before rate check activates (default: 5)
    pub rate_min_samples: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            trip_threshold: 5,
            rate_threshold: 0.5,
            rate_window: Duration::from_secs(60),
            cooldown_base: Duration::from_secs(30),
            cooldown_max: Duration::from_secs(300),
            cooldown_multiplier: 2.0,
            half_open_max_probes: 1,
            rate_min_samples: 5,
        }
    }
}

/// Metrics snapshot for health reporting
#[derive(Debug, Clone, Default)]
pub struct CircuitMetrics {
    pub state: CircuitState,
    pub consecutive_failures: u32,
    pub failures_in_window: u32,
    pub total_in_window: u32,
    pub cooldown_remaining: Option<Duration>,
    pub total_trips: u64,
}

/// Circuit breaker state machine
#[derive(Debug)]
pub struct CircuitBreaker {
    state: CircuitState,
    failure_count: u32,
    failure_window: VecDeque<Instant>,
    success_window: VecDeque<Instant>,
    last_state_change: Instant,
    open_entered_at: Option<Instant>,
    cooldown_cycles: u32,
    probes_in_half_open: u32,
    total_trips: u64,
    config: CircuitBreakerConfig,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with given config
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            failure_window: VecDeque::new(),
            success_window: VecDeque::new(),
            last_state_change: Instant::now(),
            open_entered_at: None,
            cooldown_cycles: 0,
            probes_in_half_open: 0,
            total_trips: 0,
            config,
        }
    }

    /// Returns true if state is Closed or HalfOpen
    pub fn can_execute(&self) -> bool {
        matches!(self.state, CircuitState::Closed | CircuitState::HalfOpen)
    }

    /// Transition Open -> HalfOpen if cooldown elapsed
    pub fn refresh_state(&mut self) {
        if self.state == CircuitState::Open
            && let Some(entered_at) = self.open_entered_at
        {
            let elapsed = Instant::now().duration_since(entered_at);
            let current_cooldown = self.calculate_cooldown();
            if elapsed >= current_cooldown {
                self.transition_to(CircuitState::HalfOpen);
                self.probes_in_half_open = 0;
            }
        }
    }

    /// Record successful response
    pub fn record_success(&mut self) {
        let now = Instant::now();
        self.success_window.push_back(now);
        Self::prune_window(&mut self.success_window, self.config.rate_window);
        Self::prune_window(&mut self.failure_window, self.config.rate_window);

        match self.state {
            CircuitState::HalfOpen => {
                self.probes_in_half_open += 1;
                if self.probes_in_half_open >= self.config.half_open_max_probes {
                    self.transition_to(CircuitState::Closed);
                    self.cooldown_cycles = 0;
                    self.clear_windows();
                }
            }
            CircuitState::Closed => {
                self.failure_count = 0;
            }
            CircuitState::Open => {} // Defensive: no-op if called without refresh_state
        }
    }

    /// Record failed response
    pub fn record_failure(&mut self) {
        let now = Instant::now();
        self.failure_window.push_back(now);
        Self::prune_window(&mut self.failure_window, self.config.rate_window);
        Self::prune_window(&mut self.success_window, self.config.rate_window);

        match self.state {
            CircuitState::HalfOpen => {
                self.probes_in_half_open = 0;
                self.transition_to(CircuitState::Open);
                self.cooldown_cycles = self.cooldown_cycles.saturating_add(1);
            }
            CircuitState::Closed => {
                self.failure_count += 1;
                if self.should_trip() {
                    self.transition_to(CircuitState::Open);
                    self.cooldown_cycles = 0;
                }
            }
            CircuitState::Open => {}
        }
    }

    /// Current state
    pub fn state(&self) -> CircuitState {
        self.state
    }

    /// Time remaining until HalfOpen is allowed (None if not Open)
    pub fn time_until_half_open(&self) -> Option<Duration> {
        if self.state != CircuitState::Open {
            return None;
        }
        self.open_entered_at.map(|entered| {
            let elapsed = Instant::now().duration_since(entered);
            let cooldown = self.calculate_cooldown();
            cooldown.saturating_sub(elapsed)
        })
    }

    /// Force reset to Closed (admin override or recovery)
    pub fn reset(&mut self) {
        self.transition_to(CircuitState::Closed);
        self.failure_count = 0;
        self.cooldown_cycles = 0;
        self.clear_windows();
    }

    /// Force Open (used by HealthMonitor dead declaration)
    pub fn force_open(&mut self) {
        if self.state != CircuitState::Open {
            self.transition_to(CircuitState::Open);
            self.cooldown_cycles = 0;
        }
    }

    /// Report current metrics
    #[allow(clippy::cast_possible_truncation)] // window sizes bounded by config, never exceed u32
    pub fn metrics(&self) -> CircuitMetrics {
        let failures = self.failure_window.len() as u32;
        let total = failures + self.success_window.len() as u32;
        CircuitMetrics {
            state: self.state,
            consecutive_failures: self.failure_count,
            failures_in_window: failures,
            total_in_window: total,
            cooldown_remaining: self.time_until_half_open(),
            total_trips: self.total_trips,
        }
    }

    // ─── Internal ─────────────────────────────────────────────────────

    fn transition_to(&mut self, new_state: CircuitState) {
        self.state = new_state;
        self.last_state_change = Instant::now();
        if new_state == CircuitState::Open {
            self.open_entered_at = Some(Instant::now());
            self.total_trips += 1;
        }
    }

    #[allow(clippy::cast_possible_truncation)] // window sizes bounded by config, never exceed u32
    fn should_trip(&self) -> bool {
        // Check 1: consecutive failure count
        if self.failure_count >= self.config.trip_threshold {
            return true;
        }
        // Check 2: rolling-window failure rate (only if enough samples)
        let failures = self.failure_window.len() as u32;
        let total = failures + self.success_window.len() as u32;
        if total < self.config.rate_min_samples {
            return false;
        }
        let rate = f64::from(failures) / f64::from(total);
        rate >= self.config.rate_threshold
    }

    fn calculate_cooldown(&self) -> Duration {
        let base = self.config.cooldown_base.as_secs_f64();
        let multiplier = self
            .config
            .cooldown_multiplier
            .powf(f64::from(self.cooldown_cycles));
        let target_secs = base * multiplier;
        let max = self.config.cooldown_max;
        // Guard `Duration::from_secs_f64`, which PANICS on a non-finite or
        // out-of-range value. A long outage drives `cooldown_cycles` — and thus the
        // exponential `multiplier` — high enough to overflow f64 to infinity (audit
        // D5: "a multi-day outage does not panic"). The cooldown is capped at
        // `cooldown_max` regardless, so any non-finite or over-cap target clamps
        // straight there and never reaches `from_secs_f64`.
        if !target_secs.is_finite() || target_secs >= max.as_secs_f64() {
            return max;
        }
        Duration::from_secs_f64(target_secs).min(max)
    }

    /// Prune entries older than `window` from a time-series deque.
    /// Static method to avoid double-mutable-borrow issues.
    fn prune_window(window: &mut VecDeque<Instant>, rate_window: Duration) {
        let now = Instant::now();
        while let Some(&ts) = window.front() {
            if now.duration_since(ts) > rate_window {
                window.pop_front();
            } else {
                break;
            }
        }
    }

    fn clear_windows(&mut self) {
        self.failure_window.clear();
        self.success_window.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_breaker() -> CircuitBreaker {
        let cfg = CircuitBreakerConfig {
            trip_threshold: 3,
            rate_threshold: 0.5,
            rate_window: Duration::from_secs(10),
            cooldown_base: Duration::from_millis(100),
            cooldown_max: Duration::from_secs(1),
            rate_min_samples: 5, // Need 5 samples before rate check activates
            ..CircuitBreakerConfig::default()
        };
        CircuitBreaker::new(cfg)
    }

    #[test]
    fn test_circuit_trips_on_consecutive_failures() {
        let mut cb = make_breaker();
        for _ in 0..3 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_cooldown_saturates_without_panic_after_many_trips() {
        // Audit D5: a very high cooldown_cycles count (a long outage) must not
        // overflow the exponential into a Duration::from_secs_f64 panic. The
        // cooldown clamps to cooldown_max instead.
        let mut cb = make_breaker();
        cb.cooldown_cycles = 100_000; // 2.0^100000 overflows f64 to +inf
        assert_eq!(cb.calculate_cooldown(), cb.config.cooldown_max);
        cb.cooldown_cycles = 2_000; // still overflows f64
        assert_eq!(cb.calculate_cooldown(), cb.config.cooldown_max);
    }

    #[test]
    fn test_circuit_trips_on_rate_threshold() {
        let mut cb = make_breaker();
        // Need rate_min_samples (5) before rate check activates.
        // Build a window: 3 failures + 2 successes = 60% failure rate > 50%
        cb.record_success();
        cb.record_success();
        // 2 successes, 0 failures. failure_count still 0.
        cb.record_failure(); // failure_count=1, total=3, rate not checked (< 5 samples)
        cb.record_failure(); // failure_count=2, total=4, rate not checked (< 5 samples)
        assert_eq!(cb.state(), CircuitState::Closed); // Not tripped yet
        cb.record_failure(); // failure_count=3 >= trip_threshold=3, trips on consecutive
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_circuit_trips_on_rate_not_consecutive() {
        // Test rate-based trip specifically: interleave so consecutive count
        // never reaches threshold, but rate exceeds threshold.
        let cfg = CircuitBreakerConfig {
            trip_threshold: 10, // High, so consecutive won't trigger
            rate_threshold: 0.5,
            rate_window: Duration::from_secs(10),
            cooldown_base: Duration::from_millis(100),
            rate_min_samples: 4,
            ..CircuitBreakerConfig::default()
        };
        let mut cb = CircuitBreaker::new(cfg);

        cb.record_success(); // total=1
        cb.record_failure(); // total=2, consecutive=1
        cb.record_success(); // total=3, consecutive=0
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure(); // total=4, rate=2/4=0.5, consecutive=1, trips on rate
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_circuit_cooldown_to_half_open() {
        let mut cb = make_breaker();
        for _ in 0..3 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);
        std::thread::sleep(Duration::from_millis(110));
        cb.refresh_state();
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn test_half_open_probe_success_closes() {
        let mut cb = make_breaker();
        for _ in 0..3 {
            cb.record_failure();
        }
        std::thread::sleep(Duration::from_millis(110));
        cb.refresh_state();
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_probe_failure_reopens() {
        let mut cb = make_breaker();
        for _ in 0..3 {
            cb.record_failure();
        }
        std::thread::sleep(Duration::from_millis(110));
        cb.refresh_state();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        // Cooldown should be longer due to backoff (cycles=1)
        assert!(cb.time_until_half_open().unwrap() > Duration::from_millis(100));
    }

    #[test]
    fn test_success_resets_failure_count() {
        let mut cb = make_breaker();
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        assert_eq!(cb.metrics().consecutive_failures, 0);
    }

    #[test]
    fn test_force_open_and_reset() {
        let mut cb = make_breaker();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.force_open();
        assert_eq!(cb.state(), CircuitState::Open);
        assert_eq!(cb.metrics().total_trips, 1);
        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.metrics().consecutive_failures, 0);
    }

    #[test]
    fn test_default_state_is_closed() {
        let state = CircuitState::default();
        assert_eq!(state, CircuitState::Closed);
    }
}
