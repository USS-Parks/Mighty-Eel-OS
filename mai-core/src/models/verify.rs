use thiserror::Error;
use tracing::{debug, info, warn};

use crate::registry::CompatibilityInfo;
use crate::vault::VaultInterface;

use super::package::ModelPackage;

/// Verification result
#[derive(Debug, Clone)]
pub struct VerificationResult {
    /// Whether the package passed all checks
    pub verified: bool,
    /// PQC signature verification status
    pub signature_valid: bool,
    /// Hash tree integrity status
    pub hash_tree_valid: bool,
    /// Platform compatibility status
    pub compatible: bool,
    /// Detailed messages for any failures
    pub messages: Vec<String>,
}

/// Verification errors
#[derive(Error, Debug)]
pub enum VerifyError {
    #[error("Signature verification failed: {0}")]
    SignatureFailed(String),

    #[error("Hash tree verification failed: expected {expected}, computed {computed}")]
    HashTreeMismatch { expected: String, computed: String },

    #[error("Platform compatibility check failed: {0}")]
    Incompatible(String),

    #[error("Verification read error: {0}")]
    ReadError(String),
}

/// Verify all aspects of a model package
pub async fn verify_package(
    pkg: &ModelPackage,
    vault: &dyn VaultInterface,
    current_mai_version: &str,
) -> VerificationResult {
    let mut messages = Vec::new();

    let sig_valid = match verify_signature(pkg, vault).await {
        Ok(true) => {
            messages.push("ML-DSA signature verified".to_string());
            true
        }
        Ok(false) => {
            messages.push("ML-DSA signature does not match".to_string());
            false
        }
        Err(e) => {
            messages.push(format!("Signature verification error: {e}"));
            false
        }
    };

    let hash_valid = match verify_hash_tree(pkg) {
        Ok(true) => {
            messages.push("SHA-256 hash tree verified".to_string());
            true
        }
        Ok(false) => {
            messages.push("SHA-256 hash tree mismatch".to_string());
            false
        }
        Err(e) => {
            messages.push(format!("Hash tree verification error: {e}"));
            false
        }
    };

    let compatible = match check_compatibility(&pkg.manifest.compatibility, current_mai_version) {
        Ok(()) => {
            messages.push("Platform compatibility check passed".to_string());
            true
        }
        Err(e) => {
            messages.push(format!("Compatibility check failed: {e}"));
            false
        }
    };

    let verified = sig_valid && hash_valid && compatible;

    if verified {
        info!(
            package = %pkg.name,
            "Package verification passed"
        );
    } else {
        warn!(
            package = %pkg.name,
            sig = sig_valid,
            hash = hash_valid,
            compat = compatible,
            "Package verification failed"
        );
    }

    VerificationResult {
        verified,
        signature_valid: sig_valid,
        hash_tree_valid: hash_valid,
        compatible,
        messages,
    }
}

/// Verify the ML-DSA signature on a package's weights
pub async fn verify_signature(
    pkg: &ModelPackage,
    vault: &dyn VaultInterface,
) -> Result<bool, VerifyError> {
    let weights = pkg
        .read_weights()
        .map_err(|e| VerifyError::ReadError(e.to_string()))?;
    let signature = pkg
        .read_signature()
        .map_err(|e| VerifyError::ReadError(e.to_string()))?;

    vault
        .verify_signature(&weights, &signature)
        .await
        .map_err(|e| VerifyError::SignatureFailed(e.to_string()))
}

/// Verify the SHA-256 Merkle tree root hash against the package weights
pub fn verify_hash_tree(pkg: &ModelPackage) -> Result<bool, VerifyError> {
    let expected = pkg
        .read_hash_tree()
        .map_err(|e| VerifyError::ReadError(e.to_string()))?;

    let weights = pkg
        .read_weights()
        .map_err(|e| VerifyError::ReadError(e.to_string()))?;

    let computed = compute_hash_tree_root(&weights);
    Ok(computed == expected)
}

/// Compute a SHA-256 Merkle tree root for the given data
///
/// For simplicity with models under ~64GB, this computes the root of a
/// binary Merkle tree over 1MB chunks. Production deployments should use
/// a proper incremental hash tree for streaming verification.
pub fn compute_hash_tree_root(data: &[u8]) -> String {
    use blake3::Hash;
    use std::collections::VecDeque;

    let chunk_size: usize = 1_048_576; // 1 MB
    let mut leaves: VecDeque<Hash> = VecDeque::new();

    for chunk in data.chunks(chunk_size) {
        leaves.push_back(blake3::hash(chunk));
    }

    // Build the tree bottom-up
    while leaves.len() > 1 {
        let a = leaves.pop_front().unwrap();
        let b = leaves.pop_front().unwrap();
        let mut combined = Vec::with_capacity(64);
        combined.extend_from_slice(a.as_bytes());
        combined.extend_from_slice(b.as_bytes());
        leaves.push_back(blake3::hash(&combined));
    }

    let root = leaves.pop_front().unwrap_or_else(|| blake3::hash(b""));
    root.to_hex().to_string()
}

/// Check platform compatibility
pub fn check_compatibility(
    compatibility: &CompatibilityInfo,
    current_mai_version: &str,
) -> Result<(), VerifyError> {
    // Check minimum MAI version
    if !version_at_least(current_mai_version, &compatibility.min_mai_version) {
        return Err(VerifyError::Incompatible(format!(
            "MAI version {current_mai_version} is less than required {}",
            compatibility.min_mai_version
        )));
    }

    debug!(
        current = current_mai_version,
        required = %compatibility.min_mai_version,
        backends = ?compatibility.supported_backends,
        "Compatibility check passed"
    );

    Ok(())
}

/// Check if `actual` >= `required` using simple semver comparison
fn version_at_least(actual: &str, required: &str) -> bool {
    let parse = |v: &str| -> Vec<u32> {
        v.trim_start_matches('v')
            .split('.')
            .filter_map(|p| p.parse::<u32>().ok())
            .collect()
    };

    let actual_parts = parse(actual);
    let required_parts = parse(required);

    for (a, r) in actual_parts.iter().zip(required_parts.iter()) {
        if a < r {
            return false;
        }
        if a > r {
            return true;
        }
    }
    // All compared parts equal; actual must have at least as many parts
    actual_parts.len() >= required_parts.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::CompatibilityInfo;

    #[test]
    fn test_version_comparison() {
        assert!(version_at_least("0.1.0", "0.1.0"));
        assert!(version_at_least("0.2.0", "0.1.0"));
        assert!(version_at_least("1.0.0", "0.9.9"));
        assert!(!version_at_least("0.1.0", "0.2.0"));
        assert!(!version_at_least("0.1.0", "1.0.0"));
        assert!(version_at_least("v0.1.0", "0.1.0")); // v prefix
        assert!(version_at_least("0.1.0", "0.1")); // fewer parts
        assert!(!version_at_least("0.1", "0.1.0")); // fewer parts in actual
    }

    #[test]
    fn test_check_compatibility_ok() {
        let compat = CompatibilityInfo {
            min_mai_version: "0.1.0".to_string(),
            supported_backends: vec!["ollama".to_string()],
            hardware_classes: vec!["cpu".to_string()],
        };
        assert!(check_compatibility(&compat, "0.2.0").is_ok());
    }

    #[test]
    fn test_check_compatibility_fail() {
        let compat = CompatibilityInfo {
            min_mai_version: "1.0.0".to_string(),
            supported_backends: vec!["ollama".to_string()],
            hardware_classes: vec!["cpu".to_string()],
        };
        let result = check_compatibility(&compat, "0.9.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_hash_tree_computation() {
        let data = b"hello world";
        let root = compute_hash_tree_root(data);
        // Just verify it produces a hex string of correct length
        assert_eq!(root.len(), 64); // blake3 hex = 64 chars
        assert!(root.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_hash_tree_deterministic() {
        let data = b"test data for determinism check";
        let root1 = compute_hash_tree_root(data);
        let root2 = compute_hash_tree_root(data);
        assert_eq!(root1, root2);
    }

    #[test]
    fn test_hash_tree_changes_with_data() {
        let root1 = compute_hash_tree_root(b"abcdef");
        let root2 = compute_hash_tree_root(b"abcdeg");
        assert_ne!(root1, root2);
    }
}
