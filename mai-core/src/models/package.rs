use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::registry::ModelManifest;

/// Standard file names inside a `.mai-pkg` directory
pub const MANIFEST_FILE: &str = "manifest.toml";
pub const WEIGHTS_FILE: &str = "weights.bin";
pub const SIGNATURE_FILE: &str = "signature.mldsa";
pub const HASH_TREE_FILE: &str = "hash_tree.sha256";
/// Optional ML-DSA signature over the canonical `manifest.toml` bytes. Its
/// presence marks a manifest-authenticated (v2) package. Without it the
/// manifest's identity/permission fields are unauthenticated (finding DF-01A).
pub const MANIFEST_SIG_FILE: &str = "manifest.mldsa";

/// Package suffix expected for model packages on USB/media
pub const PACKAGE_SUFFIX: &str = ".mai-pkg";

/// A discovered model package on disk
#[derive(Debug, Clone)]
pub struct ModelPackage {
    /// Absolute path to the `.mai-pkg` directory
    pub package_dir: PathBuf,
    /// Package name (directory stem, e.g. "qwen3-14b-Q4_K_M")
    pub name: String,
    /// Parsed manifest (lazy-loaded)
    pub manifest: ModelManifest,
    /// File sizes for each component
    pub file_sizes: PackageFileSizes,
}

/// Sizes of files within a package
#[derive(Debug, Clone, Default)]
pub struct PackageFileSizes {
    pub weights_bytes: u64,
    pub signature_bytes: u64,
    pub hash_tree_bytes: u64,
}

/// Errors from package operations
#[derive(Error, Debug)]
pub enum PackageError {
    #[error("Package not found at {0}")]
    NotFound(PathBuf),

    #[error("Missing required file in package: {0}")]
    MissingFile(String),

    #[error("Failed to read file {0}: {1}")]
    ReadError(String, String),

    #[error("Manifest parsing failed: {0}")]
    ManifestError(String),

    #[error("Package already exists at {0}")]
    AlreadyExists(PathBuf),

    #[error("IO error: {0}")]
    Io(String),
}

impl ModelPackage {
    /// Open a `.mai-pkg` directory and parse its manifest
    pub fn open(package_dir: &Path) -> Result<Self, PackageError> {
        if !package_dir.is_dir() {
            return Err(PackageError::NotFound(package_dir.to_path_buf()));
        }

        let name = package_dir
            .file_stem()
            .ok_or_else(|| PackageError::Io("package path has no file name".to_string()))?
            .to_string_lossy()
            .to_string();

        let manifest_path = package_dir.join(MANIFEST_FILE);
        let weights_path = package_dir.join(WEIGHTS_FILE);
        let sig_path = package_dir.join(SIGNATURE_FILE);
        let hash_path = package_dir.join(HASH_TREE_FILE);

        // Verify all required files exist
        for (label, path) in [
            ("manifest.toml", &manifest_path),
            ("weights.bin", &weights_path),
            ("signature.mldsa", &sig_path),
            ("hash_tree.sha256", &hash_path),
        ] {
            if !path.is_file() {
                return Err(PackageError::MissingFile(format!(
                    "{label} not found at {}",
                    path.display()
                )));
            }
        }

        let manifest_content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| PackageError::ReadError(MANIFEST_FILE.to_string(), e.to_string()))?;

        let manifest: ModelManifest = toml::from_str(&manifest_content)
            .map_err(|e| PackageError::ManifestError(e.to_string()))?;

        let weights_bytes = std::fs::metadata(&weights_path)
            .map(|m| m.len())
            .unwrap_or(0);
        let signature_bytes = std::fs::metadata(&sig_path).map(|m| m.len()).unwrap_or(0);
        let hash_tree_bytes = std::fs::metadata(&hash_path).map(|m| m.len()).unwrap_or(0);

        Ok(Self {
            package_dir: package_dir.to_path_buf(),
            name,
            manifest,
            file_sizes: PackageFileSizes {
                weights_bytes,
                signature_bytes,
                hash_tree_bytes,
            },
        })
    }

    /// Read the weights file into memory
    pub fn read_weights(&self) -> Result<Vec<u8>, PackageError> {
        let path = self.package_dir.join(WEIGHTS_FILE);
        std::fs::read(&path)
            .map_err(|e| PackageError::ReadError(WEIGHTS_FILE.to_string(), e.to_string()))
    }

    /// Read the ML-DSA signature file
    pub fn read_signature(&self) -> Result<Vec<u8>, PackageError> {
        let path = self.package_dir.join(SIGNATURE_FILE);
        std::fs::read(&path)
            .map_err(|e| PackageError::ReadError(SIGNATURE_FILE.to_string(), e.to_string()))
    }

    /// Read the hash tree file (hex-encoded SHA-256 root)
    pub fn read_hash_tree(&self) -> Result<String, PackageError> {
        let path = self.package_dir.join(HASH_TREE_FILE);
        std::fs::read_to_string(&path)
            .map(|s| s.trim().to_string())
            .map_err(|e| PackageError::ReadError(HASH_TREE_FILE.to_string(), e.to_string()))
    }

    /// Read the raw `manifest.toml` bytes — the exact signed preimage for
    /// manifest authentication (finding DF-01A). Reads the on-disk bytes rather
    /// than re-serializing the parsed struct so verification is byte-exact.
    pub fn read_manifest_bytes(&self) -> Result<Vec<u8>, PackageError> {
        let path = self.package_dir.join(MANIFEST_FILE);
        std::fs::read(&path)
            .map_err(|e| PackageError::ReadError(MANIFEST_FILE.to_string(), e.to_string()))
    }

    /// Read the optional manifest signature. `Ok(None)` means the package
    /// carries no `manifest.mldsa` (a legacy, manifest-unauthenticated package).
    pub fn read_manifest_signature(&self) -> Result<Option<Vec<u8>>, PackageError> {
        let path = self.package_dir.join(MANIFEST_SIG_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(PackageError::ReadError(
                MANIFEST_SIG_FILE.to_string(),
                e.to_string(),
            )),
        }
    }

    /// Compute the model ID string from manifest
    pub fn model_id(&self) -> String {
        format!(
            "{}:{}:{}",
            self.manifest.model.name,
            self.manifest.model.version,
            self.manifest
                .model
                .quantization
                .as_deref()
                .unwrap_or("native")
        )
    }
}

/// Determine if a directory name indicates a MAI model package
pub fn is_package_dir(dir_name: &str) -> bool {
    dir_name.ends_with(PACKAGE_SUFFIX)
}

/// Format version for package compatibility
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PackageVersion {
    V1,
}

impl PackageVersion {
    pub const CURRENT: PackageVersion = PackageVersion::V1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_test_package(dir: &Path) -> PathBuf {
        let pkg_dir = dir.join("test-model.mai-pkg");
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
    fn test_open_valid_package() {
        let dir = std::env::temp_dir().join("test_pkg_open");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let pkg_dir = create_test_package(&dir);
        let pkg = ModelPackage::open(&pkg_dir).unwrap();
        assert_eq!(pkg.name, "test-model");
        assert_eq!(pkg.manifest.model.name, "test-model");
        assert_eq!(pkg.model_id(), "test-model:1.0.0:Q4_K_M");
        assert_eq!(pkg.file_sizes.weights_bytes, 100);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_open_missing_directory() {
        let bad_path = Path::new("C:\\nonexistent\\pkg.mai-pkg");
        let result = ModelPackage::open(bad_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_open_missing_weight_file() {
        let dir = std::env::temp_dir().join("test_pkg_missing_weight");
        let _ = fs::remove_dir_all(&dir);
        let pkg_dir = dir.join("bad.mai-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("manifest.toml"), "[model]\nname=\"x\"\nversion=\"1\"\nformat=\"GGUF\"\nsize_bytes=1\nrequired_vram_bytes=1\n[compatibility]\nmin_mai_version=\"0.1\"\nsupported_backends=[\"ollama\"]\nhardware_classes=[\"cpu\"]\n[capabilities]\nchat=true\ncompletion=true\nembedding=false\nvision=false\nstructured_output=false\nmax_context_tokens=4096\nsupported_languages=[\"en\"]\n[security]\nsignature_algorithm=\"ML-DSA-87\"\npublic_key_fingerprint=\"test\"\nintegrity_hash_tree=\"root\"\n[metadata]\nlicense=\"MIT\"\nchangelog=\"x\"\n").unwrap();
        // Intentionally skip weights.bin

        let result = ModelPackage::open(&pkg_dir);
        assert!(result.is_err());
        assert!(matches!(result, Err(PackageError::MissingFile(_))));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_is_package_dir() {
        assert!(is_package_dir("qwen3.mai-pkg"));
        assert!(!is_package_dir("qwen3"));
        assert!(!is_package_dir("qwen3.mai-pkg.bak"));
    }

    #[test]
    fn test_read_files() {
        let dir = std::env::temp_dir().join("test_pkg_read");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let pkg_dir = create_test_package(&dir);
        let pkg = ModelPackage::open(&pkg_dir).unwrap();

        let weights = pkg.read_weights().unwrap();
        assert_eq!(weights.len(), 100);

        let sig = pkg.read_signature().unwrap();
        assert_eq!(sig.len(), 64);

        let hash = pkg.read_hash_tree().unwrap();
        assert_eq!(hash, "deadbeef");

        let _ = fs::remove_dir_all(&dir);
    }
}
