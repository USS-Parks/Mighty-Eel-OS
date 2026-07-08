//! gRPC server builder for the MAI API.
//!
//! Constructs a tonic Server with all six MAI services plus the standard
//! grpc.health.v1 health checking protocol registered. The gRPC server
//! runs on a separate port (default 8421) from the REST server (default 8420).
//!
//! # Services Registered
//!
//! - MaiInference (chat completion, streaming, embeddings)
//! - MaiModels (listing, load, unload)
//! - MaiHealth (adapter, hardware, system, watch)
//! - MaiPower (state query, transition)
//! - MaiRegistry (query, scan)
//! - MaiAudit (log retrieval)
//! - Health (grpc.health.v1 standard)
//! - Reflection (for grpcurl/grpcui discovery)
//!
//! # Shared State
//!
//! The gRPC server shares the same AppState (scheduler, registry, health,
//! power, hotswap, audit_writer, config, auth) as the REST server. Both
//! servers see the same component instances via Arc.

use std::net::SocketAddr;
use tonic::transport::Server;
use tracing::{error, info};

use super::audit::MaiAuditService;
use super::health::{GrpcHealthService, MaiHealthService};
use super::inference::MaiInferenceService;
use super::models::MaiModelsService;
use super::power::MaiPowerService;
use super::proto;
use super::registry::MaiRegistryService;
use crate::state::AppState;

/// Default gRPC server port.
pub const DEFAULT_GRPC_PORT: u16 = 8421;

/// Configuration for the gRPC server.
#[derive(Debug, Clone)]
pub struct GrpcServerConfig {
    /// Address to bind the gRPC server to.
    pub bind_addr: SocketAddr,
    /// Whether to enable the reflection service.
    pub enable_reflection: bool,
    /// Maximum message size in bytes (default: 16 MiB).
    pub max_message_size: usize,
    /// TCP keepalive interval in seconds.
    pub keepalive_secs: Option<u64>,
}

impl Default for GrpcServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([0, 0, 0, 0], DEFAULT_GRPC_PORT)),
            enable_reflection: true,
            max_message_size: 16 * 1024 * 1024, // 16 MiB
            keepalive_secs: Some(60),
        }
    }
}

/// Build and return a configured tonic Server ready to serve.
///
/// The server is built but not started. The caller is responsible for
/// calling `.serve(addr)` on the returned future, typically alongside
/// the REST server in a `tokio::select!` or `tokio::join!`.
pub async fn build_grpc_server(
    state: AppState,
    config: GrpcServerConfig,
) -> Result<
    impl std::future::Future<Output = Result<(), tonic::transport::Error>>,
    Box<dyn std::error::Error>,
> {
    info!(
        addr = %config.bind_addr,
        reflection = config.enable_reflection,
        max_message_size = config.max_message_size,
        "building gRPC server"
    );

    // Build all service implementations
    let inference_svc = proto::mai_inference_server::MaiInferenceServer::new(
        MaiInferenceService::new(state.clone()),
    )
    .max_decoding_message_size(config.max_message_size)
    .max_encoding_message_size(config.max_message_size);

    let models_svc =
        proto::mai_models_server::MaiModelsServer::new(MaiModelsService::new(state.clone()));

    let mai_health_svc =
        proto::mai_health_server::MaiHealthServer::new(MaiHealthService::new(state.clone()));

    let power_svc =
        proto::mai_power_server::MaiPowerServer::new(MaiPowerService::new(state.clone()));

    let registry_svc =
        proto::mai_registry_server::MaiRegistryServer::new(MaiRegistryService::new(state.clone()));

    let audit_svc =
        proto::mai_audit_server::MaiAuditServer::new(MaiAuditService::new(state.clone()));

    let grpc_health_svc =
        proto::health_server::HealthServer::new(GrpcHealthService::new(state.clone()));

    // Build reflection service (allows grpcurl and grpcui to discover services)
    let reflection_svc = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build_v1()
        .map_err(|e| {
            error!(error = %e, "failed to build reflection service");
            e
        })?;

    // Assemble the server
    let mut builder = Server::builder();

    if let Some(keepalive) = config.keepalive_secs {
        builder = builder.tcp_keepalive(Some(std::time::Duration::from_secs(keepalive)));
    }

    let router = builder
        .add_service(inference_svc)
        .add_service(models_svc)
        .add_service(mai_health_svc)
        .add_service(power_svc)
        .add_service(registry_svc)
        .add_service(audit_svc)
        .add_service(grpc_health_svc);

    // Reflection is opt-in (F1): only expose the full service/method map when
    // explicitly enabled. Production leaves it off so a client cannot enumerate
    // the gRPC surface. Previously `reflection_svc` was added unconditionally,
    // ignoring `config.enable_reflection`.
    let router = if config.enable_reflection {
        router.add_service(reflection_svc)
    } else {
        router
    };

    let addr = config.bind_addr;
    let serve_future = router.serve(addr);

    info!(addr = %addr, "gRPC server configured with all services");

    Ok(serve_future)
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = GrpcServerConfig::default();
        assert_eq!(config.bind_addr.port(), DEFAULT_GRPC_PORT);
        assert!(config.enable_reflection);
        assert_eq!(config.max_message_size, 16 * 1024 * 1024);
        assert_eq!(config.keepalive_secs, Some(60));
    }

    #[test]
    fn test_custom_config() {
        let config = GrpcServerConfig {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 9090)),
            enable_reflection: false,
            max_message_size: 4 * 1024 * 1024,
            keepalive_secs: None,
        };
        assert_eq!(config.bind_addr.port(), 9090);
        assert!(!config.enable_reflection);
    }
}
