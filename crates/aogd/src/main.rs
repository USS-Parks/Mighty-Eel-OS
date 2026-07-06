//! `aogd` — the Loom control-plane node daemon (VH2).
//!
//! Reads its identity + listen address from the environment, starts a
//! [`Daemon`](aogd::Daemon) (a `RaftNode` on the `aog-wire` transport), and serves
//! the combined Raft-peer + admin API until terminated. The containerized
//! multi-node conformance harness (VH4+) runs one of these per node.

use aogd::{Config, Daemon, DaemonError};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), DaemonError> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env()?;
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let daemon = Daemon::start(config).await?;

    tracing::info!(
        node_id = daemon.node().id(),
        advertise = daemon.advertise(),
        "aogd started"
    );

    axum::serve(listener, daemon.app()).await?;
    Ok(())
}
