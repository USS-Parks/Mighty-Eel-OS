//! `aogd` — the Loom control-plane node daemon.
//!
//! Reads its identity + listen address from the environment, starts a
//! [`Daemon`](aogd::Daemon) (a `RaftNode` on the `aog-wire` transport), and serves
//! the combined Raft-peer + admin API until terminated. The containerized
//! multi-node conformance harness runs one of these per node.

use std::net::SocketAddr;

use aog_wire::tls::{TlsListener, TlsPeer};
use aogd::{Config, Daemon, DaemonError};
use tracing_subscriber::EnvFilter;

/// Runtime posture. Absence is production so a typo or omitted variable cannot
/// silently select the permissive harness behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Profile {
    Production,
    Development,
}

impl Profile {
    fn parse(value: Option<&str>) -> Result<Self, DaemonError> {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("production") | Some("prod") => Ok(Self::Production),
            Some("development") | Some("dev") => Ok(Self::Development),
            Some(other) => Err(DaemonError::Config(format!(
                "unrecognized AOGD_PROFILE '{other}' (expected production | development)"
            ))),
        }
    }
}

/// Emergency containment for the two control-plane trust gaps. Production may
/// not start until the roster installs both authenticated admin authorization
/// and peer mTLS. Development remains available only by an explicit profile;
/// widening its plaintext bind requires a second, conspicuous opt-in.
fn check_startup_posture(
    profile: Profile,
    listen: &SocketAddr,
    has_trust: bool,
    has_node_tls_source: bool,
    allow_insecure_development_bind: bool,
    allow_insecure_admin: bool,
) -> Result<(), DaemonError> {
    if profile == Profile::Production {
        if allow_insecure_admin {
            return Err(DaemonError::Config(
                "AOGD_ALLOW_INSECURE_ADMIN is forbidden in production".to_owned(),
            ));
        }
        if !has_trust {
            return Err(DaemonError::Config(
                "production requires AOGD_ANCHOR_PUBKEY or AOGD_OPENBAO_ADDR; refusing the \
                 fail-open admin posture before socket bind"
                    .to_string(),
            ));
        }
        if !has_node_tls_source {
            return Err(DaemonError::Config(
                "production requires node TLS from AOGD_RAFT_TLS_OPENBAO_PATH or the complete \
                 AOGD_RAFT_{CA,CERT,KEY}_DER_PATH set"
                    .to_string(),
            ));
        }
        return Ok(());
    }

    if listen.ip().is_loopback() || allow_insecure_development_bind {
        return Ok(());
    }
    Err(DaemonError::Config(format!(
        "development AOGD_LISTEN={listen} is non-loopback while /admin and /raft are not yet \
         production-hardened; bind loopback or explicitly set AOGD_ALLOW_INSECURE_BIND=1"
    )))
}

#[tokio::main]
async fn main() -> Result<(), DaemonError> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let profile = Profile::parse(std::env::var("AOGD_PROFILE").ok().as_deref())?;
    let config = Config::from_env()?;
    let allow_insecure = std::env::var("AOGD_ALLOW_INSECURE_BIND").ok().as_deref() == Some("1");
    let has_trust = config.anchor_pubkey.is_some() || config.openbao.is_some();
    let has_node_tls_source = config.node_tls.is_some();
    let allow_insecure_admin = config.allow_insecure_admin;
    check_startup_posture(
        profile,
        &config.listen,
        has_trust,
        has_node_tls_source,
        allow_insecure,
        allow_insecure_admin,
    )?;
    let listen = config.listen;
    let daemon = Daemon::start(config).await?;
    let listener = tokio::net::TcpListener::bind(listen).await?;

    tracing::info!(
        node_id = daemon.node().id(),
        advertise = daemon.advertise(),
        "aogd started"
    );

    let server_tls = daemon
        .node_tls()
        .map(|tls| tls.server_config())
        .transpose()
        .map_err(|e| DaemonError::Config(format!("node TLS server config: {e}")))?;
    let app = daemon.app();
    if let Some(server_tls) = server_tls {
        axum::serve(
            TlsListener::new(listener, server_tls),
            app.into_make_service_with_connect_info::<TlsPeer>(),
        )
        .await?;
    } else {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Profile, SocketAddr, check_startup_posture};

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn profile_defaults_to_production_and_rejects_unknown_values() {
        assert_eq!(Profile::parse(None).unwrap(), Profile::Production);
        assert_eq!(Profile::parse(Some("")).unwrap(), Profile::Production);
        assert_eq!(
            Profile::parse(Some("development")).unwrap(),
            Profile::Development
        );
        assert!(Profile::parse(Some("demo-ish")).is_err());
    }

    #[test]
    fn production_refuses_missing_trust_before_transport_check() {
        let err = check_startup_posture(
            Profile::Production,
            &addr("127.0.0.1:4600"),
            false,
            false,
            true,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("requires AOGD_ANCHOR_PUBKEY"));
    }

    #[test]
    fn production_refuses_missing_node_tls_before_transport_check() {
        let err = check_startup_posture(
            Profile::Production,
            &addr("127.0.0.1:4600"),
            true,
            false,
            true,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("requires node TLS"));
    }

    #[test]
    fn production_with_identity_passes_prebind_posture_for_mtls_startup() {
        assert!(
            check_startup_posture(
                Profile::Production,
                &addr("127.0.0.1:4600"),
                true,
                true,
                false,
                false,
            )
            .is_ok()
        );
    }

    #[test]
    fn development_nonloopback_requires_second_explicit_opt_in() {
        assert!(
            check_startup_posture(
                Profile::Development,
                &addr("127.0.0.1:4600"),
                false,
                false,
                false,
                false,
            )
            .is_ok()
        );
        assert!(
            check_startup_posture(
                Profile::Development,
                &addr("0.0.0.0:4600"),
                false,
                false,
                false,
                false,
            )
            .is_err()
        );
        assert!(
            check_startup_posture(
                Profile::Development,
                &addr("0.0.0.0:4600"),
                false,
                false,
                true,
                true,
            )
            .is_ok()
        );
    }
}
