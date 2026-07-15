//! Production startup posture for the `wsf-api` binary.
//!
//! Production requirements apply on every bind, including loopback: hardened
//! OpenBao/HMAC material, workload authentication, and mandatory revocation.
//! The local-dev authenticator and fixture configuration are available only
//! under an explicit development profile; an isolated non-loopback development
//! bind requires a second explicit opt-in.

use std::net::SocketAddr;

/// Runtime posture. Missing or blank means production; development must be an
/// explicit operator choice and is never inferred from loopback binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Production,
    Development,
}

impl Profile {
    /// Parse `WSF_PROFILE` with a fail-safe production default.
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("production") | Some("prod") => Ok(Self::Production),
            Some("development") | Some("dev") => Ok(Self::Development),
            Some(other) => Err(format!(
                "unrecognized WSF_PROFILE '{other}' (expected production | development)"
            )),
        }
    }
}

/// True when any resolved bind address is non-loopback — i.e. reachable
/// off-host (`0.0.0.0`, `::`, and any routable IP all qualify).
#[must_use]
pub fn is_public_bind(addrs: &[SocketAddr]) -> bool {
    addrs.iter().any(|a| !a.ip().is_loopback())
}

/// Enforce the selected startup posture for a resolved bind.
///
/// Production always requires [`wsf_hardening::assert_production_ready`], a
/// workload authority key, and a wired revocation store. Development permits
/// fixture configuration on loopback; a non-loopback bind additionally needs
/// `allow_insecure_development_bind`.
///
/// # Errors
/// A human-readable reason when the selected posture is incomplete or unsafe.
pub fn enforce_startup_posture(
    profile: Profile,
    public_bind: bool,
    has_workload_key: bool,
    has_revocation_store: bool,
    allow_insecure_development_bind: bool,
    cfg: &wsf_hardening::DeploymentConfig,
) -> Result<(), String> {
    if profile == Profile::Development {
        if public_bind && !allow_insecure_development_bind {
            return Err(
                "refusing non-loopback development bind: set WSF_ALLOW_INSECURE_BIND=1 only \
                 for an isolated demo network"
                    .to_string(),
            );
        }
        return Ok(());
    }

    if let Err(violations) = wsf_hardening::assert_production_ready(cfg) {
        let detail = violations
            .iter()
            .map(|v| format!("{}: {}", v.code, v.detail))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!(
            "refusing production startup: production config not ready — {detail}"
        ));
    }
    if !has_workload_key {
        return Err(
            "refusing production startup: WSF_WORKLOAD_AUTHORITY_KEY is required; the \
             local-dev authenticator is development-only"
                .to_string(),
        );
    }
    if !has_revocation_store {
        return Err(
            "refusing production startup: a mandatory revocation store must be wired into \
             every privileged WSF service"
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
    fn profile_defaults_to_production_and_development_is_explicit() {
        assert_eq!(Profile::parse(None).unwrap(), Profile::Production);
        assert_eq!(Profile::parse(Some("")).unwrap(), Profile::Production);
        assert_eq!(
            Profile::parse(Some("development")).unwrap(),
            Profile::Development
        );
        assert!(Profile::parse(Some("demo-ish")).is_err());
    }

    #[test]
    fn development_loopback_is_allowed_but_public_bind_needs_second_opt_in() {
        let cfg = DeploymentConfig {
            mode: DeployMode::Production,
            openbao_address: "http://127.0.0.1:8250".to_string(),
            openbao_token: "root".to_string(),
            subject_hmac_key: vec![7u8; 32],
        };
        assert!(
            enforce_startup_posture(Profile::Development, false, false, false, false, &cfg).is_ok()
        );
        assert!(
            enforce_startup_posture(Profile::Development, true, false, false, false, &cfg).is_err()
        );
        assert!(
            enforce_startup_posture(Profile::Development, true, false, false, true, &cfg).is_ok()
        );
    }

    #[test]
    fn production_loopback_without_workload_key_refuses() {
        let err =
            enforce_startup_posture(Profile::Production, false, false, true, true, &hardened())
                .unwrap_err();
        assert!(err.contains("WSF_WORKLOAD_AUTHORITY_KEY"), "reason: {err}");
    }

    #[test]
    fn production_with_dev_fixtures_refuses() {
        let dev = DeploymentConfig {
            mode: DeployMode::Production,
            openbao_address: "http://openbao:8200".to_string(),
            openbao_token: "root".to_string(),
            subject_hmac_key: vec![7u8; 32],
        };
        let err = enforce_startup_posture(Profile::Production, false, true, true, true, &dev)
            .unwrap_err();
        assert!(err.contains("insecure_transport"), "reason: {err}");
    }

    #[test]
    fn production_without_revocation_refuses_even_when_other_inputs_are_hardened() {
        let err =
            enforce_startup_posture(Profile::Production, false, true, false, true, &hardened())
                .unwrap_err();
        assert!(err.contains("mandatory revocation store"), "reason: {err}");
    }

    #[test]
    fn production_hardened_with_key_and_revocation_starts() {
        assert!(
            enforce_startup_posture(Profile::Production, true, true, true, false, &hardened())
                .is_ok()
        );
    }
}
