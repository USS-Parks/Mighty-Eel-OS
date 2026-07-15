//! Fail-closed startup checks for the AOG gateway binary.

use crate::policy::Profile;

/// A configured provider endpoint that must be validated before credentials or
/// request bodies can be sent to it.
#[derive(Debug, Clone, Copy)]
pub struct ProviderEndpoint<'a> {
    pub name: &'a str,
    pub base_url: &'a str,
    pub credentialed: bool,
    pub local: bool,
}

/// Validate privileged revocation and provider egress before any listener bind
/// or OpenBao interaction. Development is explicit and may use isolated HTTP
/// fixtures. Production requires mandatory revocation, HTTPS for credentialed
/// providers, and confines a plaintext local backend to the host loopback.
pub fn enforce_startup_posture(
    profile: Profile,
    revocation_path: &str,
    providers: &[ProviderEndpoint<'_>],
) -> Result<(), String> {
    if profile == Profile::Production && revocation_path.trim().is_empty() {
        return Err(
            "refusing production startup: AOG_REVOCATION_PATH must configure the mandatory \
             revocation snapshot source"
                .to_string(),
        );
    }

    for provider in providers {
        let url = reqwest::Url::parse(provider.base_url).map_err(|e| {
            format!(
                "invalid {} provider base URL '{}': {e}",
                provider.name, provider.base_url
            )
        })?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!(
                "invalid {} provider URL scheme '{}': expected http or https",
                provider.name,
                url.scheme()
            ));
        }
        if profile != Profile::Production {
            continue;
        }
        if provider.credentialed && url.scheme() != "https" {
            return Err(format!(
                "refusing production startup: credentialed {} provider must use https",
                provider.name
            ));
        }
        if provider.local && url.scheme() == "http" && !is_loopback_host(&url) {
            return Err(format!(
                "refusing production startup: plaintext local provider '{}' is not loopback; \
                 use https or an explicit development profile",
                provider.base_url
            ));
        }
    }
    Ok(())
}

fn is_loopback_host(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint<'a>(base_url: &'a str, credentialed: bool, local: bool) -> ProviderEndpoint<'a> {
        ProviderEndpoint {
            name: if local { "local" } else { "cloud" },
            base_url,
            credentialed,
            local,
        }
    }

    #[test]
    fn production_requires_revocation_source() {
        let err = enforce_startup_posture(Profile::Production, "", &[]).unwrap_err();
        assert!(err.contains("AOG_REVOCATION_PATH"));
    }

    #[test]
    fn production_rejects_credentialed_http_provider() {
        let err = enforce_startup_posture(
            Profile::Production,
            "kv/data/aog/revocation",
            &[endpoint("http://api.example.test", true, false)],
        )
        .unwrap_err();
        assert!(err.contains("must use https"), "reason: {err}");
    }

    #[test]
    fn production_confines_plaintext_local_provider_to_loopback() {
        assert!(
            enforce_startup_posture(
                Profile::Production,
                "kv/data/aog/revocation",
                &[endpoint("http://127.0.0.1:8000", false, true)],
            )
            .is_ok()
        );
        assert!(
            enforce_startup_posture(
                Profile::Production,
                "kv/data/aog/revocation",
                &[endpoint("http://mock-llm:8000", false, true)],
            )
            .is_err()
        );
    }

    #[test]
    fn explicit_development_allows_isolated_http_fixtures() {
        assert!(
            enforce_startup_posture(
                Profile::Development,
                "",
                &[
                    endpoint("http://mock-llm:8000", false, true),
                    endpoint("http://cloud-fixture:9000", true, false),
                ],
            )
            .is_ok()
        );
    }

    #[test]
    fn unsupported_and_malformed_urls_always_fail() {
        for base in ["not a URL", "file:///tmp/model"] {
            assert!(
                enforce_startup_posture(Profile::Development, "", &[endpoint(base, false, true)])
                    .is_err(),
                "accepted {base}"
            );
        }
    }
}
