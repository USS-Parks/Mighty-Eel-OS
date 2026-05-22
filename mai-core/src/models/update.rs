//! OTA update client and tier/license policy for model packages.
//!
//! The client is intentionally transport-agnostic. Production code can plug in
//! an HTTPS transport, tests can use an in-memory transport, and air-gapped
//! systems can leave updates disabled without pulling network behavior into
//! the core registry path.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncWriteExt;

use crate::registry::ModelSummary;

/// Product/update tiers used for entitlement checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateTier {
    /// Small, single-GPU packages.
    Scout,
    /// Larger multi-GPU packages.
    Ranger,
    /// Complete library.
    PackLeader,
}

impl UpdateTier {
    /// Parse a tier name accepted by the public update API.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "scout" => Some(Self::Scout),
            "ranger" => Some(Self::Ranger),
            "pack_leader" | "pack-leader" => Some(Self::PackLeader),
            _ => None,
        }
    }

    /// Public wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scout => "scout",
            Self::Ranger => "ranger",
            Self::PackLeader => "pack_leader",
        }
    }

    fn includes(self, requested: Self) -> bool {
        self >= requested
    }
}

/// Manifest returned by `/v1/updates/manifest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateManifest {
    /// Models available to the requested tier.
    pub models: Vec<UpdateModel>,
    /// Optional seasonal release label, e.g. `2026-summer`.
    #[serde(default)]
    pub season: Option<String>,
}

/// A single model entry in the update manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateModel {
    /// Model/package name.
    pub name: String,
    /// Semantic version.
    pub version: String,
    /// Total package size in bytes.
    pub size: u64,
    /// Package manifest or archive URL.
    pub url: String,
    /// Minimum entitlement tier required to download this model.
    #[serde(default = "default_tier")]
    pub tier: UpdateTier,
    /// Optional changed shards advertised by the server.
    #[serde(default)]
    pub shards: Vec<WeightShard>,
}

fn default_tier() -> UpdateTier {
    UpdateTier::Scout
}

/// A single weight shard in a package manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WeightShard {
    /// Relative shard name/path.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Integrity hash advertised by the update server.
    pub hash: String,
    /// Direct download URL for this shard.
    pub url: String,
}

/// License document embedded in an update package or supplied by admin config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseEntitlement {
    /// License key or package-embedded license identifier.
    pub license_key: String,
    /// Maximum tier this license can access.
    pub tier: UpdateTier,
    /// Expiration as unix epoch seconds. Zero means package-embedded/offline
    /// perpetual validation.
    pub expires_at_epoch: u64,
}

/// Result of comparing installed models with an update manifest.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheckResult {
    /// Models with newer versions available.
    pub available: Vec<UpdateModel>,
    /// Models already current or not entitled for the requested tier.
    pub current: Vec<String>,
}

/// Differential download plan.
#[derive(Debug, Clone, Serialize)]
pub struct DifferentialPlan {
    /// Model this plan updates.
    pub model: String,
    /// Version being downloaded.
    pub version: String,
    /// Shards that must be fetched.
    pub shards_to_download: Vec<WeightShard>,
    /// Bytes saved by reusing unchanged local shards.
    pub reused_bytes: u64,
}

/// Minimal request used by update transports.
#[derive(Debug, Clone)]
pub struct UpdateRequest {
    /// Absolute HTTPS URL.
    pub url: String,
    /// Optional HTTP `Range` start byte for resumable downloads.
    pub range_start: Option<u64>,
    /// Optional license key for package downloads.
    pub license_key: Option<String>,
}

/// Minimal response returned by update transports.
#[derive(Debug, Clone)]
pub struct UpdateResponse {
    /// HTTP status code.
    pub status: u16,
    /// Body bytes.
    pub body: Vec<u8>,
    /// Whether server indicated range support.
    pub accepts_ranges: bool,
}

/// Transport boundary for HTTPS GETs.
#[async_trait]
pub trait UpdateTransport: Send + Sync {
    /// Execute a GET request.
    async fn get(&self, request: UpdateRequest) -> Result<UpdateResponse, UpdateError>;
}

/// Client configuration.
#[derive(Debug, Clone)]
pub struct UpdateClientConfig {
    /// Base update server URL.
    pub base_url: String,
    /// Device/product tier.
    pub tier: UpdateTier,
    /// Current MAI software version.
    pub current_version: String,
    /// Optional license key for paid package downloads.
    pub license_key: Option<String>,
    /// Whether update packages should be installed automatically after verify.
    pub auto_install: bool,
    /// Maximum bytes per second for background downloads.
    pub bandwidth_limit_bytes_per_sec: Option<u64>,
}

impl Default for UpdateClientConfig {
    fn default() -> Self {
        Self {
            base_url: "https://updates.islandmountain.ai".to_string(),
            tier: UpdateTier::Scout,
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            license_key: None,
            auto_install: false,
            bandwidth_limit_bytes_per_sec: None,
        }
    }
}

/// Errors from update operations.
#[derive(Debug, Error)]
pub enum UpdateError {
    /// Transport failed.
    #[error("transport error: {0}")]
    Transport(String),
    /// Server returned a non-success status.
    #[error("update server returned status {0}")]
    HttpStatus(u16),
    /// Manifest JSON was invalid.
    #[error("invalid update manifest: {0}")]
    InvalidManifest(String),
    /// License does not entitle requested tier/package.
    #[error("license does not allow tier {requested:?}; license tier is {licensed:?}")]
    TierNotAllowed {
        /// Requested tier.
        requested: UpdateTier,
        /// Licensed tier.
        licensed: UpdateTier,
    },
    /// License expired.
    #[error("license expired")]
    LicenseExpired,
    /// Filesystem operation failed.
    #[error("download I/O failed: {0}")]
    Io(String),
}

/// OTA update client.
pub struct UpdateClient<T: UpdateTransport> {
    config: UpdateClientConfig,
    transport: Arc<T>,
}

impl<T: UpdateTransport> UpdateClient<T> {
    /// Create a client.
    pub fn new(config: UpdateClientConfig, transport: Arc<T>) -> Self {
        Self { config, transport }
    }

    /// Fetch and parse the tier manifest.
    pub async fn fetch_manifest(&self) -> Result<UpdateManifest, UpdateError> {
        let url = format!(
            "{}/v1/updates/manifest?tier={}&version={}",
            self.config.base_url.trim_end_matches('/'),
            self.config.tier.as_str(),
            self.config.current_version
        );
        let response = self
            .transport
            .get(UpdateRequest {
                url,
                range_start: None,
                license_key: None,
            })
            .await?;
        if !(200..300).contains(&response.status) {
            return Err(UpdateError::HttpStatus(response.status));
        }
        serde_json::from_slice(&response.body)
            .map_err(|e| UpdateError::InvalidManifest(e.to_string()))
    }

    /// Check for updates without sending device-identifying information.
    pub async fn check_updates(
        &self,
        installed: &[ModelSummary],
    ) -> Result<UpdateCheckResult, UpdateError> {
        let manifest = self.fetch_manifest().await?;
        Ok(compare_manifest(installed, &manifest, self.config.tier))
    }

    /// Download a shard with resume support.
    pub async fn download_shard_resumable(
        &self,
        shard: &WeightShard,
        target_path: &Path,
    ) -> Result<u64, UpdateError> {
        let existing = match tokio::fs::metadata(target_path).await {
            Ok(meta) => meta.len(),
            Err(_) => 0,
        };
        let response = self
            .transport
            .get(UpdateRequest {
                url: shard.url.clone(),
                range_start: (existing > 0).then_some(existing),
                license_key: self.config.license_key.clone(),
            })
            .await?;
        if !(200..300).contains(&response.status) {
            return Err(UpdateError::HttpStatus(response.status));
        }

        let mut options = tokio::fs::OpenOptions::new();
        options.create(true).write(true);
        if response.status == 206 && response.accepts_ranges {
            options.append(true);
        } else {
            options.truncate(true);
        }
        let mut file = options
            .open(target_path)
            .await
            .map_err(|e| UpdateError::Io(e.to_string()))?;
        file.write_all(&response.body)
            .await
            .map_err(|e| UpdateError::Io(e.to_string()))?;
        file.flush()
            .await
            .map_err(|e| UpdateError::Io(e.to_string()))?;

        tokio::fs::metadata(target_path)
            .await
            .map(|m| m.len())
            .map_err(|e| UpdateError::Io(e.to_string()))
    }
}

/// Compare installed versions against an update manifest.
pub fn compare_manifest(
    installed: &[ModelSummary],
    manifest: &UpdateManifest,
    tier: UpdateTier,
) -> UpdateCheckResult {
    let installed_versions: HashMap<&str, &str> = installed
        .iter()
        .map(|summary| (summary.name.as_str(), summary.version.as_str()))
        .collect();

    let mut available = Vec::new();
    let mut current = Vec::new();
    for model in &manifest.models {
        if !tier.includes(model.tier) {
            current.push(model.name.clone());
            continue;
        }
        match installed_versions.get(model.name.as_str()) {
            Some(version) if !is_newer_version(&model.version, version) => {
                current.push(model.name.clone());
            }
            _ => available.push(model.clone()),
        }
    }

    UpdateCheckResult { available, current }
}

/// Build a differential plan from server shards and locally-known shard hashes.
pub fn plan_differential_download(
    model: &UpdateModel,
    local_hashes: &HashMap<String, String>,
) -> DifferentialPlan {
    let mut shards_to_download = Vec::new();
    let mut reused_bytes = 0;
    for shard in &model.shards {
        match local_hashes.get(&shard.name) {
            Some(hash) if hash == &shard.hash => reused_bytes += shard.size,
            _ => shards_to_download.push(shard.clone()),
        }
    }
    DifferentialPlan {
        model: model.name.clone(),
        version: model.version.clone(),
        shards_to_download,
        reused_bytes,
    }
}

/// Validate a license for a requested package tier.
pub fn validate_license(
    entitlement: &LicenseEntitlement,
    requested_tier: UpdateTier,
    now_epoch: u64,
) -> Result<(), UpdateError> {
    if entitlement.expires_at_epoch != 0 && entitlement.expires_at_epoch < now_epoch {
        return Err(UpdateError::LicenseExpired);
    }
    if !entitlement.tier.includes(requested_tier) {
        return Err(UpdateError::TierNotAllowed {
            requested: requested_tier,
            licensed: entitlement.tier,
        });
    }
    Ok(())
}

/// Return the first models to include in a seasonal bundle for each tier.
pub fn seasonal_bundle(models: &[UpdateModel], tier: UpdateTier) -> Vec<UpdateModel> {
    let limit = match tier {
        UpdateTier::Scout => 3,
        UpdateTier::Ranger => 7,
        UpdateTier::PackLeader => usize::MAX,
    };
    models
        .iter()
        .filter(|model| tier.includes(model.tier))
        .take(limit)
        .cloned()
        .collect()
}

/// Determine whether a package has all required shards locally.
pub fn package_complete(shards: &[WeightShard], present_names: &HashSet<String>) -> bool {
    shards
        .iter()
        .all(|shard| present_names.contains(&shard.name))
}

fn is_newer_version(candidate: &str, installed: &str) -> bool {
    let parse = |value: &str| {
        value
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    let candidate_parts = parse(candidate);
    let installed_parts = parse(installed);
    let max_len = candidate_parts.len().max(installed_parts.len());
    for index in 0..max_len {
        let left = candidate_parts.get(index).copied().unwrap_or(0);
        let right = installed_parts.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

/// Helper for constructing a temp download path from model/version.
pub fn download_dir(base: &Path, model: &str, version: &str) -> PathBuf {
    base.join(format!("{model}-{version}.download"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{CapabilityInfo, ModelStatus};
    use std::sync::Mutex;

    struct MockTransport {
        responses: Mutex<Vec<UpdateResponse>>,
        requests: Mutex<Vec<UpdateRequest>>,
    }

    #[async_trait]
    impl UpdateTransport for MockTransport {
        async fn get(&self, request: UpdateRequest) -> Result<UpdateResponse, UpdateError> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| UpdateError::Transport("no response".to_string()))
        }
    }

    fn installed(version: &str) -> ModelSummary {
        ModelSummary {
            model_id: "qwen:old:Q4".to_string(),
            name: "qwen".to_string(),
            version: version.to_string(),
            status: ModelStatus::ColdStorage,
            size_bytes: 10,
            required_vram_bytes: 10,
            capabilities: CapabilityInfo {
                chat: true,
                completion: true,
                embedding: false,
                vision: false,
                structured_output: false,
                max_context_tokens: 4096,
                supported_languages: vec!["en".to_string()],
            },
        }
    }

    #[tokio::test]
    async fn test_update_check_no_device_identity_in_url() {
        let body = br#"{"models":[{"name":"qwen","version":"1.1.0","size":100,"url":"https://mirror/pkg","tier":"scout"}]}"#;
        let transport = Arc::new(MockTransport {
            responses: Mutex::new(vec![UpdateResponse {
                status: 200,
                body: body.to_vec(),
                accepts_ranges: false,
            }]),
            requests: Mutex::new(Vec::new()),
        });
        let client = UpdateClient::new(UpdateClientConfig::default(), Arc::clone(&transport));
        let result = client.check_updates(&[installed("1.0.0")]).await.unwrap();
        assert_eq!(result.available.len(), 1);
        let requests = transport.requests.lock().unwrap();
        assert!(requests[0].url.contains("tier=scout"));
        assert!(!requests[0].url.contains("device"));
        assert!(!requests[0].url.contains("profile"));
    }

    #[test]
    fn test_differential_download_only_changed_shards() {
        let model = UpdateModel {
            name: "qwen".to_string(),
            version: "1.1.0".to_string(),
            size: 30,
            url: "https://mirror/pkg".to_string(),
            tier: UpdateTier::Scout,
            shards: vec![
                WeightShard {
                    name: "a".to_string(),
                    size: 10,
                    hash: "same".to_string(),
                    url: "https://mirror/a".to_string(),
                },
                WeightShard {
                    name: "b".to_string(),
                    size: 20,
                    hash: "changed".to_string(),
                    url: "https://mirror/b".to_string(),
                },
            ],
        };
        let local = HashMap::from([("a".to_string(), "same".to_string())]);
        let plan = plan_differential_download(&model, &local);
        assert_eq!(plan.shards_to_download.len(), 1);
        assert_eq!(plan.shards_to_download[0].name, "b");
        assert_eq!(plan.reused_bytes, 10);
    }

    #[tokio::test]
    async fn test_resumable_download_uses_range() {
        let dir = std::env::temp_dir().join("mai_update_resume_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("shard.bin");
        std::fs::write(&target, b"abc").unwrap();

        let transport = Arc::new(MockTransport {
            responses: Mutex::new(vec![UpdateResponse {
                status: 206,
                body: b"def".to_vec(),
                accepts_ranges: true,
            }]),
            requests: Mutex::new(Vec::new()),
        });
        let client = UpdateClient::new(UpdateClientConfig::default(), Arc::clone(&transport));
        let size = client
            .download_shard_resumable(
                &WeightShard {
                    name: "shard.bin".to_string(),
                    size: 6,
                    hash: "hash".to_string(),
                    url: "https://mirror/shard.bin".to_string(),
                },
                &target,
            )
            .await
            .unwrap();
        assert_eq!(size, 6);
        assert_eq!(std::fs::read(&target).unwrap(), b"abcdef");
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests[0].range_start, Some(3));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_license_validation_blocks_wrong_tier() {
        let entitlement = LicenseEntitlement {
            license_key: "test".to_string(),
            tier: UpdateTier::Scout,
            expires_at_epoch: 0,
        };
        let result = validate_license(&entitlement, UpdateTier::Ranger, 10);
        assert!(matches!(result, Err(UpdateError::TierNotAllowed { .. })));
    }

    #[test]
    fn test_seasonal_bundle_limits_by_tier() {
        let models = (0..8)
            .map(|i| UpdateModel {
                name: format!("m{i}"),
                version: "1.0.0".to_string(),
                size: 1,
                url: "https://mirror/pkg".to_string(),
                tier: UpdateTier::Scout,
                shards: Vec::new(),
            })
            .collect::<Vec<_>>();
        assert_eq!(seasonal_bundle(&models, UpdateTier::Scout).len(), 3);
        assert_eq!(seasonal_bundle(&models, UpdateTier::Ranger).len(), 7);
        assert_eq!(seasonal_bundle(&models, UpdateTier::PackLeader).len(), 8);
    }
}
