//! Parse RSS/Atom/JSON feeds (via `feed-rs`) into ingestable articles.

use feed_rs::parser;
use time::OffsetDateTime;
use uuid::Uuid;

use rustss_db::models::NewArticle;

use crate::error::FeedError;

/// A parsed feed: its metadata plus the articles ready to upsert.
pub struct ParsedFeed {
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub articles: Vec<NewArticle>,
}

/// Parse feed bytes for the given (already-created) feed id.
pub fn parse_feed(feed_id: Uuid, bytes: &[u8]) -> Result<ParsedFeed, FeedError> {
    let feed = parser::parse(bytes).map_err(|e| FeedError::Parse(e.to_string()))?;

    let title = feed.title.map(|t| t.content).filter(|s| !s.is_empty());
    let site_url = feed
        .links
        .iter()
        .find(|l| l.rel.as_deref() == Some("alternate"))
        .or_else(|| feed.links.first())
        .map(|l| l.href.clone());

    let mut articles = Vec::new();
    for entry in feed.entries {
        // A stable, unique URL is required (it is the global dedup key).
        let Some(url) = entry
            .links
            .iter()
            .find(|l| l.rel.as_deref() == Some("alternate"))
            .or_else(|| entry.links.first())
            .map(|l| l.href.clone())
        else {
            continue;
        };

        let title = entry
            .title
            .map(|t| t.content)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "(untitled)".to_string());

        let content = entry
            .content
            .as_ref()
            .and_then(|c| c.body.clone())
            .or_else(|| entry.summary.as_ref().map(|s| s.content.clone()))
            .unwrap_or_default();

        let excerpt_src = entry
            .summary
            .as_ref()
            .map(|s| s.content.clone())
            .unwrap_or_else(|| content.clone());
        let excerpt = {
            let text = truncate(&strip_tags(&excerpt_src), 280);
            (!text.is_empty()).then_some(text)
        };

        let author = entry
            .authors
            .first()
            .map(|a| a.name.clone())
            .filter(|s| !s.is_empty());

        let published_at = entry
            .published
            .or(entry.updated)
            .and_then(|d| OffsetDateTime::from_unix_timestamp(d.timestamp()).ok());

        let image_url = entry
            .media
            .iter()
            .flat_map(|m| m.content.iter())
            .find_map(|c| c.url.as_ref().map(|u| u.to_string()));

        articles.push(NewArticle {
            feed_id,
            url,
            guid: (!entry.id.is_empty()).then(|| entry.id.clone()),
            title,
            author,
            content,
            excerpt,
            image_url,
            published_at,
        });
    }

    Ok(ParsedFeed {
        title,
        site_url,
        articles,
    })
}

/// Remove HTML tags and collapse whitespace, for plain-text excerpts.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate to at most `max` characters on a char boundary, adding an ellipsis.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max).collect();
    t.push('…');
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &[u8] = br#"<?xml version="1.0"?>
    <rss version="2.0"><channel>
      <title>Example</title>
      <link>https://example.com</link>
      <item>
        <title>First Post</title>
        <link>https://example.com/1</link>
        <guid>guid-1</guid>
        <description>&lt;p&gt;Hello &lt;b&gt;world&lt;/b&gt;&lt;/p&gt;</description>
      </item>
    </channel></rss>"#;

    #[test]
    fn parses_rss_items() {
        let parsed = parse_feed(Uuid::now_v7(), RSS).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Example"));
        assert_eq!(parsed.articles.len(), 1);
        let a = &parsed.articles[0];
        assert_eq!(a.url, "https://example.com/1");
        assert_eq!(a.title, "First Post");
        assert_eq!(a.guid.as_deref(), Some("guid-1"));
        assert_eq!(a.excerpt.as_deref(), Some("Hello world"));
    }

    #[test]
    fn strip_tags_and_truncate_work() {
        assert_eq!(strip_tags("<p>a <b>b</b> c</p>"), "a b c");
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert_eq!(truncate("ab", 3), "ab");
    }
}
