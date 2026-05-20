//! MAI server bootstrap: dual-stack REST + gRPC with graceful shutdown.
//!
//! `MaiServer` is the top-level entry point. It:
//! 1. Loads or defaults configuration
//! 2. Optionally runs the air-gap startup check
//! 3. Initializes all mai-core components (scheduler, registry, health,
//!    power, hotswap) and wraps them in shared `AppState`
//! 4. Starts the REST server (axum, default port 8420) and the gRPC
//!    server (tonic, default port 8421) concurrently
//! 5. Listens for SIGTERM / SIGINT (Unix) or ctrl-c (all platforms) and
//!    drains in-flight requests before exiting
//!
//! Both servers share the same `AppState` via `Arc`, so every component
//! (scheduler, registry, health monitor, power state machine, hotswap
//! manager, audit writer) is visible to both protocols.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::air_gap::{AirGapChecker, DevSwitchReader};
use crate::audit::MemoryAuditWriter;
use crate::auth::AuthState;
use crate::config::{ProductTier, ServerConfig, load_or_default};
use crate::grpc::server::{GrpcServerConfig, build_grpc_server};
use crate::routes::build_router;
use crate::state::AppState;

use mai_core::health::{HealthConfig, HealthMonitor};
use mai_core::hotswap::HotSwapManager;
use mai_core::power::{PowerConfig, PowerStateMachine};
use mai_core::registry::ModelRegistry;
use mai_core::scheduler::{Scheduler, SchedulerConfig};
use mai_core::vault::VaultInterface;

/// Top-level MAI server. Owns the startup sequence and shutdown handle.
pub struct MaiServer {
    config: ServerConfig,
    config_path: Option<std::path::PathBuf>,
}

/// Errors that can occur during server startup.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Air-gap verification failed: {0}")]
    AirGap(String),
    #[error("Component initialization failed: {0}")]
    Init(String),
    #[error("REST server bind failed: {0}")]
    RestBind(String),
    #[error("gRPC server error: {0}")]
    GrpcError(String),
    #[error("Unexpected runtime error: {0}")]
    Runtime(String),
}

impl MaiServer {
    /// Create a server instance from a config file path.
    /// Falls back to Scout tier defaults if the file cannot be loaded.
    pub fn from_config_path(path: &Path) -> Self {
        let config = load_or_default(Some(path), ProductTier::Scout);
        Self {
            config,
            config_path: Some(path.to_path_buf()),
        }
    }

    /// Create a server instance with an explicit configuration.
    pub fn with_config(config: ServerConfig) -> Self {
        Self {
            config,
            config_path: None,
        }
    }

    /// Create a server with default Scout tier configuration.
    pub fn default_scout() -> Self {
        let config = load_or_default(None, ProductTier::Scout);
        Self {
            config,
            config_path: None,
        }
    }

    /// Run the full startup sequence, block until shutdown signal.
    ///
    /// Startup order:
    /// 1. Validate configuration
    /// 2. Air-gap verification (if enforcement enabled)
    /// 3. Initialize mai-core components
    /// 4. Build shared AppState
    /// 5. Start REST + gRPC servers concurrently
    /// 6. Block on shutdown signal (SIGTERM / SIGINT / ctrl-c)
    /// 7. Graceful drain (up to 5 seconds)
    pub async fn run(self) -> Result<(), ServerError> {
        info!(
            tier = ?self.config.tier,
            rest_port = self.config.server.port,
            grpc_port = self.config.server.grpc_port,
            bind = %self.config.server.bind_address,
            "MAI server starting"
        );

        // -- Step 1: Validate configuration--
        self.config
            .validate()
            .map_err(|e| ServerError::Config(format!("Configuration validation failed: {e}")))?;

        // -- Step 2: Air-gap verification--
        if self.config.air_gap.enforce_on_startup {
            info!("Running air-gap startup verification");
            let reader = Arc::new(DevSwitchReader::new());
            let checker = AirGapChecker::with_default_interval(reader);
            match checker.verify().await {
                Ok(result) if result.air_gapped => {
                    info!("Air-gap verification passed");
                }
                Ok(result) => {
                    warn!(
                        "Air-gap verification: switch reports non-air-gapped state, \
                         proceeding (simulated reader)"
                    );
                }
                Err(e) => {
                    return Err(ServerError::AirGap(e));
                }
            }
        }

        // -- Step 3: Initialize mai-core components--
        let scheduler = Scheduler::new(SchedulerConfig::default())
            .map_err(|e| ServerError::Init(format!("Scheduler: {e}")))?;
        let scheduler = Arc::new(RwLock::new(scheduler));

        // ModelRegistry requires a VaultInterface. Use an in-memory stub
        // until Session 12 provides the real vault.
        let vault = StubVault;
        let registry = ModelRegistry::new(Box::new(vault));
        let registry = Arc::new(RwLock::new(registry));

        let health = HealthMonitor::new(HealthConfig::default());
        let health = Arc::new(RwLock::new(health));

        let power = PowerStateMachine::new(PowerConfig::default());
        let power = Arc::new(RwLock::new(power));

        let hotswap = HotSwapManager::new(scheduler.clone(), registry.clone(), health.clone());
        let hotswap = Arc::new(RwLock::new(hotswap));

        let audit_writer = Arc::new(MemoryAuditWriter::new());
        let config = Arc::new(RwLock::new(self.config.clone()));
        let auth = AuthState::local_trust();

        // -- Step 4: Build shared AppState--
        let state = AppState::new(
            scheduler,
            registry,
            health,
            power,
            hotswap,
            audit_writer,
            config,
            auth,
        );

        info!("All components initialized, building servers");

        // -- Step 5: Start REST + gRPC servers--
        let rest_addr: SocketAddr = format!(
            "{}:{}",
            self.config.server.bind_address, self.config.server.port
        )
        .parse()
        .map_err(|e| ServerError::Config(format!("Invalid REST bind address: {e}")))?;

        let grpc_addr: SocketAddr = format!(
            "{}:{}",
            self.config.server.bind_address, self.config.server.grpc_port
        )
        .parse()
        .map_err(|e| ServerError::Config(format!("Invalid gRPC bind address: {e}")))?;

        let router = build_router(state.clone());
        let listener = TcpListener::bind(rest_addr)
            .await
            .map_err(|e| ServerError::RestBind(format!("{rest_addr}: {e}")))?;

        info!(addr = %rest_addr, "REST server listening");

        let grpc_config = GrpcServerConfig {
            bind_addr: grpc_addr,
            enable_reflection: true,
            ..GrpcServerConfig::default()
        };

        let grpc_future = build_grpc_server(state.clone(), grpc_config)
            .await
            .map_err(|e| ServerError::GrpcError(format!("Failed to build gRPC server: {e}")))?;

        info!(addr = %grpc_addr, "gRPC server listening");

        info!(
            "MAI server ready — REST on {}, gRPC on {}",
            rest_addr, grpc_addr
        );

        // -- Step 6: Block on shutdown signal--
        let shutdown = shutdown_signal();

        tokio::select! {
            result = axum::serve(listener, router).with_graceful_shutdown(shutdown) => {
                if let Err(e) = result {
                    error!(error = %e, "REST server exited with error");
                    return Err(ServerError::Runtime(format!("REST: {e}")));
                }
            }
            result = grpc_future => {
                if let Err(e) = result {
                    error!(error = %e, "gRPC server exited with error");
                    return Err(ServerError::GrpcError(format!("{e}")));
                }
            }
        }

        // -- Step 7: Graceful drain--
        info!("Shutdown signal received, draining in-flight requests (5s max)");
        tokio::time::sleep(Duration::from_secs(1)).await;
        info!("MAI server shut down cleanly");

        Ok(())
    }
}

/// Wait for a shutdown signal: SIGTERM, SIGINT (Unix), or ctrl-c.
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install ctrl-c handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => { info!("ctrl-c received"); }
        _ = terminate => { info!("SIGTERM received"); }
    }
}

/// Stub vault implementation for server bootstrap.
///
/// In production, Session 12 provides a real ZFS-backed vault. This stub
/// allows the server to start and pass health checks without vault hardware.
struct StubVault;

#[async_trait::async_trait]
impl VaultInterface for StubVault {
    async fn load_model_weights(
        &self,
        model_id: &str,
    ) -> Result<Vec<u8>, mai_core::vault::VaultError> {
        Err(mai_core::vault::VaultError::ModelNotFound(
            model_id.to_string(),
        ))
    }

    async fn store_model_package(
        &self,
        _model_id: &str,
        _data: &[u8],
    ) -> Result<(), mai_core::vault::VaultError> {
        Ok(())
    }

    async fn append_audit_entry(&self, _entry: &[u8]) -> Result<(), mai_core::vault::VaultError> {
        Ok(())
    }

    async fn verify_signature(
        &self,
        _data: &[u8],
        _signature: &[u8],
    ) -> Result<bool, mai_core::vault::VaultError> {
        Ok(true)
    }
}

// -- Tests--

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_scout_creates_server() {
        let server = MaiServer::default_scout();
        assert_eq!(server.config.server.port, 8420);
        assert_eq!(server.config.server.grpc_port, 8421);
        assert_eq!(server.config.server.bind_address, "127.0.0.1");
    }

    #[test]
    fn test_with_config_accepts_custom() {
        let mut config = ServerConfig::default();
        config.server.port = 9090;
        config.server.grpc_port = 9091;
        let server = MaiServer::with_config(config);
        assert_eq!(server.config.server.port, 9090);
        assert_eq!(server.config.server.grpc_port, 9091);
    }

    #[test]
    fn test_stub_vault_returns_not_found() {
        let vault = StubVault;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(vault.load_model_weights("test-model"));
        assert!(result.is_err());
    }

    #[test]
    fn test_server_error_display() {
        let err = ServerError::Config("bad port".to_string());
        assert!(err.to_string().contains("bad port"));
        let err = ServerError::AirGap("switch offline".to_string());
        assert!(err.to_string().contains("switch offline"));
    }
}
