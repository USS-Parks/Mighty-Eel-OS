//! Air-gap verification module for the MAI API.
//!
//! # Design Philosophy
//!
//! Air-gap is NOT a feature flag. It is the default architectural constraint.
//! Network connectivity is the exception, not the rule. Every component must
//! function with zero network access.
//!
//! # Verification Layers
//!
//! 1. **Physical switch state**: Read from hardware via the SwitchReader trait.
//!    The physical switch is the ground truth. Software cannot override it.
//!
//! 2. **Network interface state**: Verify that network interfaces are actually
//!    down/disconnected, not just logically disabled.
//!
//! 3. **Periodic re-verification**: Every 60 seconds, re-check both layers.
//!    If a discrepancy is detected, log an alert and optionally trigger
//!    protective actions (kill network-capable adapters, etc.).
//!
//! # No `if air_gap_mode:` Conditionals
//!
//! Per HANDOFF.md item #3: "If you find yourself writing `if air_gap_mode:`
//! conditionals, you have already failed. The default is air-gapped. Network
//! is the exception."

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time;
use tracing::{debug, error, info, warn};

// ── Switch State ──────────────────────────────────────────────────────

/// Physical air-gap switch position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwitchPosition {
    /// Air-gapped: no network connectivity permitted.
    AirGapped,

    /// Network enabled: controlled connectivity allowed.
    NetworkEnabled,

    /// Unknown: switch state could not be determined.
    /// Treated as AirGapped for safety.
    Unknown,
}

impl SwitchPosition {
    /// Whether this position allows network connectivity.
    pub fn network_allowed(&self) -> bool {
        matches!(self, SwitchPosition::NetworkEnabled)
    }

    /// Whether this position requires air-gap enforcement.
    /// Unknown is treated as air-gapped (fail-safe).
    pub fn is_air_gapped(&self) -> bool {
        !self.network_allowed()
    }
}

impl std::fmt::Display for SwitchPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwitchPosition::AirGapped => write!(f, "air-gapped"),
            SwitchPosition::NetworkEnabled => write!(f, "network-enabled"),
            SwitchPosition::Unknown => write!(f, "unknown (treating as air-gapped)"),
        }
    }
}

// ── Network Interface State ───────────────────────────────────────────

/// State of network interfaces on the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInterfaceState {
    /// Whether any non-loopback interface has an active link.
    pub any_link_active: bool,

    /// Number of non-loopback interfaces detected.
    pub interface_count: u32,

    /// Number of interfaces with active links.
    pub active_count: u32,

    /// Names of active interfaces (for diagnostics).
    pub active_interfaces: Vec<String>,
}

impl NetworkInterfaceState {
    /// Whether network interfaces are consistent with air-gap mode.
    pub fn consistent_with_air_gap(&self) -> bool {
        !self.any_link_active
    }
}

// ── Switch Reader Trait ───────────────────────────────────────────────

/// Trait for reading the physical air-gap switch state.
///
/// Implementations read from:
/// - GPIO pin (production hardware)
/// - sysfs entry (Linux-based systems)
/// - Configuration file (development/testing)
///
/// All reads must be local. No network calls.
#[async_trait::async_trait]
pub trait SwitchReader: Send + Sync + 'static {
    /// Read the current switch position.
    async fn read_position(&self) -> Result<SwitchPosition, String>;

    /// Read the current network interface state.
    async fn read_network_state(&self) -> Result<NetworkInterfaceState, String>;
}

/// Development switch reader that returns a configurable position.
///
/// Defaults to AirGapped. Used in development and testing.
#[derive(Debug)]
pub struct DevSwitchReader {
    position: RwLock<SwitchPosition>,
    network_state: RwLock<NetworkInterfaceState>,
}

impl DevSwitchReader {
    pub fn new() -> Self {
        Self {
            position: RwLock::new(SwitchPosition::AirGapped),
            network_state: RwLock::new(NetworkInterfaceState {
                any_link_active: false,
                interface_count: 1,
                active_count: 0,
                active_interfaces: vec![],
            }),
        }
    }

    /// Set the simulated switch position (for testing).
    pub async fn set_position(&self, position: SwitchPosition) {
        let mut pos = self.position.write().await;
        *pos = position;
    }

    /// Set the simulated network state (for testing).
    pub async fn set_network_state(&self, state: NetworkInterfaceState) {
        let mut ns = self.network_state.write().await;
        *ns = state;
    }
}

impl Default for DevSwitchReader {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl SwitchReader for DevSwitchReader {
    async fn read_position(&self) -> Result<SwitchPosition, String> {
        let pos = self.position.read().await;
        Ok(*pos)
    }

    async fn read_network_state(&self) -> Result<NetworkInterfaceState, String> {
        let state = self.network_state.read().await;
        Ok(state.clone())
    }
}

// ── Verification Result ───────────────────────────────────────────────

/// Result of an air-gap verification check.
#[derive(Debug, Clone)]
pub struct VerificationResult {
    /// Whether the system is verified air-gapped.
    pub air_gapped: bool,

    /// Physical switch position.
    pub switch_position: SwitchPosition,

    /// Network interface state.
    pub network_state: NetworkInterfaceState,

    /// Whether switch and network state are consistent.
    pub consistent: bool,

    /// Timestamp of this verification.
    pub verified_at: Instant,

    /// Human-readable status message.
    pub message: String,

    /// Any anomalies detected.
    pub anomalies: Vec<String>,
}

impl VerificationResult {
    fn new(switch_position: SwitchPosition, network_state: NetworkInterfaceState) -> Self {
        let mut anomalies = Vec::new();

        // Check consistency between switch and network
        let consistent = if switch_position.is_air_gapped() && network_state.any_link_active {
            anomalies.push(format!(
                "Switch is {} but {} network interface(s) active: {:?}",
                switch_position, network_state.active_count, network_state.active_interfaces
            ));
            false
        } else if switch_position.network_allowed() && !network_state.any_link_active {
            // Not an anomaly per se, just informational
            debug!("Switch allows network but no interfaces active");
            true
        } else {
            true
        };

        let air_gapped = switch_position.is_air_gapped();

        let message = if air_gapped && consistent {
            "Air-gap verified: switch engaged, no active network interfaces".to_string()
        } else if air_gapped && !consistent {
            "Air-gap INCONSISTENT: switch engaged but network interfaces detected active"
                .to_string()
        } else if !air_gapped && consistent {
            "Network mode: switch disengaged, network available".to_string()
        } else {
            "Network mode: switch disengaged".to_string()
        };

        Self {
            air_gapped,
            switch_position,
            network_state,
            consistent,
            verified_at: Instant::now(),
            message,
            anomalies,
        }
    }
}

// ── Air-Gap Checker ───────────────────────────────────────────────────

/// Air-gap verification manager.
///
/// Performs startup verification and periodic re-checks. Maintains
/// the most recent verification result for health reporting.
pub struct AirGapChecker {
    reader: Arc<dyn SwitchReader>,
    last_result: RwLock<Option<VerificationResult>>,
    check_interval: Duration,
}

impl AirGapChecker {
    /// Create a new checker with the given switch reader and check interval.
    pub fn new(reader: Arc<dyn SwitchReader>, check_interval: Duration) -> Self {
        Self {
            reader,
            last_result: RwLock::new(None),
            check_interval,
        }
    }

    /// Create a checker with the default 60-second interval.
    pub fn with_default_interval(reader: Arc<dyn SwitchReader>) -> Self {
        Self::new(reader, Duration::from_secs(60))
    }

    /// Perform a single verification check.
    pub async fn verify(&self) -> Result<VerificationResult, String> {
        let switch_position = self.reader.read_position().await?;
        let network_state = self.reader.read_network_state().await?;

        let result = VerificationResult::new(switch_position, network_state);

        if !result.consistent {
            warn!(
                switch = %result.switch_position,
                anomalies = ?result.anomalies,
                "Air-gap verification inconsistency detected"
            );
        }

        // Update cached result
        let mut last = self.last_result.write().await;
        *last = Some(result.clone());

        Ok(result)
    }

    /// Perform startup verification.
    ///
    /// This is called once during server initialization. If the system
    /// is air-gapped, logs confirmation. If inconsistent, logs a warning
    /// but does not prevent startup (the periodic checker will continue
    /// monitoring).
    pub async fn startup_check(&self) -> Result<VerificationResult, String> {
        info!("Performing air-gap startup verification");

        let result = self.verify().await?;

        match (result.air_gapped, result.consistent) {
            (true, true) => {
                info!("Air-gap startup verification PASSED: {}", result.message);
            }
            (true, false) => {
                warn!("Air-gap startup verification WARNING: {}", result.message);
                for anomaly in &result.anomalies {
                    warn!("  Anomaly: {}", anomaly);
                }
            }
            (false, _) => {
                info!("Network mode startup verification: {}", result.message);
            }
        }

        Ok(result)
    }

    /// Get the most recent verification result.
    pub async fn last_verification(&self) -> Option<VerificationResult> {
        let last = self.last_result.read().await;
        last.clone()
    }

    /// Check if the most recent verification is stale (older than 2x interval).
    pub async fn is_stale(&self) -> bool {
        let last = self.last_result.read().await;
        match &*last {
            None => true,
            Some(result) => result.verified_at.elapsed() > self.check_interval * 2,
        }
    }

    /// Start the periodic verification loop.
    ///
    /// This spawns a background task that re-verifies at the configured
    /// interval. Returns a JoinHandle that can be used to cancel.
    pub fn start_periodic(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let checker = Arc::clone(self);
        let interval = checker.check_interval;

        tokio::spawn(async move {
            let mut tick = time::interval(interval);
            tick.tick().await; // Skip immediate first tick (startup_check handles it)

            loop {
                tick.tick().await;

                match checker.verify().await {
                    Ok(result) => {
                        if !result.consistent {
                            error!("Periodic air-gap check FAILED: {}", result.message);
                        } else {
                            debug!(
                                air_gapped = result.air_gapped,
                                "Periodic air-gap check passed"
                            );
                        }
                    }
                    Err(e) => {
                        error!(
                            error = %e,
                            "Failed to read air-gap state during periodic check"
                        );
                    }
                }
            }
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_switch_position_safety() {
        assert!(SwitchPosition::AirGapped.is_air_gapped());
        assert!(!SwitchPosition::AirGapped.network_allowed());

        assert!(!SwitchPosition::NetworkEnabled.is_air_gapped());
        assert!(SwitchPosition::NetworkEnabled.network_allowed());

        // Unknown defaults to air-gapped (fail-safe)
        assert!(SwitchPosition::Unknown.is_air_gapped());
        assert!(!SwitchPosition::Unknown.network_allowed());
    }

    #[tokio::test]
    async fn test_dev_reader_defaults_air_gapped() {
        let reader = DevSwitchReader::new();
        let pos = reader.read_position().await.unwrap();
        assert_eq!(pos, SwitchPosition::AirGapped);

        let net = reader.read_network_state().await.unwrap();
        assert!(!net.any_link_active);
    }

    #[tokio::test]
    async fn test_verification_consistent_air_gap() {
        let reader = Arc::new(DevSwitchReader::new());
        let checker = AirGapChecker::with_default_interval(reader);

        let result = checker.verify().await.unwrap();
        assert!(result.air_gapped);
        assert!(result.consistent);
        assert!(result.anomalies.is_empty());
    }

    #[tokio::test]
    async fn test_verification_inconsistent_state() {
        let reader = Arc::new(DevSwitchReader::new());
        // Switch is air-gapped but network is active
        reader
            .set_network_state(NetworkInterfaceState {
                any_link_active: true,
                interface_count: 2,
                active_count: 1,
                active_interfaces: vec!["eth0".to_string()],
            })
            .await;

        let checker = AirGapChecker::with_default_interval(reader);
        let result = checker.verify().await.unwrap();
        assert!(result.air_gapped); // Switch still says air-gapped
        assert!(!result.consistent); // But network contradicts
        assert!(!result.anomalies.is_empty());
    }

    #[tokio::test]
    async fn test_network_mode_verification() {
        let reader = Arc::new(DevSwitchReader::new());
        reader.set_position(SwitchPosition::NetworkEnabled).await;
        reader
            .set_network_state(NetworkInterfaceState {
                any_link_active: true,
                interface_count: 2,
                active_count: 1,
                active_interfaces: vec!["eth0".to_string()],
            })
            .await;

        let checker = AirGapChecker::with_default_interval(reader);
        let result = checker.verify().await.unwrap();
        assert!(!result.air_gapped);
        assert!(result.consistent);
    }

    #[tokio::test]
    async fn test_startup_check() {
        let reader = Arc::new(DevSwitchReader::new());
        let checker = AirGapChecker::with_default_interval(reader);

        let result = checker.startup_check().await.unwrap();
        assert!(result.air_gapped);
        assert!(result.consistent);

        // Verify last_verification is populated
        let last = checker.last_verification().await;
        assert!(last.is_some());
    }

    #[tokio::test]
    async fn test_staleness_detection() {
        let reader = Arc::new(DevSwitchReader::new());
        let checker = AirGapChecker::new(reader, Duration::from_millis(10));

        // Before any check, should be stale
        assert!(checker.is_stale().await);

        // After check, should not be stale
        checker.verify().await.unwrap();
        assert!(!checker.is_stale().await);

        // After waiting 2x interval, should be stale
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(checker.is_stale().await);
    }

    #[tokio::test]
    async fn test_switch_position_transition() {
        let reader = Arc::new(DevSwitchReader::new());
        let checker = AirGapChecker::with_default_interval(reader.clone());

        // Start air-gapped
        let r1 = checker.verify().await.unwrap();
        assert!(r1.air_gapped);

        // Transition to network mode
        reader.set_position(SwitchPosition::NetworkEnabled).await;
        reader
            .set_network_state(NetworkInterfaceState {
                any_link_active: true,
                interface_count: 1,
                active_count: 1,
                active_interfaces: vec!["eth0".to_string()],
            })
            .await;

        let r2 = checker.verify().await.unwrap();
        assert!(!r2.air_gapped);
        assert!(r2.consistent);
    }
}
