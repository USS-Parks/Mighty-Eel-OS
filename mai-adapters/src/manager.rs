//! AdapterManager - Top-level orchestrator for all adapter processes.
//!
//! Discovers, loads, monitors, and manages adapter processes.
//! Provides the interface that the mai-core scheduler calls to route
//! inference requests to adapters.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

use mai_hil::traits::{
    AdapterCapabilities, AdapterConfig, Embedding, GenerationParams,
    GenerationResult, HealthStatus, Token,
};

use crate::audit::{AuditBuffer, AuditTimer};
use crate::bridge::{
    CapabilitiesResult, EmbedParams, EmbedResult, GenerateBatchParams, GenerateBatchResult,
    GenerateParams, HealthCheckResult, InitializeParams, InitializeResult,
};
use crate::config::{DiscoveredAdapter, FrameworkConfig};
use crate::errors::FrameworkError;
use crate::health::{AdapterHealthState, HealthCheckResult as HealthCheckOutcome, HealthReport};
use crate::process::{AdapterProcess, ProcessState};

/// Handle to a managed adapter returned after initialization.
#[derive(Debug, Clone)]
pub struct ManagedAdapter {
    /// Adapter name.
    pub name: String,
    /// Adapter version.
    pub version: String,
    /// Static capabilities reported by the adapter.
    pub capabilities: AdapterCapabilities,
    /// Opaque handle string from initialization.
    pub handle: String,
}

/// The top-level adapter framework orchestrator.
///
/// Runs in trusted Rust code. Spawns and manages untrusted Python
/// adapter subprocesses with cgroups isolation and crash recovery.
pub struct AdapterManager {
    /// Framework configuration.
    config: Arc<FrameworkConfig>,
    /// Map of adapter name to its process manager.
    processes: Arc<RwLock<HashMap<String, Mutex<AdapterProcess>>>>,
    /// Health state per adapter.
    health: Arc<RwLock<HashMap<String, Mutex<AdapterHealthState>>>>,
    /// Audit log buffer.
    audit: Arc<Mutex<AuditBuffer>>,
    /// Cached capabilities per adapter.
    capabilities: Arc<RwLock<HashMap<String, AdapterCapabilities>>>,
    /// Next stream ID for streaming requests.
    next_stream_id: Arc<std::sync::atomic::AtomicU64>,
}

impl AdapterManager {
    /// Create a new adapter manager with the given configuration.
    pub fn new(config: FrameworkConfig) -> Self {
        let config = Arc::new(config);
        Self {
            config,
            processes: Arc::new(RwLock::new(HashMap::new())),
            health: Arc::new(RwLock::new(HashMap::new())),
            audit: Arc::new(Mutex::new(AuditBuffer::new(10_000))),
            capabilities: Arc::new(RwLock::new(HashMap::new())),
            next_stream_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        }
    }

    /// Discover and register all adapters from the adapters directory.
    pub async fn discover(&self) -> Result<Vec<DiscoveredAdapter>, FrameworkError> {
        let discovered = self.config.discover_adapters()?;
        info!(count = discovered.len(), "Discovered adapters");

        let mut processes = self.processes.write().await;
        for adapter in &discovered {
            let process = AdapterProcess::new(adapter.clone(), Arc::clone(&self.config));
            processes.insert(adapter.name.clone(), Mutex::new(process));
        }

        Ok(discovered)
    }

    /// Start a specific adapter by name.
    pub async fn start_adapter(
        &self,
        name: &str,
        adapter_config: AdapterConfig,
    ) -> Result<ManagedAdapter, FrameworkError> {
        let processes = self.processes.read().await;
        let process_mutex = processes
            .get(name)
            .ok_or_else(|| FrameworkError::AdapterNotFound {
                name: name.to_string(),
            })?;

        let mut process = process_mutex.lock().await;

        // Spawn the subprocess
        process.spawn().await?;

        // Send initialize RPC
        let init_params = InitializeParams {
            config: adapter_config,
        };
        let params_json = serde_json::to_value(&init_params)?;

        let timer = AuditTimer::start(name.to_string(), "initialize".to_string(), 0);
        let result = process.call("initialize", params_json).await;

        match result {
            Ok(value) => {
                timer.success();
                let init_result: InitializeResult = serde_json::from_value(value)
                    .map_err(|e| FrameworkError::ProtocolError {
                        name: name.to_string(),
                        detail: format!("Invalid initialize response: {e}"),
                    })?;

                process.mark_running();

                // Fetch capabilities
                let caps_value = process.call("capabilities", Value::Null).await?;
                let caps_result: CapabilitiesResult = serde_json::from_value(caps_value)
                    .map_err(|e| FrameworkError::ProtocolError {
                        name: name.to_string(),
                        detail: format!("Invalid capabilities response: {e}"),
                    })?;

                // Cache capabilities
                {
                    let mut caps = self.capabilities.write().await;
                    caps.insert(name.to_string(), caps_result.capabilities.clone());
                }

                // Set up health monitoring
                {
                    let mut health = self.health.write().await;
                    health.insert(
                        name.to_string(),
                        Mutex::new(AdapterHealthState::new(
                            name.to_string(),
                            self.config.heartbeat_interval_ms,
                            self.config.missed_heartbeat_threshold,
                        )),
                    );
                }

                info!(adapter = %name, handle = %init_result.handle, "Adapter started successfully");

                Ok(ManagedAdapter {
                    name: name.to_string(),
                    version: process.info.version.clone(),
                    capabilities: caps_result.capabilities,
                    handle: init_result.handle,
                })
            }
            Err(e) => {
                timer.failure(format!("{e}"));
                process.kill().await;
                Err(FrameworkError::InitFailed {
                    name: name.to_string(),
                    reason: e.to_string(),
                })
            }
        }
    }

    /// Send a generate (streaming) request to an adapter.
    /// Returns a stream of Token results.
    pub async fn generate(
        &self,
        adapter_name: &str,
        prompt: String,
        params: GenerationParams,
    ) -> Result<Vec<Token>, FrameworkError> {
        let processes = self.processes.read().await;
        let process_mutex = processes
            .get(adapter_name)
            .ok_or_else(|| FrameworkError::AdapterNotFound {
                name: adapter_name.to_string(),
            })?;

        let stream_id = self
            .next_stream_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let gen_params = GenerateParams {
            prompt,
            params,
            stream_id,
        };
        let params_json = serde_json::to_value(&gen_params)?;

        let timer = AuditTimer::start(adapter_name.to_string(), "generate".to_string(), stream_id);

        let mut process = process_mutex.lock().await;
        let result = process.call("generate", params_json).await;

        match result {
            Ok(value) => {
                timer.success();
                // For non-streaming mode, the response contains all tokens
                let tokens: Vec<Token> = serde_json::from_value(value).map_err(|e| {
                    FrameworkError::ProtocolError {
                        name: adapter_name.to_string(),
                        detail: format!("Invalid generate response: {e}"),
                    }
                })?;
                Ok(tokens)
            }
            Err(e) => {
                timer.failure(format!("{e}"));
                Err(e)
            }
        }
    }

    /// Send a batch generation request.
    pub async fn generate_batch(
        &self,
        adapter_name: &str,
        prompts: Vec<String>,
        params: GenerationParams,
    ) -> Result<Vec<GenerationResult>, FrameworkError> {
        let processes = self.processes.read().await;
        let process_mutex = processes
            .get(adapter_name)
            .ok_or_else(|| FrameworkError::AdapterNotFound {
                name: adapter_name.to_string(),
            })?;

        let batch_params = GenerateBatchParams { prompts, params };
        let params_json = serde_json::to_value(&batch_params)?;

        let timer = AuditTimer::start(adapter_name.to_string(), "generate_batch".to_string(), 0);

        let mut process = process_mutex.lock().await;
        let result = process.call("generate_batch", params_json).await;

        match result {
            Ok(value) => {
                timer.success();
                let batch_result: GenerateBatchResult =
                    serde_json::from_value(value).map_err(|e| FrameworkError::ProtocolError {
                        name: adapter_name.to_string(),
                        detail: format!("Invalid batch response: {e}"),
                    })?;
                Ok(batch_result.results)
            }
            Err(e) => {
                timer.failure(format!("{e}"));
                Err(e)
            }
        }
    }

    /// Send an embedding request.
    pub async fn embed(
        &self,
        adapter_name: &str,
        texts: Vec<String>,
    ) -> Result<Vec<Embedding>, FrameworkError> {
        let processes = self.processes.read().await;
        let process_mutex = processes
            .get(adapter_name)
            .ok_or_else(|| FrameworkError::AdapterNotFound {
                name: adapter_name.to_string(),
            })?;

        let embed_params = EmbedParams { texts };
        let params_json = serde_json::to_value(&embed_params)?;

        let timer = AuditTimer::start(adapter_name.to_string(), "embed".to_string(), 0);

        let mut process = process_mutex.lock().await;
        let result = process.call("embed", params_json).await;

        match result {
            Ok(value) => {
                timer.success();
                let embed_result: EmbedResult =
                    serde_json::from_value(value).map_err(|e| FrameworkError::ProtocolError {
                        name: adapter_name.to_string(),
                        detail: format!("Invalid embed response: {e}"),
                    })?;
                Ok(embed_result.embeddings)
            }
            Err(e) => {
                timer.failure(format!("{e}"));
                Err(e)
            }
        }
    }

    /// Query adapter health.
    pub async fn health_check(
        &self,
        adapter_name: &str,
    ) -> Result<HealthStatus, FrameworkError> {
        let processes = self.processes.read().await;
        let process_mutex = processes
            .get(adapter_name)
            .ok_or_else(|| FrameworkError::AdapterNotFound {
                name: adapter_name.to_string(),
            })?;

        let mut process = process_mutex.lock().await;
        let result = process.call("health_check", Value::Null).await?;

        let hc: HealthCheckResult =
            serde_json::from_value(result).map_err(|e| FrameworkError::ProtocolError {
                name: adapter_name.to_string(),
                detail: format!("Invalid health_check response: {e}"),
            })?;

        Ok(hc.status)
    }

    /// Get cached capabilities for an adapter.
    pub async fn capabilities(
        &self,
        adapter_name: &str,
    ) -> Result<AdapterCapabilities, FrameworkError> {
        let caps = self.capabilities.read().await;
        caps.get(adapter_name)
            .cloned()
            .ok_or_else(|| FrameworkError::AdapterNotFound {
                name: adapter_name.to_string(),
            })
    }

    /// Shut down a specific adapter.
    pub async fn stop_adapter(&self, adapter_name: &str) -> Result<(), FrameworkError> {
        let processes = self.processes.read().await;
        if let Some(process_mutex) = processes.get(adapter_name) {
            let mut process = process_mutex.lock().await;
            process.shutdown().await?;
            info!(adapter = %adapter_name, "Adapter stopped");
        }
        Ok(())
    }

    /// Shut down all adapters.
    pub async fn shutdown_all(&self) -> Result<(), FrameworkError> {
        let processes = self.processes.read().await;
        for (name, process_mutex) in processes.iter() {
            let mut process = process_mutex.lock().await;
            if let Err(e) = process.shutdown().await {
                error!(adapter = %name, error = %e, "Error shutting down adapter");
            }
        }
        info!("All adapters shut down");
        Ok(())
    }

    /// Run a single heartbeat cycle across all adapters.
    /// Returns list of adapters that need restart.
    pub async fn heartbeat_cycle(&self) -> Vec<String> {
        let mut dead_adapters = Vec::new();
        let health = self.health.read().await;

        for (name, state_mutex) in health.iter() {
            let mut state = state_mutex.lock().await;

            // Send heartbeat ping
            let processes = self.processes.read().await;
            if let Some(process_mutex) = processes.get(name) {
                let mut process = process_mutex.lock().await;
                if process.state() == ProcessState::Running {
                    match process.call("heartbeat", Value::Null).await {
                        Ok(_) => {
                            state.record_heartbeat();
                            process.record_heartbeat();
                        }
                        Err(_) => {
                            debug!(adapter = %name, "Heartbeat call failed");
                        }
                    }
                }
            }

            match state.check() {
                HealthCheckOutcome::Dead { .. } => {
                    dead_adapters.push(name.clone());
                }
                HealthCheckOutcome::Missed { count } => {
                    warn!(adapter = %name, missed = count, "Heartbeat missed");
                }
                HealthCheckOutcome::Healthy => {}
            }
        }

        dead_adapters
    }

    /// Attempt to restart a crashed adapter with exponential backoff.
    pub async fn restart_adapter(
        &self,
        adapter_name: &str,
        adapter_config: AdapterConfig,
    ) -> Result<(), FrameworkError> {
        let processes = self.processes.read().await;
        let process_mutex = processes
            .get(adapter_name)
            .ok_or_else(|| FrameworkError::AdapterNotFound {
                name: adapter_name.to_string(),
            })?;

        let mut process = process_mutex.lock().await;

        // Kill existing process if any
        process.kill().await;

        // Compute backoff
        let backoff = process.mark_crashed();
        match backoff {
            Some(duration) => {
                info!(
                    adapter = %adapter_name,
                    backoff_ms = duration.as_millis() as u64,
                    "Waiting before restart"
                );
                tokio::time::sleep(duration).await;

                // Respawn
                process.spawn().await?;

                // Re-initialize
                let init_params = InitializeParams {
                    config: adapter_config,
                };
                let params_json = serde_json::to_value(&init_params)?;
                let _result = process.call("initialize", params_json).await?;

                process.mark_running();

                // Reset health state
                let health = self.health.read().await;
                if let Some(state_mutex) = health.get(adapter_name) {
                    let mut state = state_mutex.lock().await;
                    state.reset();
                }

                info!(adapter = %adapter_name, "Adapter restarted successfully");
                Ok(())
            }
            None => Err(FrameworkError::MaxRestartsExceeded {
                name: adapter_name.to_string(),
                attempts: process.restart_count(),
            }),
        }
    }

    /// Get health reports for all adapters.
    pub async fn health_reports(&self) -> Vec<HealthReport> {
        let health = self.health.read().await;
        let mut reports = Vec::new();
        for (_, state_mutex) in health.iter() {
            let state = state_mutex.lock().await;
            reports.push(state.report());
        }
        reports
    }

    /// Get the list of all registered adapter names and their states.
    pub async fn list_adapters(&self) -> Vec<(String, ProcessState)> {
        let processes = self.processes.read().await;
        let mut result = Vec::new();
        for (name, process_mutex) in processes.iter() {
            let process = process_mutex.lock().await;
            result.push((name.clone(), process.state()));
        }
        result
    }

    /// Check how many in-flight requests an adapter has.
    /// Used by the scheduler for least-loaded routing.
    pub async fn adapter_in_flight(&self, _adapter_name: &str) -> usize {
        // TODO: Track in-flight request count per adapter.
        // For now returns 0; the scheduler uses its own internal tracking.
        0
    }

    /// Drain audit entries for writing to vault.
    pub async fn drain_audit(&self) -> Vec<crate::audit::AuditEntry> {
        let mut audit = self.audit.lock().await;
        audit.drain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_manager_creation() {
        let config = FrameworkConfig::default();
        let manager = AdapterManager::new(config);
        let adapters = manager.list_adapters().await;
        assert!(adapters.is_empty());
    }

    #[tokio::test]
    async fn test_adapter_not_found() {
        let config = FrameworkConfig::default();
        let manager = AdapterManager::new(config);
        let result = manager.capabilities("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_health_reports_empty() {
        let config = FrameworkConfig::default();
        let manager = AdapterManager::new(config);
        let reports = manager.health_reports().await;
        assert!(reports.is_empty());
    }

    #[tokio::test]
    async fn test_heartbeat_cycle_empty() {
        let config = FrameworkConfig::default();
        let manager = AdapterManager::new(config);
        let dead = manager.heartbeat_cycle().await;
        assert!(dead.is_empty());
    }

    #[tokio::test]
    async fn test_shutdown_all_empty() {
        let config = FrameworkConfig::default();
        let manager = AdapterManager::new(config);
        let result = manager.shutdown_all().await;
        assert!(result.is_ok());
    }
}
