//! `aog-noded` — the Loom edge/worker node daemon (VH3).
//!
//! Reads its identity + control-plane address from the environment, registers its
//! `Node` with the control plane, and heartbeats on an interval while serving
//! `/healthz`. The containerized conformance harness (VH4+) runs one per edge node.

use aog_noded::{NodeAgent, NodeConfig, NodedError};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), NodedError> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = NodeConfig::from_env()?;
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!(
        node = %config.name,
        control_plane = %config.control_plane,
        "aog-noded starting"
    );
    let agent = NodeAgent::new(config);
    agent.serve(listener).await
}
