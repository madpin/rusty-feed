//! Bounded, no-script feed discovery for website subscription flows.

use std::collections::HashSet;

use scraper::{Html, Selector};
use url::Url;

const MAX_CANDIDATES: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFeed {
    pub url: String,
    pub title: Option<String>,
    pub media_type: Option<String>,
}

/// Return a direct feed URL or `<link rel=alternate>` feed candidates.
pub fn discover_feed_urls(
    final_url: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> Vec<DiscoveredFeed> {
    let Ok(base) = Url::parse(final_url) else {
        return Vec::new();
    };
    if looks_like_feed(content_type, body) {
        return vec![DiscoveredFeed {
            url: base.to_string(),
            title: None,
            media_type: content_type.map(normalize_media_type),
        }];
    }
    let Ok(html) = std::str::from_utf8(body) else {
        return Vec::new();
    };
    let document = Html::parse_document(html);
    let selector = Selector::parse("link[href]").expect("static selector is valid");
    let mut seen = HashSet::new();
    document
        .select(&selector)
        .filter_map(|element| {
            let value = element.value();
            let alternate = value.attr("rel").is_some_and(|rel| {
                rel.split_ascii_whitespace()
                    .any(|part| part.eq_ignore_ascii_case("alternate"))
            });
            let media_type = value.attr("type").map(normalize_media_type);
            if !alternate || !media_type.as_deref().is_some_and(is_feed_media_type) {
                return None;
            }
            let resolved = base.join(value.attr("href")?).ok()?;
            if !matches!(resolved.scheme(), "http" | "https")
                || !resolved.username().is_empty()
                || resolved.password().is_some()
            {
                return None;
            }
            let mut normalized = resolved;
            normalized.set_fragment(None);
            let url = normalized.to_string();
            seen.insert(url.clone()).then(|| DiscoveredFeed {
                url,
                title: value
                    .attr("title")
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
                    .map(|title| title.chars().take(160).collect()),
                media_type,
            })
        })
        .take(MAX_CANDIDATES)
        .collect()
}

fn normalize_media_type(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn is_feed_media_type(value: &str) -> bool {
    matches!(
        value,
        "application/rss+xml"
            | "application/atom+xml"
            | "application/feed+json"
            | "application/json"
            | "text/xml"
            | "application/xml"
    )
}

fn looks_like_feed(content_type: Option<&str>, body: &[u8]) -> bool {
    if content_type
        .map(normalize_media_type)
        .as_deref()
        .is_some_and(is_feed_media_type)
    {
        return true;
    }
    let prefix = String::from_utf8_lossy(&body[..body.len().min(512)]).to_ascii_lowercase();
    let trimmed = prefix
        .trim_start_matches(|character: char| character.is_whitespace() || character == '\u{feff}');
    trimmed.starts_with("<?xml")
        || trimmed.starts_with("<rss")
        || trimmed.starts_with("<feed")
        || (trimmed.starts_with('{')
            && trimmed.contains("\"version\"")
            && trimmed.contains("\"items\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_resolved_deduplicated_feed_alternates() {
        let found = discover_feed_urls(
            "https://example.test/posts/story",
            Some("text/html; charset=utf-8"),
            br#"<link rel="stylesheet" href="/style.css">
                <link rel="alternate" type="application/rss+xml" title="Main feed" href="/feed.xml">
                <link REL="alternate other" TYPE="application/atom+xml; charset=utf-8" href="../atom.xml#top">
                <link rel="alternate" type="application/rss+xml" href="/feed.xml">"#,
        );
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].url, "https://example.test/feed.xml");
        assert_eq!(found[0].title.as_deref(), Some("Main feed"));
        assert_eq!(found[1].url, "https://example.test/atom.xml");
    }

    #[test]
    fn recognizes_direct_json_feed_without_trusting_html_links() {
        let found = discover_feed_urls(
            "https://example.test/feed",
            Some("application/feed+json"),
            br#"{"version":"https://jsonfeed.org/version/1.1","items":[]}"#,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].url, "https://example.test/feed");
    }
}
