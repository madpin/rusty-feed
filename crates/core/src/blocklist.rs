//! Feed/article blocklist matching (docs/PUBLIC_LAUNCH_PLAN.md §5, FND-192).
//!
//! A pure, unit-testable value compiled from `feed_blocklist` rows. Domain
//! matching is suffix-based (robust against path/query/trailing-slash quirks
//! that make exact-URL matching brittle); exact-URL matching is also supported.
//! The same list guards both feed subscription and article ingest.

use std::collections::HashSet;

/// One compiled blocklist entry kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Domain,
    Url,
}

impl BlockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockKind::Domain => "domain",
            BlockKind::Url => "url",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "domain" => Some(BlockKind::Domain),
            "url" => Some(BlockKind::Url),
            _ => None,
        }
    }
}

/// A raw blocklist row (kind, pattern, reason).
#[derive(Debug, Clone)]
pub struct BlockEntry {
    pub kind: BlockKind,
    pub pattern: String,
    pub reason: Option<String>,
}

/// A compiled, immutable blocklist for O(1)/O(domains) lookups.
#[derive(Debug, Clone, Default)]
pub struct Blocklist {
    /// Exact normalized URLs → reason.
    urls: HashSet<String>,
    /// (lowercased host, reason) domain suffixes.
    domains: Vec<(String, Option<String>)>,
    /// Reason for a URL match, keyed by URL (kept small; parallel to `urls`).
    url_reasons: std::collections::HashMap<String, Option<String>>,
}

impl Blocklist {
    /// Compile from raw rows. Domains are lowercased and stripped of a trailing
    /// dot; URLs are matched exactly as stored (already normalized on insert).
    pub fn compile(entries: impl IntoIterator<Item = BlockEntry>) -> Self {
        let mut urls = HashSet::new();
        let mut url_reasons = std::collections::HashMap::new();
        let mut domains = Vec::new();
        for entry in entries {
            match entry.kind {
                BlockKind::Url => {
                    urls.insert(entry.pattern.clone());
                    url_reasons.insert(entry.pattern, entry.reason);
                }
                BlockKind::Domain => {
                    let host = normalize_host(&entry.pattern);
                    if !host.is_empty() {
                        domains.push((host, entry.reason));
                    }
                }
            }
        }
        Self {
            urls,
            domains,
            url_reasons,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.urls.is_empty() && self.domains.is_empty()
    }

    /// Whether `normalized_url` is blocked; returns the reason if so.
    pub fn blocked_reason(&self, normalized_url: &str) -> Option<String> {
        if self.urls.contains(normalized_url) {
            return Some(reason_or_default(
                self.url_reasons.get(normalized_url).and_then(Clone::clone),
            ));
        }
        let host = host_of(normalized_url)?;
        self.domains.iter().find_map(|(domain, reason)| {
            host_matches(&host, domain).then(|| reason_or_default(reason.clone()))
        })
    }

    /// Convenience predicate.
    pub fn is_blocked(&self, normalized_url: &str) -> bool {
        self.blocked_reason(normalized_url).is_some()
    }
}

fn reason_or_default(reason: Option<String>) -> String {
    reason.unwrap_or_else(|| "blocked by an administrator".to_string())
}

/// Lowercase a host and drop a single trailing dot (FQDN form).
pub fn normalize_host(host: &str) -> String {
    host.trim()
        .trim_end_matches('.')
        .to_ascii_lowercase()
        .to_string()
}

fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(normalize_host))
}

/// Case-insensitive suffix match: `host == domain` or `host` ends with `.domain`.
fn host_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dom(p: &str) -> BlockEntry {
        BlockEntry {
            kind: BlockKind::Domain,
            pattern: p.to_string(),
            reason: Some("bad".to_string()),
        }
    }
    fn url(p: &str) -> BlockEntry {
        BlockEntry {
            kind: BlockKind::Url,
            pattern: p.to_string(),
            reason: None,
        }
    }

    #[test]
    fn domain_matches_host_and_subdomains_case_insensitively() {
        let bl = Blocklist::compile([dom("Evil.example")]);
        assert!(bl.is_blocked("https://evil.example/feed.xml"));
        assert!(bl.is_blocked("https://news.EVIL.example/rss"));
        assert!(bl.is_blocked("https://evil.example./rss")); // trailing dot
        assert!(!bl.is_blocked("https://notevil.example/rss"));
        assert!(!bl.is_blocked("https://example.com/rss"));
    }

    #[test]
    fn exact_url_matches_only_that_url() {
        let bl = Blocklist::compile([url("https://ok.example/bad.xml")]);
        assert!(bl.is_blocked("https://ok.example/bad.xml"));
        assert!(!bl.is_blocked("https://ok.example/good.xml"));
    }

    #[test]
    fn empty_blocklist_blocks_nothing() {
        let bl = Blocklist::default();
        assert!(!bl.is_blocked("https://anything.example/feed"));
        assert!(bl.is_empty());
    }

    #[test]
    fn reason_is_reported() {
        let bl = Blocklist::compile([dom("spam.example")]);
        assert_eq!(
            bl.blocked_reason("https://spam.example/x"),
            Some("bad".to_string())
        );
    }
}
