//! Fail-closed startup and provider-destination checks for the AOG gateway.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use async_trait::async_trait;

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

/// Explicit provider-origin overrides. Origins are exact (`scheme://host:port`)
/// rather than suffix matches, so `trusted.example.attacker.test` cannot inherit
/// authority from `trusted.example`.
#[derive(Debug, Clone)]
pub struct ProviderEndpointPolicy {
    local_origins: HashSet<String>,
    private_origins: HashSet<String>,
    allow_insecure_development_fixtures: bool,
}

impl ProviderEndpointPolicy {
    /// Parse comma-separated exact-origin allowlists.
    pub fn new(
        local_origins: &str,
        private_origins: &str,
        allow_insecure_development_fixtures: bool,
    ) -> Result<Self, String> {
        Ok(Self {
            local_origins: parse_origins("local provider", local_origins)?,
            private_origins: parse_origins("private provider", private_origins)?,
            allow_insecure_development_fixtures,
        })
    }
}

/// A URL that passed scheme, origin, DNS/IP, locality, and allowlist policy.
/// Provider adapters require this capability instead of accepting raw URLs.
#[derive(Debug, Clone)]
pub struct ApprovedProviderEndpoint {
    url: reqwest::Url,
    canonical_origin: String,
    dns_host: Option<String>,
    resolved_addrs: Vec<SocketAddr>,
}

impl ApprovedProviderEndpoint {
    /// Create an explicitly loopback-only HTTP fixture endpoint. This is the
    /// narrow constructor used by integration tests; it cannot approve a DNS
    /// name, private LAN address, metadata endpoint, or public destination.
    pub fn loopback_fixture(base_url: &str) -> Result<Self, String> {
        let url = parse_base_url("loopback fixture", base_url)?;
        let host = url
            .host_str()
            .ok_or_else(|| "loopback fixture URL has no host".to_string())?;
        let ip = host
            .parse::<IpAddr>()
            .map_err(|_| "loopback fixture must use an IP literal".to_string())?;
        if !ip.is_loopback() {
            return Err("loopback fixture must resolve to loopback".to_string());
        }
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err("loopback fixture must use http or https".to_string());
        }
        let port = effective_port(&url)?;
        Ok(Self {
            canonical_origin: canonical_origin(&url),
            url,
            dns_host: None,
            resolved_addrs: vec![SocketAddr::new(ip, port)],
        })
    }

    pub(crate) fn request_url(&self, suffix: &str) -> reqwest::Url {
        reqwest::Url::parse(&format!(
            "{}/{}",
            self.url.as_str().trim_end_matches('/'),
            suffix.trim_start_matches('/')
        ))
        .expect("approved provider base plus static API suffix is a valid URL")
    }

    pub(crate) fn dns_host(&self) -> Option<&str> {
        self.dns_host.as_deref()
    }

    pub(crate) fn resolved_addrs(&self) -> &[SocketAddr] {
        &self.resolved_addrs
    }

    /// Canonical approved origin, useful for evidence and diagnostics.
    #[must_use]
    pub fn canonical_origin(&self) -> &str {
        &self.canonical_origin
    }
}

#[async_trait]
trait EndpointResolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String>;
}

struct SystemResolver;

#[async_trait]
impl EndpointResolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
        tokio::net::lookup_host((host, port))
            .await
            .map(|iter| iter.collect())
            .map_err(|error| format!("DNS resolution failed for {host}:{port}: {error}"))
    }
}

/// Validate privileged revocation and provider egress before any listener bind,
/// credential attachment, or OpenBao interaction. DNS names are resolved once,
/// every answer is checked, and the approved addresses are carried into the
/// provider HTTP client for connection pinning against rebinding.
pub async fn enforce_startup_posture(
    profile: Profile,
    revocation_path: &str,
    providers: &[ProviderEndpoint<'_>],
    policy: &ProviderEndpointPolicy,
) -> Result<Vec<ApprovedProviderEndpoint>, String> {
    enforce_with_resolver(profile, revocation_path, providers, policy, &SystemResolver).await
}

async fn enforce_with_resolver(
    profile: Profile,
    revocation_path: &str,
    providers: &[ProviderEndpoint<'_>],
    policy: &ProviderEndpointPolicy,
    resolver: &dyn EndpointResolver,
) -> Result<Vec<ApprovedProviderEndpoint>, String> {
    if profile == Profile::Production && revocation_path.trim().is_empty() {
        return Err(
            "refusing production startup: AOG_REVOCATION_PATH must configure the mandatory \
             revocation snapshot source"
                .to_string(),
        );
    }

    let mut approved = Vec::with_capacity(providers.len());
    for provider in providers {
        let url = parse_base_url(provider.name, provider.base_url)?;
        let origin = canonical_origin(&url);
        let host = url
            .host_str()
            .ok_or_else(|| format!("{} provider URL has no host", provider.name))?
            .to_ascii_lowercase();
        let port = effective_port(&url)?;
        let literal = host.parse::<IpAddr>().ok();
        let mut addresses = if let Some(ip) = literal {
            vec![SocketAddr::new(ip, port)]
        } else {
            resolver.resolve(&host, port).await?
        };
        addresses.sort_unstable();
        addresses.dedup();
        if addresses.is_empty() {
            return Err(format!(
                "refusing {} provider '{}': DNS returned no addresses",
                provider.name, origin
            ));
        }

        let local_approved = policy.local_origins.contains(&origin);
        let private_approved = policy.private_origins.contains(&origin);
        if provider.local && !local_approved {
            return Err(format!(
                "refusing local provider origin '{origin}': add the exact origin to \
                 AOG_LOCAL_ALLOWED_ORIGINS"
            ));
        }
        if provider.credentialed && url.scheme() != "https" {
            let loopback_fixture = profile == Profile::Development
                && policy.allow_insecure_development_fixtures
                && addresses.iter().all(|addr| addr.ip().is_loopback());
            if !loopback_fixture {
                return Err(format!(
                    "refusing credentialed {} provider: HTTPS is required",
                    provider.name
                ));
            }
        }
        if url.scheme() == "http" {
            let loopback = addresses.iter().all(|addr| addr.ip().is_loopback());
            let explicit_dev_fixture = profile == Profile::Development
                && policy.allow_insecure_development_fixtures
                && local_approved;
            if !(loopback || explicit_dev_fixture) {
                return Err(format!(
                    "refusing {} provider '{}': plaintext HTTP is limited to loopback or an \
                     explicitly allowlisted development fixture",
                    provider.name, origin
                ));
            }
        }

        for address in &addresses {
            let ip = address.ip();
            if is_never_provider_destination(ip) {
                return Err(format!(
                    "refusing {} provider '{}': address {ip} is metadata, link-local, \
                     unspecified, or multicast",
                    provider.name, origin
                ));
            }
            if is_private_destination(ip) && !(local_approved || private_approved) {
                return Err(format!(
                    "refusing {} provider '{}': private address {ip} is not explicitly approved",
                    provider.name, origin
                ));
            }
        }

        approved.push(ApprovedProviderEndpoint {
            url,
            canonical_origin: origin,
            dns_host: literal.is_none().then_some(host),
            resolved_addrs: addresses,
        });
    }
    Ok(approved)
}

fn parse_origins(label: &str, value: &str) -> Result<HashSet<String>, String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let url = parse_base_url(label, entry)?;
            if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
                return Err(format!(
                    "{label} allowlist entry must be an origin: '{entry}'"
                ));
            }
            Ok(canonical_origin(&url))
        })
        .collect()
}

fn parse_base_url(name: &str, base_url: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(base_url)
        .map_err(|error| format!("invalid {name} provider base URL '{base_url}': {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "invalid {name} provider URL scheme '{}': expected http or https",
            url.scheme()
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!(
            "invalid {name} provider URL: embedded credentials are forbidden"
        ));
    }
    if url.host_str().is_none() {
        return Err(format!("invalid {name} provider URL: host is required"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(format!(
            "invalid {name} provider URL: query and fragment components are forbidden"
        ));
    }
    Ok(url)
}

fn effective_port(url: &reqwest::Url) -> Result<u16, String> {
    url.port_or_known_default()
        .ok_or_else(|| format!("provider URL '{}' has no effective port", url))
}

fn canonical_origin(url: &reqwest::Url) -> String {
    url.origin().ascii_serialization()
}

fn is_never_provider_destination(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() || ip.is_link_local()
        }
        IpAddr::V6(ip) => {
            ip.to_ipv4_mapped().is_some_and(|ip| {
                ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() || ip.is_link_local()
            }) || ip.is_unspecified()
                || ip.is_multicast()
                || is_ipv6_unicast_link_local(ip)
        }
    }
}

fn is_private_destination(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || is_shared_ipv4(ip),
        IpAddr::V6(ip) => {
            ip.to_ipv4_mapped()
                .is_some_and(|ip| ip.is_private() || ip.is_loopback() || is_shared_ipv4(ip))
                || ip.is_loopback()
                || is_unique_local_ipv6(ip)
        }
    }
}

fn is_shared_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

fn is_unique_local_ipv6(ip: Ipv6Addr) -> bool {
    ip.octets()[0] & 0xfe == 0xfc
}

fn is_ipv6_unicast_link_local(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    first & 0xffc0 == 0xfe80
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    struct StaticResolver(HashMap<String, Vec<IpAddr>>);

    #[async_trait]
    impl EndpointResolver for StaticResolver {
        async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
            self.0
                .get(host)
                .cloned()
                .ok_or_else(|| format!("no fixture answer for {host}"))
                .map(|ips| {
                    ips.into_iter()
                        .map(|ip| SocketAddr::new(ip, port))
                        .collect()
                })
        }
    }

    fn endpoint<'a>(
        name: &'a str,
        base_url: &'a str,
        credentialed: bool,
        local: bool,
    ) -> ProviderEndpoint<'a> {
        ProviderEndpoint {
            name,
            base_url,
            credentialed,
            local,
        }
    }

    fn policy(local: &str, private: &str, insecure_dev: bool) -> ProviderEndpointPolicy {
        ProviderEndpointPolicy::new(local, private, insecure_dev).unwrap()
    }

    fn resolver(entries: &[(&str, &[IpAddr])]) -> StaticResolver {
        StaticResolver(
            entries
                .iter()
                .map(|(host, ips)| ((*host).to_string(), ips.to_vec()))
                .collect(),
        )
    }

    #[tokio::test]
    async fn production_requires_revocation_source() {
        let err = enforce_with_resolver(
            Profile::Production,
            "",
            &[],
            &policy("", "", false),
            &resolver(&[]),
        )
        .await
        .unwrap_err();
        assert!(err.contains("AOG_REVOCATION_PATH"));
    }

    #[tokio::test]
    async fn http_metadata_arbitrary_local_and_mixed_dns_fail_before_dispatch() {
        let public = "203.0.113.10".parse().unwrap();
        let private = "10.10.0.8".parse().unwrap();
        let answers = resolver(&[
            ("api.example.test", &[public]),
            ("rebind.example.test", &[public, private]),
        ]);
        let cfg = policy("http://127.0.0.1:8000", "", false);

        for candidate in [
            endpoint("openai", "http://api.example.test", true, false),
            endpoint(
                "openai",
                "https://169.254.169.254/latest/meta-data",
                true,
                false,
            ),
            endpoint("local", "https://10.10.0.8:8443", false, true),
            endpoint("openai", "https://rebind.example.test", true, false),
        ] {
            assert!(
                enforce_with_resolver(
                    Profile::Production,
                    "kv/data/aog/revocation",
                    &[candidate],
                    &cfg,
                    &answers,
                )
                .await
                .is_err(),
                "accepted {}",
                candidate.base_url
            );
        }
    }

    #[tokio::test]
    async fn approved_public_and_exact_private_origins_are_pinned() {
        let public = "203.0.113.10".parse().unwrap();
        let private = "10.10.0.8".parse().unwrap();
        let answers = resolver(&[("api.example.test", &[public]), ("model.lan", &[private])]);
        let cfg = policy("https://model.lan:8443", "https://model.lan:8443", false);
        let approved = enforce_with_resolver(
            Profile::Production,
            "kv/data/aog/revocation",
            &[
                endpoint("openai", "https://api.example.test", true, false),
                endpoint("local", "https://model.lan:8443", false, true),
            ],
            &cfg,
            &answers,
        )
        .await
        .unwrap();
        assert_eq!(approved[0].canonical_origin(), "https://api.example.test");
        assert_eq!(
            approved[0].resolved_addrs(),
            &[SocketAddr::new(public, 443)]
        );
        assert_eq!(
            approved[1].resolved_addrs(),
            &[SocketAddr::new(private, 8443)]
        );
    }

    #[tokio::test]
    async fn insecure_non_loopback_fixture_requires_explicit_development_override() {
        let private = "10.20.0.5".parse().unwrap();
        let answers = resolver(&[("mock-llm", &[private])]);
        let provider = endpoint("local", "http://mock-llm:8000", false, true);
        assert!(
            enforce_with_resolver(
                Profile::Development,
                "",
                &[provider],
                &policy("http://mock-llm:8000", "", false),
                &answers,
            )
            .await
            .is_err()
        );
        assert!(
            enforce_with_resolver(
                Profile::Development,
                "",
                &[provider],
                &policy("http://mock-llm:8000", "", true),
                &answers,
            )
            .await
            .is_ok()
        );
    }

    #[test]
    fn loopback_fixture_rejects_non_loopback_and_dns_names() {
        assert!(ApprovedProviderEndpoint::loopback_fixture("http://127.0.0.1:9000").is_ok());
        assert!(ApprovedProviderEndpoint::loopback_fixture("http://10.0.0.1:9000").is_err());
        assert!(ApprovedProviderEndpoint::loopback_fixture("http://localhost:9000").is_err());
    }
}
