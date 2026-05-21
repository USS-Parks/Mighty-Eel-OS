//! Dual-threshold admission control for continuous batching.
//!
//! Decides whether a queued sequence should be admitted to the active batch
//! based on current VRAM pressure. Three operating regions:
//!
//! - **Aggressive** (VRAM < 80%): admit immediately if batch has room
//! - **Selective** (VRAM 80-90%): admit only short sequences or high priority
//! - **Eviction-required** (VRAM > 90%): must evict before admitting
//!
//! Thresholds are configurable per-instance via `AdmissionConfig`.

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::types::Priority;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Admission control thresholds and tuning parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdmissionConfig {
    /// VRAM usage fraction below which admission is aggressive (default 0.80).
    #[serde(default = "default_aggressive_threshold")]
    pub aggressive_threshold: f64,

    /// VRAM usage fraction above which admission requires eviction (default 0.90).
    #[serde(default = "default_eviction_threshold")]
    pub eviction_threshold: f64,

    /// In selective mode, maximum prompt tokens for a sequence to be admitted
    /// without requiring high priority (default 512).
    #[serde(default = "default_selective_max_tokens")]
    pub selective_max_tokens: u32,

    /// In selective mode, minimum priority for admission of long sequences.
    /// Sequences at or above this priority are admitted regardless of length.
    /// 0 = System, 1 = High, 2 = Normal, 3 = Background.
    /// Default: 1 (High priority and above bypass the token check).
    #[serde(default = "default_selective_min_priority")]
    pub selective_min_priority: u8,
}

fn default_aggressive_threshold() -> f64 {
    0.80
}
fn default_eviction_threshold() -> f64 {
    0.90
}
fn default_selective_max_tokens() -> u32 {
    512
}
fn default_selective_min_priority() -> u8 {
    1 // High
}

impl Default for AdmissionConfig {
    fn default() -> Self {
        Self {
            aggressive_threshold: default_aggressive_threshold(),
            eviction_threshold: default_eviction_threshold(),
            selective_max_tokens: default_selective_max_tokens(),
            selective_min_priority: default_selective_min_priority(),
        }
    }
}

// ---------------------------------------------------------------------------
// Admission decision
// ---------------------------------------------------------------------------

/// Result of an admission check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionDecision {
    /// Admit immediately: VRAM is available and the sequence qualifies.
    Admit,
    /// Admit after eviction: VRAM is tight but evicting idle sequences
    /// can make room. The caller should invoke eviction before admitting.
    AdmitAfterEviction,
    /// Reject: the sequence does not qualify under current pressure.
    /// It stays in the waiting queue for the next build step.
    Reject,
}

// ---------------------------------------------------------------------------
// Admission controller
// ---------------------------------------------------------------------------

/// Per-instance admission controller. Stateless beyond config: the VRAM
/// state is passed in on each call so there's nothing to synchronize.
#[derive(Debug, Clone)]
pub struct AdmissionController {
    config: AdmissionConfig,
}

impl AdmissionController {
    /// Create a new admission controller with the given configuration.
    pub fn new(config: AdmissionConfig) -> Self {
        Self { config }
    }

    /// Update configuration at runtime (config reload).
    pub fn update_config(&mut self, config: AdmissionConfig) {
        self.config = config;
    }

    /// Current configuration (for introspection).
    pub fn config(&self) -> &AdmissionConfig {
        &self.config
    }

    /// Check whether a sequence should be admitted to the active batch.
    ///
    /// Arguments:
    /// - `vram_usage_fraction`: current VRAM used / total VRAM (0.0..1.0)
    /// - `prompt_tokens`: estimated prompt token count for the candidate
    /// - `priority`: request priority
    /// - `kv_can_fit`: whether the KV cache manager reports enough room
    /// - `batch_has_room`: whether the batch has an open slot
    pub fn check(
        &self,
        vram_usage_fraction: f64,
        prompt_tokens: u32,
        priority: Priority,
        kv_can_fit: bool,
        batch_has_room: bool,
    ) -> AdmissionDecision {
        // No room in the batch at all: reject regardless of VRAM
        if !batch_has_room {
            debug!(
                vram_frac = vram_usage_fraction,
                tokens = prompt_tokens,
                "Admission rejected: batch full"
            );
            return AdmissionDecision::Reject;
        }

        // Region 1: Aggressive (VRAM < aggressive_threshold)
        if vram_usage_fraction < self.config.aggressive_threshold {
            if kv_can_fit {
                return AdmissionDecision::Admit;
            }
            // KV reports no room even though overall VRAM is low.
            // This can happen if the KV budget is smaller than total VRAM.
            // Try eviction path.
            return AdmissionDecision::AdmitAfterEviction;
        }

        // Region 2: Selective (aggressive_threshold <= VRAM < eviction_threshold)
        if vram_usage_fraction < self.config.eviction_threshold {
            let priority_qualifies =
                (priority as u8) <= self.config.selective_min_priority;
            let short_enough = prompt_tokens <= self.config.selective_max_tokens;

            if priority_qualifies || short_enough {
                if kv_can_fit {
                    return AdmissionDecision::Admit;
                }
                return AdmissionDecision::AdmitAfterEviction;
            }

            debug!(
                vram_frac = vram_usage_fraction,
                tokens = prompt_tokens,
                priority = %priority,
                "Admission rejected: selective mode, sequence too long/low priority"
            );
            return AdmissionDecision::Reject;
        }

        // Region 3: Eviction-required (VRAM >= eviction_threshold)
        // Always require eviction before admission. System priority
        // sequences still go through eviction path (they'll just evict
        // lower-priority victims).
        if kv_can_fit {
            // Rare: VRAM fraction is high but KV says there's room.
            // This means non-KV VRAM consumption is dominating. Still
            // flag as eviction path since overall pressure is critical.
            debug!(
                vram_frac = vram_usage_fraction,
                "High VRAM but KV has room; admitting via eviction path"
            );
        }
        AdmissionDecision::AdmitAfterEviction
    }

    /// Convenience: compute VRAM usage fraction from used/total bytes.
    pub fn vram_fraction(used_bytes: u64, total_bytes: u64) -> f64 {
        if total_bytes == 0 {
            return 1.0; // treat zero budget as full
        }
        used_bytes as f64 / total_bytes as f64
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_controller() -> AdmissionController {
        AdmissionController::new(AdmissionConfig::default())
    }

    #[test]
    fn test_aggressive_admit() {
        let ctrl = make_controller();
        // VRAM at 50%, KV has room, batch has room
        let decision = ctrl.check(0.50, 2048, Priority::Normal, true, true);
        assert_eq!(decision, AdmissionDecision::Admit);
    }

    #[test]
    fn test_aggressive_no_kv_room() {
        let ctrl = make_controller();
        // VRAM at 50% but KV can't fit -> eviction path
        let decision = ctrl.check(0.50, 2048, Priority::Normal, false, true);
        assert_eq!(decision, AdmissionDecision::AdmitAfterEviction);
    }

    #[test]
    fn test_batch_full_always_rejects() {
        let ctrl = make_controller();
        // No room in batch, regardless of VRAM
        let decision = ctrl.check(0.10, 256, Priority::System, true, false);
        assert_eq!(decision, AdmissionDecision::Reject);
    }

    #[test]
    fn test_selective_short_sequence_admitted() {
        let ctrl = make_controller();
        // VRAM at 85%, short sequence (256 tokens < 512 threshold)
        let decision = ctrl.check(0.85, 256, Priority::Normal, true, true);
        assert_eq!(decision, AdmissionDecision::Admit);
    }

    #[test]
    fn test_selective_high_priority_admitted() {
        let ctrl = make_controller();
        // VRAM at 85%, long sequence but High priority
        let decision = ctrl.check(0.85, 4096, Priority::High, true, true);
        assert_eq!(decision, AdmissionDecision::Admit);
    }

    #[test]
    fn test_selective_system_priority_admitted() {
        let ctrl = make_controller();
        // VRAM at 85%, any length, System priority
        let decision = ctrl.check(0.85, 8192, Priority::System, true, true);
        assert_eq!(decision, AdmissionDecision::Admit);
    }

    #[test]
    fn test_selective_long_normal_rejected() {
        let ctrl = make_controller();
        // VRAM at 85%, long sequence, Normal priority -> rejected
        let decision = ctrl.check(0.85, 4096, Priority::Normal, true, true);
        assert_eq!(decision, AdmissionDecision::Reject);
    }

    #[test]
    fn test_selective_background_rejected() {
        let ctrl = make_controller();
        // VRAM at 85%, short but Background still qualifies (short_enough check)
        let decision = ctrl.check(0.85, 256, Priority::Background, true, true);
        assert_eq!(decision, AdmissionDecision::Admit);

        // Long Background -> rejected
        let decision = ctrl.check(0.85, 4096, Priority::Background, true, true);
        assert_eq!(decision, AdmissionDecision::Reject);
    }

    #[test]
    fn test_eviction_required_region() {
        let ctrl = make_controller();
        // VRAM at 95%, KV has no room -> eviction required
        let decision = ctrl.check(0.95, 512, Priority::Normal, false, true);
        assert_eq!(decision, AdmissionDecision::AdmitAfterEviction);
    }

    #[test]
    fn test_eviction_required_even_with_kv_room() {
        let ctrl = make_controller();
        // VRAM at 92%, KV claims room -> still eviction path (overall pressure)
        let decision = ctrl.check(0.92, 512, Priority::Normal, true, true);
        assert_eq!(decision, AdmissionDecision::AdmitAfterEviction);
    }

    #[test]
    fn test_vram_fraction_calculation() {
        assert!((AdmissionController::vram_fraction(800, 1000) - 0.8).abs() < f64::EPSILON);
        assert!((AdmissionController::vram_fraction(0, 1000) - 0.0).abs() < f64::EPSILON);
        assert!((AdmissionController::vram_fraction(0, 0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_boundary_aggressive_selective() {
        let ctrl = make_controller();
        // Exactly at 0.80 -> selective (not aggressive, >= threshold)
        let decision = ctrl.check(0.80, 4096, Priority::Normal, true, true);
        assert_eq!(decision, AdmissionDecision::Reject); // long + normal in selective

        // Just below 0.80 -> aggressive
        let decision = ctrl.check(0.799, 4096, Priority::Normal, true, true);
        assert_eq!(decision, AdmissionDecision::Admit);
    }

    #[test]
    fn test_boundary_selective_eviction() {
        let ctrl = make_controller();
        // At 0.90 -> eviction-required
        let decision = ctrl.check(0.90, 256, Priority::High, true, true);
        assert_eq!(decision, AdmissionDecision::AdmitAfterEviction);

        // Just below 0.90 -> selective (High priority qualifies)
        let decision = ctrl.check(0.899, 256, Priority::High, true, true);
        assert_eq!(decision, AdmissionDecision::Admit);
    }

    #[test]
    fn test_config_update() {
        let mut ctrl = make_controller();
        assert!((ctrl.config().aggressive_threshold - 0.80).abs() < f64::EPSILON);

        let new_config = AdmissionConfig {
            aggressive_threshold: 0.70,
            ..Default::default()
        };
        ctrl.update_config(new_config);
        assert!((ctrl.config().aggressive_threshold - 0.70).abs() < f64::EPSILON);
    }
}
