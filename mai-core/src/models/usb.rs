use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::{debug, info, warn};

use super::package::{ModelPackage, is_package_dir};

/// Information about a discovered USB drive
#[derive(Debug, Clone)]
pub struct UsbDriveInfo {
    /// Mount point of the USB drive
    pub mount_point: PathBuf,
    /// Volume label (if available)
    pub label: Option<String>,
    /// Available space in bytes
    pub available_bytes: u64,
}

/// Result of USB package discovery
#[derive(Debug, Clone)]
pub struct DiscoveryResult {
    /// Packages found on all scanned USB drives
    pub packages: Vec<ModelPackage>,
    /// USB drives that were scanned
    pub drives_scanned: Vec<UsbDriveInfo>,
    /// Any errors encountered during scanning
    pub errors: Vec<String>,
}

/// Errors from USB operations
#[derive(Error, Debug)]
pub enum UsbError {
    #[error("No USB drives found")]
    NoDrivesFound,

    #[error("Failed to access drive {0}: {1}")]
    DriveAccessError(String, String),

    #[error("Failed to read directory {0}: {1}")]
    ReadDirError(String, String),
}

/// Platform-specific USB mount points to check
fn get_platform_mount_points() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    // Windows: check drive letters D: through Z:
    for letter in 'D'..='Z' {
        candidates.push(PathBuf::from(format!("{letter}:")));
    }

    // Also check common USB paths
    candidates.push(PathBuf::from("D:"));
    candidates.push(PathBuf::from("E:"));

    candidates
}

/// Check if a path appears to be a removable/USB drive on Windows
fn is_removable_drive(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    if path_str.len() == 2 && path_str.ends_with(':') {
        let drive_letter = path_str.chars().next().unwrap();
        // Typical USB drive letters are D:, E:, F:, etc.
        ('D'..='Z').contains(&drive_letter)
    } else {
        false
    }
}

/// Discover all `.mai-pkg` directories on removable USB drives
pub fn discover_usb_packages() -> DiscoveryResult {
    let mut packages = Vec::new();
    let mut drives_scanned = Vec::new();
    let mut errors = Vec::new();

    let mount_points = get_platform_mount_points();

    for mp in &mount_points {
        if !mp.exists() {
            continue;
        }

        if !is_removable_drive(mp) {
            continue;
        }

        debug!(mount_point = %mp.display(), "Scanning USB drive for MAI packages");

        let available_bytes = fs_available_bytes(mp);

        drives_scanned.push(UsbDriveInfo {
            mount_point: mp.clone(),
            label: None,
            available_bytes,
        });

        // Scan for .mai-pkg directories at the root
        match std::fs::read_dir(mp) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    if entry_path.is_dir()
                        && let Some(dir_name) = entry_path.file_name()
                    {
                        let dir_name = dir_name.to_string_lossy();
                        if is_package_dir(&dir_name) {
                            match ModelPackage::open(&entry_path) {
                                Ok(pkg) => {
                                    info!(
                                        package = %pkg.name,
                                        model = %pkg.manifest.model.name,
                                        path = %entry_path.display(),
                                        "Discovered MAI model package on USB"
                                    );
                                    packages.push(pkg);
                                }
                                Err(e) => {
                                    warn!(
                                        path = %entry_path.display(),
                                        error = %e,
                                        "Skipping invalid package directory"
                                    );
                                    errors.push(format!(
                                        "Invalid package at {}: {e}",
                                        entry_path.display()
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                let msg = format!("Failed to read USB drive {}: {e}", mp.display());
                warn!("{msg}");
                errors.push(msg);
            }
        }
    }

    if packages.is_empty() && drives_scanned.is_empty() {
        debug!("No USB drives or MAI packages found");
    } else {
        info!(
            drives = drives_scanned.len(),
            packages = packages.len(),
            "USB package discovery complete"
        );
    }

    DiscoveryResult {
        packages,
        drives_scanned,
        errors,
    }
}

/// Get available bytes on a filesystem
fn fs_available_bytes(path: &Path) -> u64 {
    // Follow-up: Use platform-specific APIs for accurate free space
    // - Windows: GetDiskFreeSpaceExW
    // - Linux/macOS: statvfs
    let _ = path;
    0
}

/// Scan a specific path for `.mai-pkg` directories (non-recursive)
pub fn scan_path_for_packages(path: &Path) -> Vec<ModelPackage> {
    let mut packages = Vec::new();

    if !path.is_dir() {
        return packages;
    }

    match std::fs::read_dir(path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if entry_path.is_dir()
                    && let Some(dir_name) = entry_path.file_name()
                {
                    let dir_name = dir_name.to_string_lossy();
                    if is_package_dir(&dir_name) {
                        match ModelPackage::open(&entry_path) {
                            Ok(pkg) => packages.push(pkg),
                            Err(e) => {
                                warn!(
                                    path = %entry_path.display(),
                                    error = %e,
                                    "Skipping invalid package during scan"
                                );
                            }
                        }
                    }
                }
            }
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Failed to scan path");
        }
    }

    packages
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_test_package(dir: &Path, name: &str) -> PathBuf {
        let pkg_dir = dir.join(name);
        fs::create_dir_all(&pkg_dir).unwrap();

        let manifest = r#"
[model]
name = "test-model"
version = "1.0.0"
format = "GGUF"
quantization = "Q4_K_M"
size_bytes = 1000
required_vram_bytes = 2000

[compatibility]
min_mai_version = "0.1.0"
supported_backends = ["ollama"]
hardware_classes = ["cpu"]

[capabilities]
chat = true
completion = true
embedding = false
vision = false
structured_output = false
max_context_tokens = 4096
supported_languages = ["en"]

[security]
signature_algorithm = "ML-DSA-87"
public_key_fingerprint = "sha256:test"
integrity_hash_tree = "root_hash"

[metadata]
license = "MIT"
changelog = "Initial"
"#;
        fs::write(pkg_dir.join("manifest.toml"), manifest).unwrap();
        fs::write(pkg_dir.join("weights.bin"), vec![0u8; 100]).unwrap();
        fs::write(pkg_dir.join("signature.mldsa"), vec![1u8; 64]).unwrap();
        fs::write(pkg_dir.join("hash_tree.sha256"), "deadbeef\n").unwrap();
        pkg_dir
    }

    #[test]
    fn test_scan_path_for_packages() {
        let dir = std::env::temp_dir().join("test_usb_scan");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        create_test_package(&dir, "model-a.mai-pkg");
        create_test_package(&dir, "model-b.mai-pkg");
        // Non-package directory should be ignored
        fs::create_dir_all(dir.join("regular-dir")).unwrap();

        let packages = scan_path_for_packages(&dir);
        assert_eq!(packages.len(), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_scan_path_for_packages_empty_dir() {
        let dir = std::env::temp_dir().join("test_usb_scan_empty");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let packages = scan_path_for_packages(&dir);
        assert!(packages.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_scan_path_for_packages_nonexistent() {
        let packages = scan_path_for_packages(Path::new("C:\\nonexistent_path_12345"));
        assert!(packages.is_empty());
    }

    #[test]
    fn test_is_removable_drive() {
        assert!(is_removable_drive(Path::new("D:")));
        assert!(is_removable_drive(Path::new("E:")));
        assert!(is_removable_drive(Path::new("Z:")));
        assert!(!is_removable_drive(Path::new("C:")));
        assert!(!is_removable_drive(Path::new("\\\\server\\share")));
    }
}
