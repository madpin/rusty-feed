//! Deterministic, no-network policy for identifying remote tracking images.

use serde::Deserialize;
use url::Url;

const BUILTIN_TRACKER_HOSTS: &[&str] = &[
    "analytics.twitter.com",
    "bat.bing.com",
    "ib.adnxs.com",
    "pixel.wp.com",
    "px.ads.linkedin.com",
    "ssl.google-analytics.com",
    "stats.wp.com",
    "www.google-analytics.com",
];

/// How aggressively URL-only tracker signals are interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TrackingPixelStrictness {
    /// Remove hidden/tiny images and administrator-blocked hosts only.
    Conservative,
    /// Also remove known tracker hosts and unambiguous beacon endpoints.
    #[default]
    Balanced,
    /// Also remove images whose URL contains broader tracking signals.
    Strict,
}

impl TrackingPixelStrictness {
    /// Stable value used in configuration/status surfaces.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conservative => "conservative",
            Self::Balanced => "balanced",
            Self::Strict => "strict",
        }
    }
}

/// Immutable instance policy for remote tracking images.
///
/// Host entries match the named host and its subdomains. Allow-list entries
/// override URL/host classification, but never override explicit tiny/hidden
/// dimensions handled by the HTML sanitizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackingPixelPolicy {
    strictness: TrackingPixelStrictness,
    blocked_hosts: Vec<String>,
    allowed_hosts: Vec<String>,
}

impl Default for TrackingPixelPolicy {
    fn default() -> Self {
        Self {
            strictness: TrackingPixelStrictness::Balanced,
            blocked_hosts: Vec::new(),
            allowed_hosts: Vec::new(),
        }
    }
}

impl TrackingPixelPolicy {
    /// Build and validate an environment-friendly pixel policy.
    pub fn try_new(
        strictness: TrackingPixelStrictness,
        blocked_hosts: &str,
        allowed_hosts: &str,
    ) -> Result<Self, String> {
        Ok(Self {
            strictness,
            blocked_hosts: parse_hosts(blocked_hosts, "blocked tracking-pixel hosts")?,
            allowed_hosts: parse_hosts(allowed_hosts, "allowed tracking-pixel hosts")?,
        })
    }

    pub const fn strictness(&self) -> TrackingPixelStrictness {
        self.strictness
    }

    pub fn blocked_host_count(&self) -> usize {
        self.blocked_hosts.len()
    }

    pub fn allowed_host_count(&self) -> usize {
        self.allowed_hosts.len()
    }

    /// Decide from a resolved HTTP(S) image URL without contacting its host.
    pub fn blocks_url(&self, value: &str) -> bool {
        let Ok(url) = Url::parse(value) else {
            return false;
        };
        if !matches!(url.scheme(), "http" | "https") {
            return false;
        }
        let Some(host) = url.host_str().map(normalize_host) else {
            return false;
        };
        if self
            .allowed_hosts
            .iter()
            .any(|allowed| host_matches(&host, allowed))
        {
            return false;
        }
        if self
            .blocked_hosts
            .iter()
            .any(|blocked| host_matches(&host, blocked))
        {
            return true;
        }
        if self.strictness != TrackingPixelStrictness::Conservative
            && BUILTIN_TRACKER_HOSTS
                .iter()
                .any(|blocked| host_matches(&host, blocked))
        {
            return true;
        }

        let path = url.path().to_ascii_lowercase();
        if self.strictness != TrackingPixelStrictness::Conservative
            && host_matches(&host, "www.facebook.com")
            && path == "/tr"
        {
            return true;
        }
        let unambiguous_path = ["/beacon", "/pixel.gif", "/tracking.gif", "/open.gif"]
            .iter()
            .any(|signal| path.ends_with(signal) || path.contains(&format!("{signal}/")));
        if self.strictness != TrackingPixelStrictness::Conservative && unambiguous_path {
            return true;
        }
        if self.strictness == TrackingPixelStrictness::Strict {
            let broader_path = ["/track/", "/tracker/", "/tracking/", "/pixel/"]
                .iter()
                .any(|signal| path.contains(signal));
            let tracking_query = url.query_pairs().any(|(name, value)| {
                ["beacon", "pixel", "track", "tracking"]
                    .iter()
                    .any(|signal| {
                        name.eq_ignore_ascii_case(signal) || value.eq_ignore_ascii_case(signal)
                    })
            });
            return broader_path || tracking_query;
        }
        false
    }
}

fn parse_hosts(value: &str, label: &str) -> Result<Vec<String>, String> {
    let mut hosts = value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_host)
        .collect::<Vec<_>>();
    if hosts.len() > 100 {
        return Err(format!("{label} supports at most 100 entries"));
    }
    for host in &hosts {
        let valid_labels = host.split('.').count() >= 2
            && host.split('.').all(|part| {
                !part.is_empty()
                    && !part.starts_with('-')
                    && !part.ends_with('-')
                    && part
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '-')
            });
        if host.is_empty() || host.len() > 253 || !valid_labels {
            return Err(format!("invalid {label} entry: {host}"));
        }
    }
    hosts.sort();
    hosts.dedup();
    Ok(hosts)
}

fn normalize_host(value: &str) -> String {
    value.trim_end_matches('.').to_ascii_lowercase()
}

fn host_matches(host: &str, configured: &str) -> bool {
    host == configured
        || host
            .strip_suffix(configured)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_blocks_known_hosts_and_beacons() {
        let policy = TrackingPixelPolicy::default();
        assert!(policy.blocks_url("https://pixel.wp.com/a.gif"));
        assert!(policy.blocks_url("https://news.example.com/events/beacon"));
        assert!(!policy.blocks_url("https://news.example.com/images/cover.jpg"));
    }

    #[test]
    fn allow_list_overrides_url_classification() {
        let policy = TrackingPixelPolicy::try_new(
            TrackingPixelStrictness::Strict,
            "blocked.example.com",
            "images.blocked.example.com,pixel.wp.com",
        )
        .expect("valid policy");
        assert!(!policy.blocks_url("https://images.blocked.example.com/tracking/pixel.gif"));
        assert!(!policy.blocks_url("https://pixel.wp.com/a.gif"));
        assert!(policy.blocks_url("https://blocked.example.com/photo.jpg"));
    }

    #[test]
    fn strict_adds_broader_signals_and_configuration_is_bounded() {
        let strict = TrackingPixelPolicy::try_new(TrackingPixelStrictness::Strict, "", "")
            .expect("valid policy");
        assert!(strict.blocks_url("https://example.com/assets/tracking/open"));
        assert!(TrackingPixelPolicy::try_new(
            TrackingPixelStrictness::Balanced,
            "https://example.com",
            ""
        )
        .is_err());
    }
}
