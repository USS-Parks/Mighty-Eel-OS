//! `aogd` — the Loom control-plane node daemon (VH2).
//!
//! Reads its identity + listen address from the environment, starts a
//! [`Daemon`](aogd::Daemon) (a `RaftNode` on the `aog-wire` transport), and serves
//! the combined Raft-peer + admin API until terminated. The containerized
//! multi-node conformance harness (VH4+) runs one of these per node.

use std::net::SocketAddr;

use aogd::{Config, Daemon, DaemonError};
use tracing_subscriber::EnvFilter;

/// Phase-0.2 containment (audit C1/C2): the `/admin/*` API is unauthenticated and the
/// `/raft/*` transport is plaintext until PSPR phase A (admin auth) and A2 (mTLS) land.
/// Refuse a non-loopback bind so the control plane is not exposed off-host by default;
/// loopback always proceeds, and an operator on a trusted isolated network can opt in.
fn check_bind_containment(listen: &SocketAddr, allow_insecure: bool) -> Result<(), DaemonError> {
    if listen.ip().is_loopback() || allow_insecure {
        return Ok(());
    }
    Err(DaemonError::Config(format!(
        "AOGD_LISTEN={listen} is non-loopback, but the /admin/* API is unauthenticated and \
         /raft/* is plaintext until the auth/mTLS remediation (PSPR phase A) lands. Bind a \
         loopback address, or set AOGD_ALLOW_INSECURE_BIND=1 to accept the risk on a trusted, \
         isolated network."
    )))
}

#[tokio::main]
async fn main() -> Result<(), DaemonError> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env()?;
    let allow_insecure = std::env::var("AOGD_ALLOW_INSECURE_BIND").ok().as_deref() == Some("1");
    check_bind_containment(&config.listen, allow_insecure)?;
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

#[cfg(test)]
mod tests {
    use super::{SocketAddr, check_bind_containment};

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn loopback_ok_nonloopback_refused_unless_optin() {
        // Loopback always proceeds.
        assert!(check_bind_containment(&addr("127.0.0.1:4600"), false).is_ok());
        assert!(check_bind_containment(&addr("[::1]:4600"), false).is_ok());
        // Non-loopback (incl. the all-interfaces 0.0.0.0) is refused by default.
        assert!(check_bind_containment(&addr("0.0.0.0:4600"), false).is_err());
        assert!(check_bind_containment(&addr("10.0.0.5:4600"), false).is_err());
        // Explicit opt-in permits a non-loopback bind.
        assert!(check_bind_containment(&addr("0.0.0.0:4600"), true).is_ok());
    }
}
