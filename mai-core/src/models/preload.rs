//! First-boot and affinity-based model pre-loading plans.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::lifecycle::ModelLifecycleManager;
use crate::registry::{ModelRegistry, ModelStatus};

/// Preload configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreloadConfig {
    /// Sentinel model loaded before all user-facing models.
    pub sentinel_model_id: String,
    /// User preferred default model.
    pub preferred_model_id: Option<String>,
    /// Maximum number of models to load in the background.
    pub max_background_models: usize,
}

impl Default for PreloadConfig {
    fn default() -> Self {
        Self {
            sentinel_model_id: "sentinel:default:native".to_string(),
            preferred_model_id: None,
            max_background_models: 2,
        }
    }
}

/// A model selected for boot preloading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreloadTarget {
    /// Model identifier.
    pub model_id: String,
    /// Why it was selected.
    pub reason: PreloadReason,
    /// Load order.
    pub order: usize,
}

/// Reason a model is preloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreloadReason {
    /// Sentinel model always goes first.
    Sentinel,
    /// User configured model.
    Preferred,
    /// High-affinity frequently used model.
    Affinity,
}

/// Build a non-blocking preload plan.
pub fn build_preload_plan(
    registry: &ModelRegistry,
    lifecycle: &ModelLifecycleManager,
    config: &PreloadConfig,
) -> Vec<PreloadTarget> {
    let installed = registry.list_models(None);
    let installed_ids = installed
        .iter()
        .map(|summary| summary.model_id.as_str())
        .collect::<HashSet<_>>();
    let already_loaded = installed
        .iter()
        .filter(|summary| {
            matches!(
                summary.status,
                ModelStatus::Loaded | ModelStatus::Active { .. }
            )
        })
        .map(|summary| summary.model_id.as_str())
        .collect::<HashSet<_>>();

    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    push_if_available(
        &mut targets,
        &mut seen,
        &installed_ids,
        &already_loaded,
        &config.sentinel_model_id,
        PreloadReason::Sentinel,
    );

    if let Some(preferred) = &config.preferred_model_id {
        push_if_available(
            &mut targets,
            &mut seen,
            &installed_ids,
            &already_loaded,
            preferred,
            PreloadReason::Preferred,
        );
    }

    for model_id in lifecycle.affinity_order() {
        if targets.len() >= config.max_background_models.saturating_add(1) {
            break;
        }
        push_if_available(
            &mut targets,
            &mut seen,
            &installed_ids,
            &already_loaded,
            &model_id,
            PreloadReason::Affinity,
        );
    }

    for (index, target) in targets.iter_mut().enumerate() {
        target.order = index;
    }
    targets
}

fn push_if_available(
    targets: &mut Vec<PreloadTarget>,
    seen: &mut HashSet<String>,
    installed_ids: &HashSet<&str>,
    already_loaded: &HashSet<&str>,
    model_id: &str,
    reason: PreloadReason,
) {
    if installed_ids.contains(model_id)
        && !already_loaded.contains(model_id)
        && seen.insert(model_id.to_string())
    {
        targets.push(PreloadTarget {
            model_id: model_id.to_string(),
            reason,
            order: targets.len(),
        });
    }
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
            Ok(vec![])
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

    fn manifest(name: &str) -> crate::registry::ModelManifest {
        ModelRegistry::parse_manifest(&format!(
            r#"
[model]
name = "{name}"
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
"#
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn test_preload_plan_sentinel_then_preferred_then_affinity() {
        let mut registry = ModelRegistry::new(Box::new(MockVault));
        for id in [
            "sentinel:1.0.0:Q4_K_M",
            "preferred:1.0.0:Q4_K_M",
            "affinity:1.0.0:Q4_K_M",
        ] {
            let name = id.split(':').next().unwrap();
            registry
                .register_cold_model(id.to_string(), manifest(name), PathBuf::from("/vault"))
                .await
                .unwrap();
        }

        let mut lifecycle = ModelLifecycleManager::new();
        lifecycle.record_use("affinity:1.0.0:Q4_K_M");
        let config = PreloadConfig {
            sentinel_model_id: "sentinel:1.0.0:Q4_K_M".to_string(),
            preferred_model_id: Some("preferred:1.0.0:Q4_K_M".to_string()),
            max_background_models: 3,
        };
        let plan = build_preload_plan(&registry, &lifecycle, &config);
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].reason, PreloadReason::Sentinel);
        assert_eq!(plan[1].reason, PreloadReason::Preferred);
        assert_eq!(plan[2].reason, PreloadReason::Affinity);
    }
}
