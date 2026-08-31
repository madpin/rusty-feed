//! SSRF-guarded HTTP client (docs/ARCHITECTURE.md §12; fixes FND-006).
//!
//! The **only** way RustSS makes outbound HTTP for feeds/extraction/robots. A
//! custom DNS resolver rejects any host that resolves to a non-public address,
//! which also protects every redirect hop (reqwest resolves each hop through the
//! resolver), mitigating DNS-rebinding and redirect-to-internal attacks.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{HeaderMap, HeaderValue, COOKIE};

use crate::error::FeedError;

pub const DEFAULT_USER_AGENT: &str = "RustSS/0.1 (+https://github.com/madpin/rustss)";

/// Validated construction policy for the one outbound publisher client.
#[derive(Debug, Clone)]
pub struct SafeFetchOptions {
    pub timeout: Duration,
    pub allow_private: bool,
    pub user_agent: String,
    pub http1_only: bool,
}

impl SafeFetchOptions {
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            allow_private: false,
            user_agent: DEFAULT_USER_AGENT.to_string(),
            http1_only: false,
        }
    }
}

/// A bounded response fetched through the SSRF-guarded client.
#[derive(Debug, Clone)]
pub struct FetchedBytes {
    pub body: Vec<u8>,
    pub content_type: Option<String>,
    pub final_url: String,
}

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
    allowed_hosts: Vec<String>,
    private_hosts: Vec<String>,
}

impl Resolve for SafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let allow_private = self.allow_private;
        let allowed_hosts = self.allowed_hosts.clone();
        let private_hosts = self.private_hosts.clone();
        Box::pin(async move {
            let host = name.as_str().to_owned();
            if !allowed_hosts.is_empty()
                && !allowed_hosts
                    .iter()
                    .any(|allowed| host_matches(&host, allowed))
            {
                return Err(format!("host '{host}' is outside the acquisition profile").into());
            }
            let resolved = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
            let addrs: Vec<SocketAddr> = resolved
                .filter(|sa| {
                    allow_private
                        || private_hosts
                            .iter()
                            .any(|allowed| host_matches(&host, allowed))
                        || !is_blocked_ip(&sa.ip())
                })
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
    no_redirect_client: reqwest::Client,
    allow_private: bool,
    user_agent: String,
    http1_only: bool,
    timeout: Duration,
    allowed_hosts: Vec<String>,
    allowed_path_prefix: Option<String>,
    uses_proxy: bool,
}

/// Decrypted administrator-owned acquisition boundary for one shared source.
#[derive(Debug, Clone, Default)]
pub struct AcquisitionPolicy {
    pub allowed_hosts: Vec<String>,
    pub allowed_path_prefix: Option<String>,
    pub cookie_header: Option<String>,
    pub proxy_url: Option<String>,
    pub allow_invalid_tls: bool,
}

impl SafeFetchClient {
    /// Build the production client: rejects non-public addresses (SSRF-safe).
    pub fn new(timeout: Duration) -> Result<Self, FeedError> {
        Self::with_options(SafeFetchOptions::new(timeout))
    }

    /// Build a client that allows private/loopback addresses. **For tests and
    /// explicitly-trusted setups only** (e.g. an intranet feed).
    pub fn new_allow_private(timeout: Duration) -> Result<Self, FeedError> {
        let mut options = SafeFetchOptions::new(timeout);
        options.allow_private = true;
        Self::with_options(options)
    }

    /// Build a client with an explicit instance acquisition policy.
    pub fn with_options(options: SafeFetchOptions) -> Result<Self, FeedError> {
        let SafeFetchOptions {
            timeout,
            allow_private,
            user_agent,
            http1_only,
        } = options;
        Self::build(
            timeout,
            allow_private,
            user_agent,
            http1_only,
            AcquisitionPolicy::default(),
        )
    }

    fn build(
        timeout: Duration,
        allow_private: bool,
        user_agent: String,
        http1_only: bool,
        acquisition: AcquisitionPolicy,
    ) -> Result<Self, FeedError> {
        validate_acquisition_policy(&acquisition)?;
        let proxy_host = acquisition
            .proxy_url
            .as_deref()
            .and_then(|value| url::Url::parse(value).ok())
            .and_then(|value| value.host_str().map(str::to_string));
        let mut resolver_hosts = acquisition.allowed_hosts.clone();
        if let Some(host) = proxy_host.as_ref() {
            if !resolver_hosts
                .iter()
                .any(|allowed| host_matches(host, allowed))
            {
                resolver_hosts.push(host.clone());
            }
        }
        let resolver = Arc::new(SafeResolver {
            allow_private,
            allowed_hosts: resolver_hosts,
            private_hosts: proxy_host.into_iter().collect(),
        });
        let redirect_hosts = acquisition.allowed_hosts.clone();
        let redirect_path = acquisition.allowed_path_prefix.clone();
        let forbid_redirects = acquisition.proxy_url.is_some();
        let mut client_builder = reqwest::Client::builder()
            .user_agent(&user_agent)
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= 5
                    || forbid_redirects
                    || !allows_redirect(attempt.url(), allow_private)
                    || (!redirect_hosts.is_empty()
                        && !attempt.url().host_str().is_some_and(|host| {
                            redirect_hosts
                                .iter()
                                .any(|allowed| host_matches(host, allowed))
                        }))
                    || redirect_path
                        .as_deref()
                        .is_some_and(|prefix| !attempt.url().path().starts_with(prefix))
                {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .no_proxy()
            .dns_resolver(resolver.clone());
        let mut no_redirect_builder = reqwest::Client::builder()
            .user_agent(&user_agent)
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .dns_resolver(resolver);
        if let Some(cookie) = acquisition.cookie_header.as_deref() {
            let mut headers = HeaderMap::new();
            headers.insert(
                COOKIE,
                HeaderValue::from_str(cookie)
                    .map_err(|_| FeedError::InvalidUrl("invalid acquisition cookie".into()))?,
            );
            client_builder = client_builder.default_headers(headers.clone());
            no_redirect_builder = no_redirect_builder.default_headers(headers);
        }
        if let Some(proxy_url) = acquisition.proxy_url.as_deref() {
            let proxy = reqwest::Proxy::all(proxy_url).map_err(FeedError::Http)?;
            client_builder = client_builder.proxy(proxy.clone());
            no_redirect_builder = no_redirect_builder.proxy(proxy);
        }
        if acquisition.allow_invalid_tls {
            client_builder = client_builder.danger_accept_invalid_certs(true);
            no_redirect_builder = no_redirect_builder.danger_accept_invalid_certs(true);
        }
        if http1_only {
            client_builder = client_builder.http1_only();
        }
        let client = client_builder.build().map_err(FeedError::Http)?;
        if http1_only {
            no_redirect_builder = no_redirect_builder.http1_only();
        }
        let no_redirect_client = no_redirect_builder.build().map_err(FeedError::Http)?;
        Ok(Self {
            client,
            no_redirect_client,
            allow_private,
            user_agent,
            http1_only,
            timeout,
            allowed_hosts: acquisition.allowed_hosts,
            allowed_path_prefix: acquisition.allowed_path_prefix,
            uses_proxy: acquisition.proxy_url.is_some(),
        })
    }

    /// Derive a source-pinned client without weakening the shared default client.
    pub fn for_acquisition(&self, policy: AcquisitionPolicy) -> Result<Self, FeedError> {
        Self::build(
            self.timeout,
            self.allow_private,
            self.user_agent.clone(),
            self.http1_only,
            policy,
        )
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    pub const fn http1_only(&self) -> bool {
        self.http1_only
    }

    /// Access the underlying reqwest client (for extraction/other GETs).
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn client_without_redirects(&self) -> &reqwest::Client {
        &self.no_redirect_client
    }

    pub async fn get(&self, url: &str) -> Result<reqwest::RequestBuilder, FeedError> {
        self.validate_url_for_request(url).await?;
        Ok(self.client.get(url))
    }

    pub async fn get_without_redirects(
        &self,
        url: &str,
    ) -> Result<reqwest::RequestBuilder, FeedError> {
        self.validate_url_for_request(url).await?;
        Ok(self.no_redirect_client.get(url))
    }

    /// Validate a URL for this client (scheme + IP-literal policy).
    pub fn validate_url(&self, url: &str) -> Result<(), FeedError> {
        let parsed = url::Url::parse(url).map_err(|_| FeedError::InvalidUrl(url.to_string()))?;
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(FeedError::InvalidUrl(
                "URL credentials are not allowed".to_string(),
            ));
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| FeedError::InvalidUrl("URL has no host".into()))?;
        if !self.allowed_hosts.is_empty()
            && !self
                .allowed_hosts
                .iter()
                .any(|allowed| host_matches(host, allowed))
        {
            return Err(FeedError::InvalidUrl(
                "URL is outside the acquisition profile host boundary".into(),
            ));
        }
        if self
            .allowed_path_prefix
            .as_deref()
            .is_some_and(|prefix| !parsed.path().starts_with(prefix))
        {
            return Err(FeedError::InvalidUrl(
                "URL is outside the acquisition profile path boundary".into(),
            ));
        }
        if self.allow_private {
            return match parsed.scheme() {
                "http" | "https" => Ok(()),
                other => Err(FeedError::UnsupportedScheme(other.to_string())),
            };
        }
        ensure_http_scheme(url)
    }

    /// Re-resolve the destination immediately before a proxied request so a
    /// proxy cannot turn a public hostname into an internal SSRF hop.
    pub async fn validate_url_for_request(&self, url: &str) -> Result<(), FeedError> {
        self.validate_url(url)?;
        if !self.uses_proxy || self.allow_private {
            return Ok(());
        }
        let parsed = url::Url::parse(url).map_err(|_| FeedError::InvalidUrl(url.into()))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| FeedError::InvalidUrl("URL has no host".into()))?;
        let port = parsed.port_or_known_default().unwrap_or(443);
        let addresses = tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| FeedError::InvalidUrl(format!("DNS lookup failed: {error}")))?;
        if addresses
            .into_iter()
            .any(|address| !is_global_ip(&address.ip()))
        {
            return Err(FeedError::InvalidUrl(
                "proxied destination resolved to a non-public address".into(),
            ));
        }
        Ok(())
    }

    /// Fetch a public resource into memory with an explicit byte ceiling.
    ///
    /// This is suitable for small import/configuration documents such as OPML;
    /// callers choose a limit appropriate to their format.
    pub async fn get_bytes(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, FeedError> {
        Ok(self.get_bytes_with_metadata(url, max_bytes).await?.body)
    }

    /// Fetch a public resource into memory and retain safe response metadata.
    ///
    /// The final URL has passed the same redirect and DNS policy as the body.
    pub async fn get_bytes_with_metadata(
        &self,
        url: &str,
        max_bytes: usize,
    ) -> Result<FetchedBytes, FeedError> {
        let response = self.get(url).await?.send().await.map_err(FeedError::Http)?;
        if !response.status().is_success() {
            return Err(FeedError::Status(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            return Err(FeedError::TooLarge);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let final_url = response.url().to_string();
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(FeedError::Http)?;
            if body.len().saturating_add(chunk.len()) > max_bytes {
                return Err(FeedError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(FetchedBytes {
            body,
            content_type,
            final_url,
        })
    }
}

fn host_matches(host: &str, allowed: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let allowed = allowed.trim_end_matches('.').to_ascii_lowercase();
    host == allowed || host.ends_with(&format!(".{allowed}"))
}

fn validate_acquisition_policy(policy: &AcquisitionPolicy) -> Result<(), FeedError> {
    if policy.allowed_hosts.is_empty()
        && (policy.cookie_header.is_some()
            || policy.proxy_url.is_some()
            || policy.allow_invalid_tls)
    {
        return Err(FeedError::InvalidUrl(
            "sensitive acquisition profiles require an explicit host boundary".into(),
        ));
    }
    if policy.allowed_hosts.len() > 32
        || policy.allowed_hosts.iter().any(|host| {
            host.is_empty()
                || host.len() > 253
                || host.contains('/')
                || host.contains(':')
                || host.chars().any(char::is_whitespace)
        })
    {
        return Err(FeedError::InvalidUrl(
            "invalid acquisition profile host boundary".into(),
        ));
    }
    if policy
        .allowed_path_prefix
        .as_deref()
        .is_some_and(|path| !path.starts_with('/') || path.len() > 512 || path.contains(".."))
    {
        return Err(FeedError::InvalidUrl(
            "invalid acquisition profile path boundary".into(),
        ));
    }
    if let Some(cookie) = policy.cookie_header.as_deref() {
        if cookie.is_empty()
            || cookie.len() > 4_096
            || cookie.contains(['\r', '\n'])
            || !cookie.contains('=')
        {
            return Err(FeedError::InvalidUrl(
                "invalid acquisition cookie header".into(),
            ));
        }
    }
    if let Some(proxy_url) = policy.proxy_url.as_deref() {
        let proxy = url::Url::parse(proxy_url)
            .map_err(|_| FeedError::InvalidUrl("invalid outbound proxy URL".into()))?;
        if !matches!(proxy.scheme(), "http" | "https" | "socks5" | "socks5h")
            || proxy.host_str().is_none()
            || (proxy.path() != "" && proxy.path() != "/")
            || proxy.query().is_some()
            || proxy.fragment().is_some()
        {
            return Err(FeedError::InvalidUrl(
                "outbound proxy must be a host-only http(s) or socks5 URL".into(),
            ));
        }
    }
    Ok(())
}

fn allows_redirect(url: &url::Url, allow_private: bool) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    allow_private || ensure_http_scheme(url.as_str()).is_ok()
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
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(FeedError::InvalidUrl(
            "URL credentials are not allowed".to_string(),
        ));
    }
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
        assert!(ensure_http_scheme("https://user:pass@example.com/feed").is_err());
        assert!(ensure_http_scheme("http://1.1.1.1/feed").is_ok());
    }

    #[test]
    fn redirect_policy_rejects_private_literals_and_credentials() {
        assert!(!allows_redirect(
            &url::Url::parse("http://127.0.0.1/private").unwrap(),
            false
        ));
        assert!(!allows_redirect(
            &url::Url::parse("https://user:pass@example.com/private").unwrap(),
            false
        ));
        assert!(allows_redirect(
            &url::Url::parse("https://example.com/next").unwrap(),
            false
        ));
    }

    #[test]
    fn preserves_custom_identity_and_protocol_policy() {
        let client = SafeFetchClient::with_options(SafeFetchOptions {
            timeout: Duration::from_secs(5),
            allow_private: false,
            user_agent: "ExampleReader/2.0".to_string(),
            http1_only: true,
        })
        .unwrap();
        assert_eq!(client.user_agent(), "ExampleReader/2.0");
        assert!(client.http1_only());
    }

    #[test]
    fn acquisition_profiles_are_host_and_path_pinned() {
        let base = SafeFetchClient::new(Duration::from_secs(5)).unwrap();
        assert!(base
            .for_acquisition(AcquisitionPolicy {
                cookie_header: Some("session=secret".into()),
                ..AcquisitionPolicy::default()
            })
            .is_err());
        let profile = base
            .for_acquisition(AcquisitionPolicy {
                allowed_hosts: vec!["example.com".into()],
                allowed_path_prefix: Some("/members/".into()),
                cookie_header: Some("session=secret".into()),
                proxy_url: None,
                allow_invalid_tls: true,
            })
            .unwrap();
        assert!(profile
            .validate_url("https://news.example.com/members/feed.xml")
            .is_ok());
        assert!(profile
            .validate_url("https://news.example.com/public/feed.xml")
            .is_err());
        assert!(profile
            .validate_url("https://example.net/members/feed.xml")
            .is_err());
    }

    #[test]
    fn explicit_proxy_is_a_separate_pinned_connection_boundary() {
        let base = SafeFetchClient::new(Duration::from_secs(5)).unwrap();
        let profile = base
            .for_acquisition(AcquisitionPolicy {
                allowed_hosts: vec!["example.com".into()],
                proxy_url: Some("http://127.0.0.1:3128".into()),
                ..AcquisitionPolicy::default()
            })
            .unwrap();
        assert!(profile.validate_url("https://example.com/feed").is_ok());
        assert!(profile.validate_url("https://internal.test/feed").is_err());
    }
}
