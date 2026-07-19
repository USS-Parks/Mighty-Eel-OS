//! Approved provider destinations: exact-origin policy, address validation,
//! DNS pinning, and redirect confinement before provider credentials exist.

use std::collections::BTreeSet;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use reqwest::Url;

const PROVIDER_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PROVIDER_READ_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_LOCAL_ORIGINS: &[&str] = &[
    "http://127.0.0.1:8000",
    "http://[::1]:8000",
    "http://localhost:8000",
];
const DEFAULT_CLOUD_ORIGINS: &[&str] = &["https://api.openai.com", "https://api.anthropic.com"];

/// Whether a destination is local to the appliance or public cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointClass {
    Local,
    Cloud,
}

/// Exact-origin and address-class policy used to approve a provider endpoint.
#[derive(Debug, Clone)]
pub struct EndpointPolicy {
    class: EndpointClass,
    allowed_origins: BTreeSet<String>,
    allow_development_plaintext: bool,
}

impl EndpointPolicy {
    /// Local-provider policy. The stock loopback endpoint is approved; any
    /// additional origin must be explicitly listed by the operator.
    pub fn local(additional_origins: &str) -> Result<Self, EndpointPolicyError> {
        Self::new(
            EndpointClass::Local,
            DEFAULT_LOCAL_ORIGINS,
            additional_origins,
            false,
        )
    }

    /// Local-provider policy for an explicitly development-only isolated
    /// fixture network. Every non-loopback origin still requires an exact
    /// allowlist entry; production never selects this policy.
    pub fn development_local(additional_origins: &str) -> Result<Self, EndpointPolicyError> {
        Self::new(
            EndpointClass::Local,
            DEFAULT_LOCAL_ORIGINS,
            additional_origins,
            true,
        )
    }

    /// Credentialed cloud-provider policy. Only official origins are approved
    /// by default.
    pub fn cloud(additional_origins: &str) -> Result<Self, EndpointPolicyError> {
        Self::new(
            EndpointClass::Cloud,
            DEFAULT_CLOUD_ORIGINS,
            additional_origins,
            false,
        )
    }

    fn new(
        class: EndpointClass,
        defaults: &[&str],
        additional_origins: &str,
        allow_development_plaintext: bool,
    ) -> Result<Self, EndpointPolicyError> {
        let mut allowed_origins = BTreeSet::new();
        for raw in defaults
            .iter()
            .copied()
            .chain(additional_origins.split(',').map(str::trim))
            .filter(|raw| !raw.is_empty())
        {
            let url = parse_url(raw)?;
            if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
                return Err(error(format!(
                    "provider allowlist entry '{raw}' must contain only an origin"
                )));
            }
            if class == EndpointClass::Cloud && url.scheme() != "https" {
                return Err(error(format!(
                    "cloud provider allowlist origin '{}' must use https",
                    url.origin().ascii_serialization()
                )));
            }
            if url.scheme() == "http"
                && !url_host_is_literal_loopback(&url)
                && !allow_development_plaintext
            {
                return Err(error(format!(
                    "plaintext provider allowlist origin '{}' must be an explicit loopback",
                    url.origin().ascii_serialization()
                )));
            }
            allowed_origins.insert(url.origin().ascii_serialization());
        }
        Ok(Self {
            class,
            allowed_origins,
            allow_development_plaintext,
        })
    }

    #[cfg(test)]
    fn exact(class: EndpointClass, origin: &str) -> Self {
        Self {
            class,
            allowed_origins: BTreeSet::from([origin.to_string()]),
            allow_development_plaintext: false,
        }
    }
}

/// A parsed, policy-approved, DNS-pinned provider endpoint. Provider adapters
/// accept this type instead of raw URLs, preventing an unsafe construction path.
#[derive(Clone)]
pub struct ApprovedEndpoint {
    base_url: Url,
    client: reqwest::Client,
}

impl fmt::Debug for ApprovedEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApprovedEndpoint")
            .field("origin", &self.base_url.origin().ascii_serialization())
            .finish_non_exhaustive()
    }
}

impl ApprovedEndpoint {
    /// Resolve, validate, and pin every address before a provider adapter can be
    /// constructed or a credential attached.
    pub async fn resolve(
        base_url: &str,
        policy: &EndpointPolicy,
    ) -> Result<Self, EndpointPolicyError> {
        let url = parse_url(base_url)?;
        let origin = url.origin().ascii_serialization();
        if !policy.allowed_origins.contains(&origin) {
            return Err(error(format!(
                "provider origin '{origin}' is not in the {:?} allowlist",
                policy.class
            )));
        }

        let host = url
            .host_str()
            .ok_or_else(|| error("provider URL has no host"))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| error("provider URL has no known port"))?;
        let addresses = if let Some(ip) = parse_host_ip(host) {
            vec![SocketAddr::new(ip, port)]
        } else {
            let mut addresses: Vec<_> = tokio::net::lookup_host((host, port))
                .await
                .map_err(|e| error(format!("provider DNS resolution failed for '{host}': {e}")))?
                .collect();
            addresses.sort_unstable();
            addresses.dedup();
            addresses
        };
        Self::from_resolved(url, policy, addresses)
    }

    /// Explicit local-only constructor for deterministic test providers. It is
    /// incapable of approving a hostname, private-LAN address, or cloud origin.
    pub fn loopback_test(base_url: &str) -> Result<Self, EndpointPolicyError> {
        let url = parse_url(base_url)?;
        let host = url
            .host_str()
            .ok_or_else(|| error("test provider URL has no host"))?;
        let ip = parse_host_ip(host)
            .ok_or_else(|| error("test provider URL must use a literal loopback address"))?;
        if !ip.is_loopback() {
            return Err(error("test provider URL must use a loopback address"));
        }
        let port = url
            .port_or_known_default()
            .ok_or_else(|| error("test provider URL has no known port"))?;
        let policy = EndpointPolicy {
            class: EndpointClass::Local,
            allowed_origins: BTreeSet::from([url.origin().ascii_serialization()]),
            allow_development_plaintext: false,
        };
        Self::from_resolved(url, &policy, vec![SocketAddr::new(ip, port)])
    }

    fn from_resolved(
        url: Url,
        policy: &EndpointPolicy,
        addresses: Vec<SocketAddr>,
    ) -> Result<Self, EndpointPolicyError> {
        if addresses.is_empty() {
            return Err(error("provider DNS resolution returned no addresses"));
        }
        for address in &addresses {
            let allowed = match policy.class {
                EndpointClass::Local => is_local_provider_address(address.ip()),
                EndpointClass::Cloud => is_public_provider_address(address.ip()),
            };
            if !allowed {
                return Err(error(format!(
                    "provider origin '{}' resolved to forbidden {:?} address {}",
                    url.origin().ascii_serialization(),
                    policy.class,
                    address.ip()
                )));
            }
        }
        if url.scheme() != "https"
            && !(policy.class == EndpointClass::Local
                && (addresses.iter().all(|address| address.ip().is_loopback())
                    || policy.allow_development_plaintext))
        {
            return Err(error(format!(
                "provider origin '{}' must use https unless every pinned address is loopback",
                url.origin().ascii_serialization()
            )));
        }

        let host = url
            .host_str()
            .ok_or_else(|| error("provider URL has no host"))?;
        let mut builder = reqwest::Client::builder()
            .connect_timeout(PROVIDER_CONNECT_TIMEOUT)
            .read_timeout(PROVIDER_READ_TIMEOUT)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none());
        if parse_host_ip(host).is_none() {
            builder = builder.resolve_to_addrs(host, &addresses);
        }
        let client = builder
            .build()
            .map_err(|e| error(format!("provider HTTP client configuration failed: {e}")))?;
        Ok(Self {
            base_url: url,
            client,
        })
    }

    pub(crate) fn request_url(&self, suffix: &str) -> Url {
        let mut url = self.base_url.clone();
        let base_path = url.path().trim_end_matches('/');
        url.set_path(&format!("{base_path}/{suffix}"));
        url
    }

    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.client
    }
}

/// Provider endpoint policy failure. Credentials never enter this type.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct EndpointPolicyError(String);

fn error(message: impl Into<String>) -> EndpointPolicyError {
    EndpointPolicyError(message.into())
}

fn parse_url(raw: &str) -> Result<Url, EndpointPolicyError> {
    let url = Url::parse(raw).map_err(|e| error(format!("invalid provider URL '{raw}': {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(error(format!(
            "invalid provider URL scheme '{}': expected http or https",
            url.scheme()
        )));
    }
    if url.host_str().is_none() {
        return Err(error("provider URL has no host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(error("provider URL must not contain userinfo"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(error(
            "provider base URL must not contain a query or fragment",
        ));
    }
    Ok(url)
}

fn url_host_is_literal_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || parse_host_ip(host).is_some_and(|address| address.is_loopback())
    })
}

fn parse_host_ip(host: &str) -> Option<IpAddr> {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .ok()
}

fn is_local_provider_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => address.is_loopback() || address.is_private(),
        IpAddr::V6(address) => address.is_loopback() || address.is_unique_local(),
    }
}

fn is_public_provider_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_v4(address),
        IpAddr::V6(address) => is_public_v6(address),
    }
}

fn is_public_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(address.is_unspecified()
        || address.is_private()
        || address.is_loopback()
        || address.is_link_local()
        || address.is_broadcast()
        || address.is_documentation()
        || address.is_multicast()
        || a == 0
        || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && (b == 18 || b == 19))
        || a >= 240)
}

fn is_public_v6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_v4(mapped);
    }
    let segments = address.segments();
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || address.is_unique_local()
        || address.is_unicast_link_local()
        || (segments[0] == 0x2001 && segments[1] == 0x0db8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_local_origin_is_not_implicitly_trusted() {
        let policy = EndpointPolicy::local("").unwrap();
        let url = parse_url("https://api.example.test").unwrap();
        assert!(
            !policy
                .allowed_origins
                .contains(&url.origin().ascii_serialization())
        );
    }

    #[test]
    fn credentialed_cloud_allowlist_requires_https() {
        let err = EndpointPolicy::cloud("http://203.0.113.10:8080").unwrap_err();
        assert!(err.to_string().contains("must use https"));
    }

    #[test]
    fn private_plaintext_requires_the_development_only_policy() {
        assert!(EndpointPolicy::local("http://mock-llm:8000").is_err());
        assert!(EndpointPolicy::development_local("http://mock-llm:8000").is_ok());
    }

    #[test]
    fn dns_rebinding_answer_set_is_rejected_and_never_pinned() {
        let url = parse_url("https://provider.example.test").unwrap();
        let policy =
            EndpointPolicy::exact(EndpointClass::Cloud, &url.origin().ascii_serialization());
        let addresses = vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)), 443),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)), 443),
        ];
        let err = ApprovedEndpoint::from_resolved(url, &policy, addresses).unwrap_err();
        assert!(err.to_string().contains("forbidden Cloud address 10.0.0.5"));
    }

    #[test]
    fn metadata_and_link_local_destinations_are_rejected() {
        let url = parse_url("https://169.254.169.254").unwrap();
        let policy =
            EndpointPolicy::exact(EndpointClass::Local, &url.origin().ascii_serialization());
        let err = ApprovedEndpoint::from_resolved(
            url,
            &policy,
            vec![SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
                443,
            )],
        )
        .unwrap_err();
        assert!(err.to_string().contains("169.254.169.254"));
    }

    #[test]
    fn plaintext_is_confined_to_literal_loopback_test_provider() {
        assert!(ApprovedEndpoint::loopback_test("http://127.0.0.1:9000").is_ok());
        assert!(ApprovedEndpoint::loopback_test("http://10.0.0.8:9000").is_err());
    }
}
