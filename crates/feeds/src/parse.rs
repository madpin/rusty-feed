//! Parse RSS/Atom/JSON feeds (via `feed-rs`) into ingestable articles.

use feed_rs::model::FeedType;
use feed_rs::parser;
use quick_xml::escape::unescape;
use quick_xml::events::Event;
use quick_xml::Reader;
use time::OffsetDateTime;
use uuid::Uuid;

use rustss_db::models::NewArticle;

use crate::error::FeedError;

fn safe_comments_url(value: &str) -> Option<String> {
    let mut url = url::Url::parse(value.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    url.set_fragment(None);
    Some(url.to_string())
}

fn scan_rss2_comments(bytes: &[u8]) -> Result<Vec<Option<String>>, FeedError> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut item_depth = None;
    let mut comments_depth = None;
    let mut comment = String::new();
    let mut result = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|_| FeedError::Parse("Could not read RSS comment links".to_string()))?
        {
            Event::Start(event) => {
                depth += 1;
                if item_depth.is_none() && event.name().as_ref() == b"item" {
                    item_depth = Some(depth);
                    result.push(None);
                } else if item_depth.is_some_and(|item_depth| depth == item_depth + 1)
                    && event.name().as_ref() == b"comments"
                {
                    comments_depth = Some(depth);
                    comment.clear();
                }
            }
            Event::Text(text) if comments_depth.is_some() => {
                let text = text.xml10_content().map_err(|_| {
                    FeedError::Parse("Could not decode RSS comment links".to_string())
                })?;
                comment.push_str(&text);
            }
            Event::GeneralRef(reference) if comments_depth.is_some() => {
                let reference = reference.xml10_content().map_err(|_| {
                    FeedError::Parse("Could not decode RSS comment links".to_string())
                })?;
                let escaped = format!("&{reference};");
                let text = unescape(&escaped).map_err(|_| {
                    FeedError::Parse("Could not decode RSS comment links".to_string())
                })?;
                comment.push_str(&text);
            }
            Event::CData(text) if comments_depth.is_some() => {
                let text = text.xml10_content().map_err(|_| {
                    FeedError::Parse("Could not decode RSS comment links".to_string())
                })?;
                comment.push_str(&text);
            }
            Event::End(event) => {
                if comments_depth == Some(depth) && event.name().as_ref() == b"comments" {
                    if let Some(slot) = result.last_mut() {
                        *slot = safe_comments_url(&comment);
                    }
                    comments_depth = None;
                }
                if item_depth == Some(depth) && event.name().as_ref() == b"item" {
                    item_depth = None;
                    comments_depth = None;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(result)
}

/// A parsed feed: its metadata plus the articles ready to upsert.
pub struct ParsedFeed {
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub icon_url: Option<String>,
    pub articles: Vec<NewArticle>,
}

/// Parse feed bytes for the given (already-created) feed id.
pub fn parse_feed(feed_id: Uuid, bytes: &[u8]) -> Result<ParsedFeed, FeedError> {
    let feed = parser::parse(bytes).map_err(|e| FeedError::Parse(e.to_string()))?;
    let comments_by_item = if matches!(feed.feed_type, FeedType::RSS2) {
        let comments = scan_rss2_comments(bytes)?;
        if comments.len() != feed.entries.len() {
            return Err(FeedError::Parse(
                "RSS item/comment link alignment mismatch".to_string(),
            ));
        }
        Some(comments)
    } else {
        None
    };

    let title = feed.title.map(|t| t.content).filter(|s| !s.is_empty());
    let site_url = feed
        .links
        .iter()
        .find(|l| l.rel.as_deref() == Some("alternate"))
        .or_else(|| feed.links.first())
        .map(|l| l.href.clone());
    let icon_url = feed
        .icon
        .as_ref()
        .or(feed.logo.as_ref())
        .map(|image| image.uri.clone())
        .filter(|url| !url.trim().is_empty());

    let mut articles = Vec::new();
    for (index, entry) in feed.entries.into_iter().enumerate() {
        let comments_url = comments_by_item
            .as_ref()
            .and_then(|comments| comments.get(index))
            .cloned()
            .flatten()
            .or_else(|| {
                entry
                    .links
                    .iter()
                    .find(|link| link.rel.as_deref() == Some("replies"))
                    .and_then(|link| safe_comments_url(&link.href))
            });
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
            comments_url,
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
        icon_url,
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
      <image><url>https://example.com/icon.png</url><title>Example</title><link>https://example.com</link></image>
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
        assert_eq!(
            parsed.icon_url.as_deref(),
            Some("https://example.com/icon.png")
        );
        assert_eq!(parsed.articles.len(), 1);
        let a = &parsed.articles[0];
        assert_eq!(a.url, "https://example.com/1");
        assert_eq!(a.title, "First Post");
        assert_eq!(a.guid.as_deref(), Some("guid-1"));
        assert_eq!(a.excerpt.as_deref(), Some("Hello world"));
    }

    #[test]
    fn preserves_rss_comments_links_without_replacing_original_urls() {
        let rss = br#"<?xml version="1.0"?><rss version="2.0" xmlns:slash="http://purl.org/rss/1.0/modules/slash/"><channel>
          <title>Hacker News</title>
          <item><title>First</title><link>https://example.com/first</link><description>First</description></item>
          <item><title>Second</title><link>https://example.com/second</link><comments>https://news.ycombinator.com/item?id=123&amp;source=rss</comments><slash:comments>42</slash:comments><description>Second</description></item>
        </channel></rss>"#;
        let parsed = parse_feed(Uuid::now_v7(), rss).unwrap();
        assert_eq!(parsed.articles[0].url, "https://example.com/first");
        assert_eq!(parsed.articles[0].comments_url, None);
        assert_eq!(parsed.articles[1].url, "https://example.com/second");
        assert_eq!(
            parsed.articles[1].comments_url.as_deref(),
            Some("https://news.ycombinator.com/item?id=123&source=rss")
        );
        assert_eq!(safe_comments_url("javascript:alert(1)"), None);
    }

    #[test]
    fn atom_icon_takes_precedence_over_logo() {
        let atom = br#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom">
          <title>Example</title><icon>https://example.com/icon.png</icon><logo>https://example.com/logo.png</logo>
          <entry><id>one</id><title>One</title><link href="https://example.com/one" /></entry>
        </feed>"#;
        let parsed = parse_feed(Uuid::now_v7(), atom).unwrap();
        assert_eq!(
            parsed.icon_url.as_deref(),
            Some("https://example.com/icon.png")
        );
    }

    #[test]
    fn strip_tags_and_truncate_work() {
        assert_eq!(strip_tags("<p>a <b>b</b> c</p>"), "a b c");
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert_eq!(truncate("ab", 3), "ab");
    }
}
