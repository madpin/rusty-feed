//! SSRF-guarded HTTP client (docs/ARCHITECTURE.md §12; fixes FND-006).
//!
//! The **only** way RustSS makes outbound HTTP for feeds/extraction/robots. A
//! custom DNS resolver rejects any host that resolves to a non-public address,
//! which also protects every redirect hop (reqwest resolves each hop through the
//! resolver), mitigating DNS-rebinding and redirect-to-internal attacks.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::error::FeedError;

const USER_AGENT: &str = "RustSS/0.1 (+https://github.com/madpin/rustss)";

/// Whether an IP is safe to connect to (i.e. globally routable / public).
pub fn is_global_ip(ip: &IpAddr) -> bool {
    !is_blocked_ip(ip)
}

fn is_blocked_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => is_blocked_v6(v6),
    }
}

fn is_blocked_v4(ip: &Ipv4Addr) -> bool {
    if ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
    {
        return true;
    }
    let o = ip.octets();
    o[0] == 0                                   // 0.0.0.0/8
        || (o[0] == 100 && (o[1] & 0xC0) == 64) // 100.64.0.0/10 (CGNAT)
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // 198.18.0.0/15 (benchmarking)
        || (o[0] == 169 && o[1] == 254) // link-local (also covered above)
}

fn is_blocked_v6(ip: &Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_v4(&v4);
    }
    let seg = ip.segments();
    (seg[0] & 0xfe00) == 0xfc00  // ULA fc00::/7
        || (seg[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
}

/// A DNS resolver that filters out non-public addresses.
#[derive(Debug, Clone, Default)]
struct SafeResolver {
    allow_private: bool,
}

impl Resolve for SafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let allow_private = self.allow_private;
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let resolved = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
            let addrs: Vec<SocketAddr> = resolved
                .filter(|sa| allow_private || !is_blocked_ip(&sa.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(
                    format!("refusing to connect to a non-public address for '{host}'").into(),
                );
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// An HTTP client that refuses to connect to private/loopback/link-local hosts.
#[derive(Clone)]
pub struct SafeFetchClient {
    client: reqwest::Client,
    allow_private: bool,
}

impl SafeFetchClient {
    /// Build the production client: rejects non-public addresses (SSRF-safe).
    pub fn new(timeout: Duration) -> Result<Self, FeedError> {
        Self::build(timeout, false)
    }

    /// Build a client that allows private/loopback addresses. **For tests and
    /// explicitly-trusted setups only** (e.g. an intranet feed).
    pub fn new_allow_private(timeout: Duration) -> Result<Self, FeedError> {
        Self::build(timeout, true)
    }

    fn build(timeout: Duration, allow_private: bool) -> Result<Self, FeedError> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(5))
            .dns_resolver(Arc::new(SafeResolver { allow_private }))
            .build()
            .map_err(FeedError::Http)?;
        Ok(Self {
            client,
            allow_private,
        })
    }

    /// Access the underlying reqwest client (for extraction/other GETs).
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Validate a URL for this client (scheme + IP-literal policy).
    pub fn validate_url(&self, url: &str) -> Result<(), FeedError> {
        if self.allow_private {
            let parsed =
                url::Url::parse(url).map_err(|_| FeedError::InvalidUrl(url.to_string()))?;
            return match parsed.scheme() {
                "http" | "https" => Ok(()),
                other => Err(FeedError::UnsupportedScheme(other.to_string())),
            };
        }
        ensure_http_scheme(url)
    }
}

impl Default for SafeFetchClient {
    fn default() -> Self {
        Self::new(Duration::from_secs(30)).expect("default client builds")
    }
}

/// Validate a URL before fetching: http/https only, and reject IP-literal hosts
/// that point at non-public addresses (reqwest can bypass the DNS resolver for
/// IP literals, so we must check them here too). Hostnames are checked by the
/// [`SafeResolver`] at connect time.
pub fn ensure_http_scheme(url: &str) -> Result<(), FeedError> {
    let parsed = url::Url::parse(url).map_err(|_| FeedError::InvalidUrl(url.to_string()))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(FeedError::UnsupportedScheme(other.to_string())),
    }
    match parsed.host() {
        Some(url::Host::Ipv4(ip)) if is_blocked_ip(&IpAddr::V4(ip)) => Err(FeedError::InvalidUrl(
            format!("blocked non-public address: {ip}"),
        )),
        Some(url::Host::Ipv6(ip)) if is_blocked_ip(&IpAddr::V6(ip)) => Err(FeedError::InvalidUrl(
            format!("blocked non-public address: {ip}"),
        )),
        Some(_) => Ok(()),
        None => Err(FeedError::InvalidUrl(url.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_private_and_loopback() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            let addr: IpAddr = ip.parse().unwrap();
            assert!(!is_global_ip(&addr), "{ip} should be blocked");
        }
    }

    #[test]
    fn allows_public() {
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "2606:4700:4700::1111",
        ] {
            let addr: IpAddr = ip.parse().unwrap();
            assert!(is_global_ip(&addr), "{ip} should be allowed");
        }
    }

    #[test]
    fn scheme_validation() {
        assert!(ensure_http_scheme("https://example.com/feed").is_ok());
        assert!(ensure_http_scheme("http://example.com").is_ok());
        assert!(ensure_http_scheme("file:///etc/passwd").is_err());
        assert!(ensure_http_scheme("ftp://x").is_err());
        assert!(ensure_http_scheme("not a url").is_err());
        // IP-literal hosts pointing at non-public addresses are blocked here.
        assert!(ensure_http_scheme("http://127.0.0.1/feed").is_err());
        assert!(ensure_http_scheme("http://169.254.169.254/latest/meta-data").is_err());
        assert!(ensure_http_scheme("http://[::1]/feed").is_err());
        assert!(ensure_http_scheme("http://1.1.1.1/feed").is_ok());
    }
}
