//! P1 — production startup posture for the `wsf-api` binary.
//!
//! The trust plane binds loopback by default; an operator widens `WSF_LISTEN`
//! to a public interface only behind an authenticated ingress. This module is
//! the enforcement: a **public (non-loopback) bind** must present a real
//! workload-credential authority key *and* a hardened OpenBao/HMAC config, or
//! the service refuses to start — it must never answer a public interface with
//! the local-dev authenticator or a dev-fixture config.

use std::net::SocketAddr;

/// True when any resolved bind address is non-loopback — i.e. reachable
/// off-host (`0.0.0.0`, `::`, and any routable IP all qualify).
#[must_use]
pub fn is_public_bind(addrs: &[SocketAddr]) -> bool {
    addrs.iter().any(|a| !a.ip().is_loopback())
}

/// Enforce the production startup posture for a resolved bind.
///
/// A loopback bind is unrestricted (the dev fallback stays host-only). A public
/// bind must satisfy both:
///   * [`wsf_hardening::assert_production_ready`] — no `http://` OpenBao, no dev
///     root token, no weak/uniform subject-HMAC key; and
///   * `has_workload_key` — a workload-credential authority key is configured,
///     so the local-dev authenticator is not what answers a public interface.
///
/// # Errors
/// A human-readable reason when a public bind is missing the authority key or
/// carries a dev-fixture config.
pub fn enforce_startup_posture(
    public_bind: bool,
    has_workload_key: bool,
    cfg: &wsf_hardening::DeploymentConfig,
) -> Result<(), String> {
    if !public_bind {
        return Ok(());
    }
    if let Err(violations) = wsf_hardening::assert_production_ready(cfg) {
        let detail = violations
            .iter()
            .map(|v| format!("{}: {}", v.code, v.detail))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!(
            "refusing public bind: production config not ready — {detail}"
        ));
    }
    if !has_workload_key {
        return Err(
            "refusing public bind: WSF_WORKLOAD_AUTHORITY_KEY is required for a \
             non-loopback bind (the local-dev authenticator must not serve a public interface)"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wsf_hardening::{DeployMode, DeploymentConfig};

    fn hardened() -> DeploymentConfig {
        DeploymentConfig {
            mode: DeployMode::Production,
            openbao_address: "https://openbao.internal:8200".to_string(),
            openbao_token: "s.real-approle-secret".to_string(),
            subject_hmac_key: (0u8..32).collect(),
        }
    }

    fn parse(a: &str) -> Vec<SocketAddr> {
        vec![a.parse().unwrap()]
    }

    #[test]
    fn loopback_is_not_public() {
        assert!(!is_public_bind(&parse("127.0.0.1:8300")));
        assert!(!is_public_bind(&parse("[::1]:8300")));
    }

    #[test]
    fn wildcard_and_routable_are_public() {
        assert!(is_public_bind(&parse("0.0.0.0:8300")));
        assert!(is_public_bind(&parse("[::]:8300")));
        assert!(is_public_bind(&parse("10.0.0.5:8300")));
    }

    #[test]
    fn loopback_bind_is_unrestricted() {
        // No key and a dev-ish config: a loopback bind still starts, because the
        // dev fallback is only reachable from the host.
        let cfg = DeploymentConfig {
            mode: DeployMode::Production,
            openbao_address: "http://127.0.0.1:8250".to_string(),
            openbao_token: "root".to_string(),
            subject_hmac_key: vec![7u8; 32],
        };
        assert!(enforce_startup_posture(false, false, &cfg).is_ok());
    }

    #[test]
    fn public_bind_without_workload_key_refuses() {
        let err = enforce_startup_posture(true, false, &hardened()).unwrap_err();
        assert!(err.contains("WSF_WORKLOAD_AUTHORITY_KEY"), "reason: {err}");
    }

    #[test]
    fn public_bind_with_dev_fixtures_refuses() {
        let dev = DeploymentConfig {
            mode: DeployMode::Production,
            openbao_address: "http://openbao:8200".to_string(),
            openbao_token: "root".to_string(),
            subject_hmac_key: vec![7u8; 32],
        };
        // Even with a key present, dev fixtures block a public bind.
        let err = enforce_startup_posture(true, true, &dev).unwrap_err();
        assert!(err.contains("insecure_transport"), "reason: {err}");
    }

    #[test]
    fn public_bind_hardened_with_key_starts() {
        assert!(enforce_startup_posture(true, true, &hardened()).is_ok());
    }
}
