//! Adapter host validation under air-gap policy.
//!
//! Loopback enforcement runs at two points:
//!
//! 1. **Config load** — [`validate_adapter_host`] is called for every
//!    [`mai_hil::traits::AdapterConfig::host`] when a TOML config is
//!    parsed. Any non-loopback address fails immediately when the
//!    [`ConnectivityState`] is [`ConnectivityState::AirGapped`].
//! 2. **Hot reload** — the same validator is re-run after a config
//!    swap so a previously-valid config that introduced a network host
//!    cannot slip in while the runtime is air-gapped.
//!
//! The unbiased wildcard `0.0.0.0` and IPv6 unspecified `::` are always
//! rejected regardless of state — they bind every interface and have no
//! legitimate use in this codebase.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use mai_core::airgap::ConnectivityState;

use crate::errors::FrameworkError;

/// Failure reasons surfaced by [`validate_adapter_host`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostValidationError {
    /// The host string failed to parse as either an IP address or a
    /// hostname suitable for loopback comparison.
    Unparseable(String),
    /// The host is one of the wildcard "bind all" addresses, which are
    /// never permitted.
    WildcardForbidden(String),
    /// The current [`ConnectivityState`] forbids non-loopback hosts and
    /// the configured host is non-loopback.
    NonLoopbackUnderAirGap {
        host: String,
        state: ConnectivityState,
    },
    /// The host is non-loopback and is not present in the operator-approved
    /// allowlist. Returned only when the state permits network traffic.
    NotInAllowList(String),
}

impl std::fmt::Display for HostValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unparseable(h) => write!(f, "adapter host {h:?} could not be parsed"),
            Self::WildcardForbidden(h) => write!(
                f,
                "adapter host {h:?} is a wildcard bind address and is always forbidden"
            ),
            Self::NonLoopbackUnderAirGap { host, state } => write!(
                f,
                "adapter host {host:?} is non-loopback but connectivity state is {state}; \
                 loopback (127.0.0.1 or ::1) is required when air-gapped"
            ),
            Self::NotInAllowList(h) => write!(
                f,
                "adapter host {h:?} is not in the operator-approved allowlist"
            ),
        }
    }
}

impl std::error::Error for HostValidationError {}

impl From<HostValidationError> for FrameworkError {
    fn from(err: HostValidationError) -> Self {
        FrameworkError::ConfigError {
            name: "adapter-host".to_string(),
            reason: err.to_string(),
        }
    }
}

/// Validate a single adapter host string against the current connectivity
/// state and the operator allowlist.
///
/// The allowlist is consulted only when [`ConnectivityState::permits_cloud_route`]
/// returns true — i.e. `Connected` or `Degraded`. In every other state the
/// host MUST be a loopback literal.
///
/// `allow_list` should contain canonical host strings (typically IPs); an
/// empty list means "loopback only, no networked hosts permitted".
pub fn validate_adapter_host(
    host: &str,
    state: ConnectivityState,
    allow_list: &[String],
) -> Result<(), HostValidationError> {
    let trimmed = host.trim();
    if trimmed.is_empty() {
        return Err(HostValidationError::Unparseable(host.to_string()));
    }

    // Wildcard binds are always forbidden — they expose every interface.
    if is_wildcard(trimmed) {
        return Err(HostValidationError::WildcardForbidden(host.to_string()));
    }

    let loopback = is_loopback(trimmed);

    if state.is_air_gapped() || state.requires_local_only() {
        if loopback {
            return Ok(());
        }
        return Err(HostValidationError::NonLoopbackUnderAirGap {
            host: host.to_string(),
            state,
        });
    }

    // State permits some network traffic. Loopback always wins; otherwise
    // consult the allowlist.
    if loopback {
        return Ok(());
    }

    if allow_list.iter().any(|h| h.trim() == trimmed) {
        return Ok(());
    }

    Err(HostValidationError::NotInAllowList(host.to_string()))
}

/// Returns true iff `host` is a loopback literal in either v4 or v6 form.
/// Hostnames are not accepted as loopback — the explicit literal must be
/// used so that the validator does not depend on the system resolver.
#[must_use]
pub fn is_loopback(host: &str) -> bool {
    // Strip a trailing scope id like `::1%lo0`.
    let bare = host.split('%').next().unwrap_or(host);
    if let Ok(addr) = bare.parse::<IpAddr>() {
        return match addr {
            IpAddr::V4(v4) => v4 == Ipv4Addr::LOCALHOST,
            IpAddr::V6(v6) => v6 == Ipv6Addr::LOCALHOST,
        };
    }
    false
}

/// Returns true iff `host` is the v4 or v6 "any" wildcard.
#[must_use]
pub fn is_wildcard(host: &str) -> bool {
    let bare = host.split('%').next().unwrap_or(host);
    if let Ok(addr) = bare.parse::<IpAddr>() {
        return match addr {
            IpAddr::V4(v4) => v4 == Ipv4Addr::UNSPECIFIED,
            IpAddr::V6(v6) => v6 == Ipv6Addr::UNSPECIFIED,
        };
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_v4_v6_recognised() {
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("::1"));
        assert!(is_loopback("::1%lo0"));
        assert!(!is_loopback("127.0.0.2"));
        assert!(!is_loopback("localhost")); // explicit literal required
        assert!(!is_loopback("10.0.0.1"));
    }

    #[test]
    fn wildcards_recognised() {
        assert!(is_wildcard("0.0.0.0"));
        assert!(is_wildcard("::"));
        assert!(!is_wildcard("127.0.0.1"));
        assert!(!is_wildcard(""));
    }

    #[test]
    fn loopback_allowed_under_air_gap() {
        assert!(validate_adapter_host("127.0.0.1", ConnectivityState::AirGapped, &[]).is_ok());
        assert!(validate_adapter_host("::1", ConnectivityState::AirGapped, &[]).is_ok());
    }

    #[test]
    fn non_loopback_rejected_under_air_gap() {
        let err = validate_adapter_host("10.0.0.5", ConnectivityState::AirGapped, &[])
            .expect_err("non-loopback must be rejected under air-gap");
        assert!(matches!(
            err,
            HostValidationError::NonLoopbackUnderAirGap { .. }
        ));
    }

    #[test]
    fn non_loopback_rejected_under_expired() {
        // Expired state is local-only same as AirGapped.
        assert!(matches!(
            validate_adapter_host("10.0.0.5", ConnectivityState::Expired, &[]).unwrap_err(),
            HostValidationError::NonLoopbackUnderAirGap { .. }
        ));
    }

    #[test]
    fn wildcard_always_forbidden() {
        for state in [
            ConnectivityState::Connected,
            ConnectivityState::Degraded,
            ConnectivityState::StaleNotExpired,
            ConnectivityState::Expired,
            ConnectivityState::AirGapped,
        ] {
            let allow_all = vec!["0.0.0.0".to_string()];
            let err = validate_adapter_host("0.0.0.0", state, &allow_all).unwrap_err();
            assert!(matches!(err, HostValidationError::WildcardForbidden(_)));
        }
    }

    #[test]
    fn allow_list_consulted_only_when_connected() {
        // Allowlist hit succeeds when connected.
        let allow = vec!["10.0.0.5".to_string()];
        assert!(validate_adapter_host("10.0.0.5", ConnectivityState::Connected, &allow).is_ok());
        // Same host fails under air-gap even if allowlisted.
        let err =
            validate_adapter_host("10.0.0.5", ConnectivityState::AirGapped, &allow).unwrap_err();
        assert!(matches!(
            err,
            HostValidationError::NonLoopbackUnderAirGap { .. }
        ));
    }

    #[test]
    fn non_loopback_without_allow_list_fails_when_connected() {
        let err = validate_adapter_host("10.0.0.5", ConnectivityState::Connected, &[]).unwrap_err();
        assert!(matches!(err, HostValidationError::NotInAllowList(_)));
    }

    #[test]
    fn empty_host_rejected() {
        let err = validate_adapter_host("", ConnectivityState::Connected, &[]).unwrap_err();
        assert!(matches!(err, HostValidationError::Unparseable(_)));
    }

    #[test]
    fn framework_error_conversion_carries_reason() {
        let val_err =
            validate_adapter_host("8.8.8.8", ConnectivityState::AirGapped, &[]).unwrap_err();
        let fw_err: FrameworkError = val_err.into();
        match fw_err {
            FrameworkError::ConfigError { name, reason } => {
                assert_eq!(name, "adapter-host");
                assert!(
                    reason.contains("8.8.8.8"),
                    "reason should mention host: {reason}"
                );
            }
            _ => panic!("expected ConfigError variant"),
        }
    }
}
