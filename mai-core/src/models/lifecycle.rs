//! Model lifecycle operations: load, unload, benchmark, export, and affinity.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::registry::{ModelRegistry, ModelStatus, RegistryError};

/// Installed model metadata with lifecycle details.
#[derive(Debug, Clone, Serialize)]
pub struct InstalledModel {
    /// Model identifier.
    pub model_id: String,
    /// Display/model name.
    pub name: String,
    /// Version.
    pub version: String,
    /// Size on disk.
    pub size_bytes: u64,
    /// Required VRAM.
    pub required_vram_bytes: u64,
    /// Preferred backend.
    pub backend: Option<String>,
    /// Current status.
    pub status: String,
    /// Whether the model is loaded in VRAM.
    pub loaded: bool,
    /// Last observed use timestamp, if known.
    pub last_used_epoch: Option<u64>,
    /// Affinity/use count.
    pub use_count: u64,
}

/// Benchmark result for a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    /// Model identifier.
    pub model_id: String,
    /// Tokens generated per second.
    pub tokens_per_sec: f64,
    /// Time to first token in milliseconds.
    pub ttft_ms: u64,
    /// Peak memory used in bytes.
    pub memory_used_bytes: u64,
    /// Benchmark completion timestamp.
    pub completed_at_epoch: u64,
}

/// Shareable TOML deployment config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentExport {
    /// Model identifier.
    pub model_id: String,
    /// Model name.
    pub name: String,
    /// Version.
    pub version: String,
    /// Preferred backend adapter.
    pub backend: String,
    /// Required VRAM.
    pub required_vram_bytes: u64,
    /// Generated TOML text.
    pub toml: String,
}

/// Errors from lifecycle operations.
#[derive(Debug, Error)]
pub enum LifecycleError {
    /// Registry operation failed.
    #[error(transparent)]
    Registry(#[from] RegistryError),
    /// Model has no compatible backend.
    #[error("model {0} has no compatible backend")]
    NoCompatibleBackend(String),
}

/// Tracks lifecycle metadata not currently stored in the registry manifest.
#[derive(Debug, Default)]
pub struct ModelLifecycleManager {
    affinity: HashMap<String, AffinityRecord>,
    benchmarks: HashMap<String, BenchmarkResult>,
}

#[derive(Debug, Clone)]
struct AffinityRecord {
    use_count: u64,
    last_used: Instant,
    last_used_epoch: u64,
}

impl ModelLifecycleManager {
    /// Create a lifecycle manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// List installed models with status and affinity metadata.
    pub fn list_installed(&self, registry: &ModelRegistry) -> Vec<InstalledModel> {
        registry
            .list_models(None)
            .into_iter()
            .map(|summary| {
                let affinity = self.affinity.get(&summary.model_id);
                InstalledModel {
                    model_id: summary.model_id.clone(),
                    name: summary.name,
                    version: summary.version,
                    size_bytes: summary.size_bytes,
                    required_vram_bytes: summary.required_vram_bytes,
                    backend: if summary.capabilities.chat {
                        Some("auto".to_string())
                    } else {
                        None
                    },
                    loaded: matches!(
                        summary.status,
                        ModelStatus::Loaded | ModelStatus::Active { .. }
                    ),
                    status: status_name(&summary.status).to_string(),
                    last_used_epoch: affinity.map(|record| record.last_used_epoch),
                    use_count: affinity.map_or(0, |record| record.use_count),
                }
            })
            .collect()
    }

    /// Load a model via the registry and record affinity.
    pub async fn load_model(
        &mut self,
        registry: &mut ModelRegistry,
        model_id: &str,
        preferred_backend: Option<&str>,
    ) -> Result<(), LifecycleError> {
        let manifest = registry
            .get_model(&model_id.to_string())
            .ok_or_else(|| RegistryError::ModelNotFound(model_id.to_string()))?;
        let backend = preferred_backend
            .map(ToString::to_string)
            .or_else(|| manifest.compatibility.supported_backends.first().cloned())
            .ok_or_else(|| LifecycleError::NoCompatibleBackend(model_id.to_string()))?;
        registry.load_model(&model_id.to_string(), backend).await?;
        self.record_use(model_id);
        Ok(())
    }

    /// Unload a model via the registry.
    pub fn unload_model(
        &mut self,
        registry: &mut ModelRegistry,
        model_id: &str,
    ) -> Result<(), LifecycleError> {
        registry.unload_model(&model_id.to_string())?;
        Ok(())
    }

    /// Run a deterministic synthetic benchmark.
    ///
    /// Real adapter-backed benchmarking lands where live inference engines are
    /// available; this keeps the lifecycle surface testable and stable.
    pub fn benchmark_model(
        &mut self,
        registry: &ModelRegistry,
        model_id: &str,
    ) -> Result<BenchmarkResult, LifecycleError> {
        let manifest = registry
            .get_model(&model_id.to_string())
            .ok_or_else(|| RegistryError::ModelNotFound(model_id.to_string()))?;
        let size_gb = (manifest.model.size_bytes as f64 / 1_000_000_000.0).max(1.0);
        let context_factor = (manifest.capabilities.max_context_tokens as f64 / 4096.0).max(1.0);
        let result = BenchmarkResult {
            model_id: model_id.to_string(),
            tokens_per_sec: (120.0 / size_gb).max(1.0),
            ttft_ms: (80.0 * context_factor).round() as u64,
            memory_used_bytes: manifest.model.required_vram_bytes,
            completed_at_epoch: now_epoch(),
        };
        self.benchmarks.insert(model_id.to_string(), result.clone());
        Ok(result)
    }

    /// Return the last benchmark result.
    pub fn last_benchmark(&self, model_id: &str) -> Option<&BenchmarkResult> {
        self.benchmarks.get(model_id)
    }

    /// Export a deployment TOML for this model.
    pub fn export_config(
        &self,
        registry: &ModelRegistry,
        model_id: &str,
    ) -> Result<DeploymentExport, LifecycleError> {
        let manifest = registry
            .get_model(&model_id.to_string())
            .ok_or_else(|| RegistryError::ModelNotFound(model_id.to_string()))?;
        let backend = manifest
            .compatibility
            .supported_backends
            .first()
            .cloned()
            .unwrap_or_else(|| "auto".to_string());
        let toml = format!(
            "[model]\nid = \"{}\"\nname = \"{}\"\nversion = \"{}\"\nbackend = \"{}\"\nrequired_vram_bytes = {}\n",
            model_id,
            manifest.model.name,
            manifest.model.version,
            backend,
            manifest.model.required_vram_bytes
        );
        Ok(DeploymentExport {
            model_id: model_id.to_string(),
            name: manifest.model.name.clone(),
            version: manifest.model.version.clone(),
            backend,
            required_vram_bytes: manifest.model.required_vram_bytes,
            toml,
        })
    }

    /// Record model use for pre-warming affinity.
    pub fn record_use(&mut self, model_id: &str) {
        let now = Instant::now();
        let now_epoch = now_epoch();
        self.affinity
            .entry(model_id.to_string())
            .and_modify(|record| {
                record.use_count += 1;
                record.last_used = now;
                record.last_used_epoch = now_epoch;
            })
            .or_insert(AffinityRecord {
                use_count: 1,
                last_used: now,
                last_used_epoch: now_epoch,
            });
    }

    /// Return model IDs ordered by highest affinity.
    pub fn affinity_order(&self) -> Vec<String> {
        let mut records = self
            .affinity
            .iter()
            .map(|(id, record)| (id.clone(), record.use_count, record.last_used))
            .collect::<Vec<_>>();
        records.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.cmp(&a.2)));
        records.into_iter().map(|(id, _, _)| id).collect()
    }

    /// Age-out affinity records.
    pub fn prune_affinity_older_than(&mut self, max_age: Duration) {
        self.affinity
            .retain(|_, record| record.last_used.elapsed() <= max_age);
    }
}

fn status_name(status: &ModelStatus) -> &'static str {
    match status {
        ModelStatus::ColdStorage => "cold_storage",
        ModelStatus::Loading { .. } => "loading",
        ModelStatus::Loaded => "loaded",
        ModelStatus::Active { .. } => "active",
        ModelStatus::Evicting => "evicting",
        ModelStatus::Evicted => "evicted",
    }
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ModelRegistry;
    use crate::vault::{VaultError, VaultInterface};
    use std::path::PathBuf;

    struct MockVault;

    #[async_trait::async_trait]
    impl VaultInterface for MockVault {
        async fn load_model_weights(&self, _model_id: &str) -> Result<Vec<u8>, VaultError> {
            Ok(vec![0u8; 10])
        }

        async fn store_model_package(
            &self,
            _model_id: &str,
            _data: &[u8],
        ) -> Result<(), VaultError> {
            Ok(())
        }

        async fn append_audit_entry(&self, _entry: &[u8]) -> Result<(), VaultError> {
            Ok(())
        }

        async fn verify_signature(
            &self,
            _data: &[u8],
            _signature: &[u8],
        ) -> Result<bool, VaultError> {
            Ok(true)
        }
    }

    fn manifest() -> crate::registry::ModelManifest {
        ModelRegistry::parse_manifest(
            r#"
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
"#,
        )
        .unwrap()
    }

    async fn registry() -> ModelRegistry {
        let mut registry = ModelRegistry::new(Box::new(MockVault));
        registry
            .register_cold_model(
                "test-model:1.0.0:Q4_K_M".to_string(),
                manifest(),
                PathBuf::from("/vault/test"),
            )
            .await
            .unwrap();
        registry
    }

    #[tokio::test]
    async fn test_lifecycle_load_benchmark_unload_round_trip() {
        let mut registry = registry().await;
        let mut manager = ModelLifecycleManager::new();
        let model_id = "test-model:1.0.0:Q4_K_M";

        manager
            .load_model(&mut registry, model_id, Some("ollama"))
            .await
            .unwrap();
        assert!(matches!(
            registry.get_status(&model_id.to_string()),
            Some(ModelStatus::Loaded)
        ));

        let benchmark = manager.benchmark_model(&registry, model_id).unwrap();
        assert!(benchmark.tokens_per_sec > 0.0);
        assert!(manager.last_benchmark(model_id).is_some());

        manager.unload_model(&mut registry, model_id).unwrap();
        assert!(matches!(
            registry.get_status(&model_id.to_string()),
            Some(ModelStatus::ColdStorage)
        ));
    }

    #[tokio::test]
    async fn test_list_installed_includes_affinity() {
        let registry = registry().await;
        let mut manager = ModelLifecycleManager::new();
        manager.record_use("test-model:1.0.0:Q4_K_M");
        let installed = manager.list_installed(&registry);
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].use_count, 1);
    }

    #[tokio::test]
    async fn test_export_config_contains_backend() {
        let registry = registry().await;
        let manager = ModelLifecycleManager::new();
        let export = manager
            .export_config(&registry, "test-model:1.0.0:Q4_K_M")
            .unwrap();
        assert!(export.toml.contains("backend = \"ollama\""));
    }

    #[test]
    fn test_affinity_order_prefers_most_used() {
        let mut manager = ModelLifecycleManager::new();
        manager.record_use("a");
        manager.record_use("b");
        manager.record_use("b");
        assert_eq!(manager.affinity_order()[0], "b");
    }
}
