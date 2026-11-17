//! Parse RSS/Atom/JSON feeds (via `feed-rs`) into ingestable articles.

use std::collections::HashMap;

use feed_rs::model::FeedType;
use feed_rs::parser;
use quick_xml::escape::unescape;
use quick_xml::events::Event;
use quick_xml::Reader;
use scraper::Html;
use time::OffsetDateTime;
use uuid::Uuid;

use rusty_feed_core::{
    content_rules::{apply_rewrite_rules, RewriteRule},
    html::{normalize_article_content_with_policies, normalize_article_content_with_policy},
    tracking_pixel::TrackingPixelPolicy,
    tracking_url::TrackingUrlPolicy,
};
use rusty_feed_db::models::{NewArticle, NewArticleEnclosure, MAX_ARTICLE_ENCLOSURES};

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

fn normalized_mime_type(value: Option<&str>) -> Option<String> {
    value
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 255)
        .map(str::to_ascii_lowercase)
}

fn is_enclosure_link(link: &feed_rs::model::Link) -> bool {
    link.rel.as_deref() == Some("enclosure")
        || (link.rel.is_none()
            && link.media_type.as_deref().is_some_and(|mime_type| {
                !matches!(
                    mime_type
                        .split(';')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .to_ascii_lowercase()
                        .as_str(),
                    "text/html" | "application/xhtml+xml"
                )
            }))
}

fn json_attachment_durations(bytes: &[u8]) -> Vec<HashMap<String, i64>> {
    let Ok(document) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Vec::new();
    };
    document
        .get("items")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            item.get("attachments")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|attachment| {
                    let url = attachment
                        .get("url")
                        .and_then(serde_json::Value::as_str)
                        .and_then(enclosure_url)?
                        .to_string();
                    let duration = attachment
                        .get("duration_in_seconds")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|duration| i64::try_from(duration).ok())?;
                    Some((url, duration))
                })
                .take(MAX_ARTICLE_ENCLOSURES)
                .collect()
        })
        .collect()
}

fn media_kind(mime_type: Option<&str>, url: &url::Url) -> &'static str {
    if let Some(mime_type) = mime_type {
        if mime_type.starts_with("audio/") || mime_type == "application/ogg" {
            return "audio";
        }
        if mime_type.starts_with("video/") {
            return "video";
        }
        if mime_type.starts_with("image/") {
            return "image";
        }
    }
    let extension = url
        .path_segments()
        .and_then(Iterator::last)
        .and_then(|name| name.rsplit_once('.').map(|(_, extension)| extension))
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("mp3" | "m4a" | "aac" | "flac" | "oga" | "ogg" | "opus" | "wav") => "audio",
        Some("mp4" | "m4v" | "ogv" | "webm" | "mov") => "video",
        Some("avif" | "gif" | "jpeg" | "jpg" | "png" | "webp") => "image",
        _ => "file",
    }
}

fn bounded_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(240).collect())
}

fn enclosure_url(value: &str) -> Option<url::Url> {
    let mut url = url::Url::parse(value.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.as_str().len() > 4_096
    {
        return None;
    }
    url.set_fragment(None);
    Some(url)
}

fn merge_enclosure(
    enclosures: &mut Vec<NewArticleEnclosure>,
    positions: &mut HashMap<String, usize>,
    incoming: NewArticleEnclosure,
) {
    if let Some(position) = positions.get(&incoming.url).copied() {
        let stored = &mut enclosures[position];
        if stored.media_kind == "file" && incoming.media_kind != "file" {
            stored.media_kind = incoming.media_kind;
        }
        if stored.mime_type.is_none() {
            stored.mime_type = incoming.mime_type;
        }
        if stored.title.is_none() {
            stored.title = incoming.title;
        }
        if stored.size_bytes.is_none() {
            stored.size_bytes = incoming.size_bytes;
        }
        if stored.duration_seconds.is_none() {
            stored.duration_seconds = incoming.duration_seconds;
        }
        if stored.width.is_none() {
            stored.width = incoming.width;
        }
        if stored.height.is_none() {
            stored.height = incoming.height;
        }
    } else if enclosures.len() < MAX_ARTICLE_ENCLOSURES {
        positions.insert(incoming.url.clone(), enclosures.len());
        enclosures.push(incoming);
    }
}

fn entry_enclosures(entry: &feed_rs::model::Entry) -> Vec<NewArticleEnclosure> {
    let mut positions = HashMap::new();
    let mut enclosures = Vec::new();
    for link in entry.links.iter().filter(|link| is_enclosure_link(link)) {
        let Some(url) = enclosure_url(&link.href) else {
            continue;
        };
        let url_string = url.to_string();
        let mime_type = normalized_mime_type(link.media_type.as_deref());
        merge_enclosure(
            &mut enclosures,
            &mut positions,
            NewArticleEnclosure {
                url: url_string,
                media_kind: media_kind(mime_type.as_deref(), &url).to_string(),
                mime_type,
                title: bounded_text(link.title.as_deref()),
                size_bytes: link.length.and_then(|value| i64::try_from(value).ok()),
                duration_seconds: None,
                width: None,
                height: None,
            },
        );
    }
    for media in &entry.media {
        for content in &media.content {
            let Some(url) = content
                .url
                .as_ref()
                .and_then(|url| enclosure_url(url.as_str()))
            else {
                continue;
            };
            let url_string = url.to_string();
            let mime_type = normalized_mime_type(
                content
                    .content_type
                    .as_ref()
                    .map(|content_type| content_type.as_str()),
            );
            let media_kind = media_kind(mime_type.as_deref(), &url).to_string();
            let duration_seconds = if matches!(media_kind.as_str(), "audio" | "video") {
                content
                    .duration
                    .or(media.duration)
                    .and_then(|value| i64::try_from(value.as_secs()).ok())
            } else {
                None
            };
            merge_enclosure(
                &mut enclosures,
                &mut positions,
                NewArticleEnclosure {
                    url: url_string,
                    media_kind,
                    mime_type,
                    title: bounded_text(media.title.as_ref().map(|title| title.content.as_str())),
                    size_bytes: content.size.and_then(|value| i64::try_from(value).ok()),
                    duration_seconds,
                    width: content.width.and_then(|value| i32::try_from(value).ok()),
                    height: content.height.and_then(|value| i32::try_from(value).ok()),
                },
            );
        }
    }
    drop_smaller_image_variants(enclosures)
}

/// Publishers such as The Guardian list one photo as several `media:content`
/// entries at different widths; keep only the largest so the reader shows it once.
fn drop_smaller_image_variants(enclosures: Vec<NewArticleEnclosure>) -> Vec<NewArticleEnclosure> {
    let image_positions: Vec<usize> = enclosures
        .iter()
        .enumerate()
        .filter(|(_, enclosure)| enclosure.media_kind == "image")
        .map(|(position, _)| position)
        .collect();
    if image_positions.len() < 2 {
        return enclosures;
    }
    let images: Vec<(&str, Option<u32>)> = image_positions
        .iter()
        .map(|&position| {
            let enclosure = &enclosures[position];
            let width = enclosure.width.and_then(|width| u32::try_from(width).ok());
            (enclosure.url.as_str(), width)
        })
        .collect();
    let keep: std::collections::HashSet<usize> =
        rusty_feed_core::image_dedup::largest_variant_indices(&images)
            .into_iter()
            .map(|index| image_positions[index])
            .collect();
    enclosures
        .into_iter()
        .enumerate()
        .filter(|(position, enclosure)| enclosure.media_kind != "image" || keep.contains(position))
        .map(|(_, enclosure)| enclosure)
        .collect()
}

#[derive(Clone, Copy)]
enum RssItemUrlField {
    Comments,
    FeedBurnerOriginal,
}

impl RssItemUrlField {
    fn matches(self, event: &quick_xml::events::BytesStart<'_>) -> bool {
        match self {
            Self::Comments => event.name().as_ref() == b"comments",
            Self::FeedBurnerOriginal => event.local_name().as_ref() == b"origLink",
        }
    }

    fn matches_end(self, event: &quick_xml::events::BytesEnd<'_>) -> bool {
        match self {
            Self::Comments => event.name().as_ref() == b"comments",
            Self::FeedBurnerOriginal => event.local_name().as_ref() == b"origLink",
        }
    }
}

fn scan_rss2_item_urls(
    bytes: &[u8],
    field: RssItemUrlField,
) -> Result<Vec<Option<String>>, FeedError> {
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
                    && field.matches(&event)
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
                if comments_depth == Some(depth) && field.matches_end(&event) {
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
    original_article_urls: HashMap<String, String>,
    blocked_tracking_pixels: Vec<u32>,
}

impl ParsedFeed {
    /// Publisher-provided URL before tracking cleanup, when it differs from the
    /// canonical ingest URL.
    pub fn original_url_for(&self, article: &NewArticle) -> Option<&str> {
        self.original_article_urls
            .get(&article.url)
            .map(String::as_str)
    }

    /// Number of tracking images removed from this article's shared RSS body.
    pub fn blocked_tracking_pixels_for(&self, article_index: usize) -> u32 {
        self.blocked_tracking_pixels
            .get(article_index)
            .copied()
            .unwrap_or_default()
    }
}

/// Parse feed bytes for the given (already-created) feed id.
pub fn parse_feed(feed_id: Uuid, bytes: &[u8]) -> Result<ParsedFeed, FeedError> {
    parse_feed_with_policies(
        feed_id,
        bytes,
        &TrackingUrlPolicy::default(),
        &TrackingPixelPolicy::default(),
    )
}

/// Parse feed bytes and apply the instance-wide URL privacy policy before the
/// shared deduplication key is persisted.
pub fn parse_feed_with_policy(
    feed_id: Uuid,
    bytes: &[u8],
    tracking_url_policy: &TrackingUrlPolicy,
) -> Result<ParsedFeed, FeedError> {
    parse_feed_with_policies(
        feed_id,
        bytes,
        tracking_url_policy,
        &TrackingPixelPolicy::default(),
    )
}

/// Parse a feed with the immutable shared URL and tracking-image policies.
pub fn parse_feed_with_policies(
    feed_id: Uuid,
    bytes: &[u8],
    tracking_url_policy: &TrackingUrlPolicy,
    tracking_pixel_policy: &TrackingPixelPolicy,
) -> Result<ParsedFeed, FeedError> {
    parse_feed_with_policies_and_rules(
        feed_id,
        bytes,
        tracking_url_policy,
        tracking_pixel_policy,
        &[],
    )
}

/// Parse a feed and apply validated, ordered shared rewrite rules. Rewritten
/// markup is normalized and sanitized again before it can reach storage.
pub fn parse_feed_with_policies_and_rules(
    feed_id: Uuid,
    bytes: &[u8],
    tracking_url_policy: &TrackingUrlPolicy,
    tracking_pixel_policy: &TrackingPixelPolicy,
    rewrite_rules: &[RewriteRule],
) -> Result<ParsedFeed, FeedError> {
    let feed = parser::parse(bytes).map_err(|e| FeedError::Parse(e.to_string()))?;
    let comments_by_item = if matches!(feed.feed_type, FeedType::RSS2) {
        let comments = scan_rss2_item_urls(bytes, RssItemUrlField::Comments)?;
        if comments.len() != feed.entries.len() {
            return Err(FeedError::Parse(
                "RSS item/comment link alignment mismatch".to_string(),
            ));
        }
        Some(comments)
    } else {
        None
    };
    let feedburner_original_by_item = if matches!(feed.feed_type, FeedType::RSS2) {
        let original = scan_rss2_item_urls(bytes, RssItemUrlField::FeedBurnerOriginal)?;
        if original.len() != feed.entries.len() {
            return Err(FeedError::Parse(
                "RSS item/original link alignment mismatch".to_string(),
            ));
        }
        Some(original)
    } else {
        None
    };
    let json_durations_by_item =
        matches!(feed.feed_type, FeedType::JSON).then(|| json_attachment_durations(bytes));

    let title = feed
        .title
        .as_ref()
        .map(text_construct_text)
        .filter(|title| !title.is_empty());
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
    let mut original_article_urls = HashMap::new();
    let mut blocked_tracking_pixels = Vec::new();
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
            })
            .map(|url| tracking_url_policy.clean(&url));
        let mut enclosures = entry_enclosures(&entry);
        if let Some(durations) = json_durations_by_item
            .as_ref()
            .and_then(|durations| durations.get(index))
        {
            for enclosure in &mut enclosures {
                if matches!(enclosure.media_kind.as_str(), "audio" | "video")
                    && enclosure.duration_seconds.is_none()
                {
                    enclosure.duration_seconds = durations.get(&enclosure.url).copied();
                }
            }
        }
        let image_url = entry
            .media
            .iter()
            .flat_map(|media| media.thumbnails.iter())
            .find_map(|thumbnail| enclosure_url(&thumbnail.image.uri).map(|url| url.to_string()))
            .filter(|thumbnail_url| {
                enclosures
                    .iter()
                    .all(|enclosure| enclosure.url != *thumbnail_url)
            });
        // A stable, unique URL is required (it is the global dedup key).
        let original_url = feedburner_original_by_item
            .as_ref()
            .and_then(|original| original.get(index))
            .cloned()
            .flatten()
            .and_then(|url| enclosure_url(&url).map(|url| url.to_string()))
            .or_else(|| {
                entry
                    .links
                    .iter()
                    .find(|l| l.rel.as_deref() == Some("alternate"))
                    .or_else(|| entry.links.iter().find(|link| !is_enclosure_link(link)))
                    .and_then(|link| enclosure_url(&link.href))
                    .map(|url| url.to_string())
            })
            .or_else(|| enclosure_url(&entry.id).map(|url| url.to_string()))
            .or_else(|| {
                (!entry.id.is_empty() && !enclosures.is_empty()).then(|| {
                    format!(
                        "urn:rusty-feed:article:{feed_id}:{}",
                        rusty_feed_core::hash::content_hash(&entry.id)
                    )
                })
            });
        let Some(original_url) = original_url else {
            continue;
        };
        let url = tracking_url_policy.clean(&original_url);

        let title = entry
            .title
            .as_ref()
            .map(text_construct_text)
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "(untitled)".to_string());

        let content = entry
            .content
            .as_ref()
            .and_then(|content| {
                content.body.as_deref().map(|body| {
                    normalize_article_content_with_policies(
                        body,
                        content.content_type.as_str(),
                        &url,
                        tracking_url_policy,
                        tracking_pixel_policy,
                    )
                })
            })
            .or_else(|| {
                entry.summary.as_ref().map(|summary| {
                    normalize_article_content_with_policies(
                        &summary.content,
                        summary.content_type.as_str(),
                        &url,
                        tracking_url_policy,
                        tracking_pixel_policy,
                    )
                })
            })
            .unwrap_or_default();
        let mut blocked_count = content.blocked_tracking_pixels;
        let content = if rewrite_rules.is_empty() {
            content.html
        } else {
            let rewritten = apply_rewrite_rules(&content.html, rewrite_rules)
                .map_err(|error| FeedError::Parse(format!("content rewrite failed: {error}")))?;
            let normalized = normalize_article_content_with_policies(
                &rewritten,
                "text/html",
                &url,
                tracking_url_policy,
                tracking_pixel_policy,
            );
            blocked_count = blocked_count.saturating_add(normalized.blocked_tracking_pixels);
            normalized.html
        };

        let excerpt_src = entry
            .summary
            .as_ref()
            .map(|summary| {
                normalize_article_content_with_policies(
                    &summary.content,
                    summary.content_type.as_str(),
                    &url,
                    tracking_url_policy,
                    tracking_pixel_policy,
                )
                .html
            })
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

        if original_url != url {
            original_article_urls.insert(url.clone(), original_url);
        }
        blocked_tracking_pixels.push(blocked_count);
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
            enclosures,
            published_at,
        });
    }

    Ok(ParsedFeed {
        title,
        site_url,
        icon_url,
        articles,
        original_article_urls,
        blocked_tracking_pixels,
    })
}

fn text_construct_text(text: &feed_rs::model::Text) -> String {
    let html = normalize_article_content_with_policy(
        &text.content,
        text.content_type.as_str(),
        "https://rusty-feed.invalid/",
        &TrackingUrlPolicy::default(),
    );
    strip_tags(&html)
}

/// Remove HTML tags and collapse whitespace, for plain-text excerpts.
fn strip_tags(html: &str) -> String {
    Html::parse_fragment(html)
        .root_element()
        .text()
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
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
    fn canonicalizes_article_and_content_links_but_preserves_provenance() {
        let rss = br#"<?xml version="1.0"?><rss version="2.0"><channel><title>Clean</title><item>
          <guid>clean-1</guid><title>Clean link</title>
          <link>https://example.com/post?id=7&amp;utm_source=rss&amp;campaign_id=abc</link>
          <description>&lt;a href="https://other.example/path?utm_medium=feed&amp;amp;id=9"&gt;Related&lt;/a&gt;</description>
        </item></channel></rss>"#;
        let policy = TrackingUrlPolicy::try_new(true, "campaign_id", "").expect("valid policy");
        let parsed = parse_feed_with_policy(Uuid::now_v7(), rss, &policy).expect("parse feed");
        let article = &parsed.articles[0];
        assert_eq!(article.url, "https://example.com/post?id=7");
        assert_eq!(
            parsed.original_url_for(article),
            Some("https://example.com/post?id=7&utm_source=rss&campaign_id=abc")
        );
        assert!(article
            .content
            .contains("href=\"https://other.example/path?id=9\""));
    }

    #[test]
    fn prefers_feedburner_original_links_without_an_extra_fetch() {
        let rss = br#"<?xml version="1.0"?><rss version="2.0" xmlns:feedburner="http://rssnamespace.org/feedburner/ext/1.0"><channel><title>Burned</title>
          <item><guid>burn-1</guid><title>Original</title><link>https://feeds.feedburner.com/~r/example/~3/token</link><feedburner:origLink>https://publisher.example/story?utm_source=feedburner&amp;id=9</feedburner:origLink><description>Story</description></item>
          <item><guid>burn-2</guid><title>Fallback</title><link>https://publisher.example/fallback</link><feedburner:origLink>javascript:alert(1)</feedburner:origLink><description>Fallback</description></item>
        </channel></rss>"#;
        let parsed = parse_feed(Uuid::now_v7(), rss).unwrap();
        assert_eq!(
            parsed.articles[0].url,
            "https://publisher.example/story?id=9"
        );
        assert_eq!(
            parsed.original_url_for(&parsed.articles[0]),
            Some("https://publisher.example/story?utm_source=feedburner&id=9")
        );
        assert_eq!(parsed.articles[1].url, "https://publisher.example/fallback");
    }

    #[test]
    fn applies_shared_tracking_pixel_policy_and_reports_removals() {
        let rss = br#"<?xml version="1.0"?><rss version="2.0"><channel><title>Private</title><item>
          <guid>private-1</guid><title>Private article</title>
          <link>https://example.com/private</link>
          <description>&lt;p&gt;Story&lt;/p&gt;&lt;img src="https://metrics.example.com/open.gif"&gt;&lt;img src="https://cdn.example.com/cover.jpg"&gt;</description>
        </item></channel></rss>"#;
        let pixels = TrackingPixelPolicy::try_new(
            rusty_feed_core::tracking_pixel::TrackingPixelStrictness::Conservative,
            "metrics.example.com",
            "",
        )
        .expect("valid policy");
        let parsed =
            parse_feed_with_policies(Uuid::now_v7(), rss, &TrackingUrlPolicy::default(), &pixels)
                .expect("parse feed");
        let article = &parsed.articles[0];
        assert_eq!(parsed.blocked_tracking_pixels_for(0), 1);
        assert!(!article.content.contains("metrics.example.com"));
        assert!(article.content.contains("cdn.example.com/cover.jpg"));
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
    fn media_content_resolution_variants_keep_only_the_largest() {
        let rss = br#"<?xml version="1.0"?>
        <rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/"><channel>
          <title>Guardian</title>
          <item>
            <title>Live</title><link>https://example.com/live</link>
            <media:content width="140" url="https://i.example.com/img/media/f8e6/master/4120.jpg?width=140&amp;quality=85&amp;auto=format&amp;fit=max&amp;s=aaa"><media:credit>PA</media:credit></media:content>
            <media:content width="460" url="https://i.example.com/img/media/f8e6/master/4120.jpg?width=460&amp;quality=85&amp;auto=format&amp;fit=max&amp;s=bbb"><media:credit>PA</media:credit></media:content>
            <media:content width="700" url="https://i.example.com/img/media/f8e6/master/4120.jpg?width=700&amp;quality=85&amp;auto=format&amp;fit=max&amp;s=ccc"><media:credit>PA</media:credit></media:content>
            <media:content width="460" url="https://i.example.com/img/media/other/master/99.jpg?width=460&amp;s=ddd" />
          </item>
        </channel></rss>"#;
        let parsed = parse_feed(Uuid::now_v7(), rss).unwrap();
        let urls: Vec<&str> = parsed.articles[0]
            .enclosures
            .iter()
            .map(|enclosure| enclosure.url.as_str())
            .collect();
        assert_eq!(urls.len(), 2, "{urls:?}");
        assert!(urls[0].contains("width=700"), "{urls:?}");
        assert!(urls[1].contains("other"), "{urls:?}");
    }

    #[test]
    fn preserves_multiple_enclosure_formats_in_stable_order() {
        let rss = br#"<?xml version="1.0"?>
        <rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/"><channel>
          <title>Media feed</title>
          <item>
            <title>Episode</title><link>https://example.com/episode</link>
            <enclosure url="https://cdn.example.com/show.mp3" type="audio/mpeg; charset=binary" length="12345" />
            <enclosure url="javascript:alert(1)" type="audio/mpeg" />
            <media:group>
              <media:title>Episode trailer</media:title>
              <media:content url="https://cdn.example.com/trailer.mp4" type="video/mp4" duration="62" width="1280" height="720" />
              <media:content url="https://cdn.example.com/poster.webp" type="image/webp" width="800" height="450" />
              <media:content url="https://cdn.example.com/show.mp3" type="audio/mpeg" />
            </media:group>
          </item>
        </channel></rss>"#;
        let parsed = parse_feed(Uuid::now_v7(), rss).unwrap();
        let article = &parsed.articles[0];
        assert_eq!(article.enclosures.len(), 3);
        assert_eq!(article.enclosures[0].media_kind, "video");
        assert_eq!(
            article.enclosures[0].title.as_deref(),
            Some("Episode trailer")
        );
        assert_eq!(article.enclosures[0].duration_seconds, Some(62));
        assert_eq!(article.enclosures[0].width, Some(1280));
        assert_eq!(article.enclosures[1].media_kind, "image");
        assert_eq!(article.enclosures[2].media_kind, "audio");
        assert_eq!(
            article.enclosures[2].mime_type.as_deref(),
            Some("audio/mpeg")
        );
        assert_eq!(article.enclosures[2].size_bytes, Some(12_345));
        assert_eq!(article.image_url, None);
    }

    #[test]
    fn caps_enclosures_from_untrusted_feeds() {
        let enclosure_xml = (0..MAX_ARTICLE_ENCLOSURES + 5)
            .map(|index| {
                format!(
                    r#"<enclosure url="https://cdn.example.com/{index}.mp3" type="audio/mpeg" />"#
                )
            })
            .collect::<String>();
        let rss = format!(
            r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Many</title><item><title>Episode</title><link>https://example.com/episode</link>{enclosure_xml}</item></channel></rss>"#
        );
        let parsed = parse_feed(Uuid::now_v7(), rss.as_bytes()).unwrap();
        assert_eq!(parsed.articles[0].enclosures.len(), MAX_ARTICLE_ENCLOSURES);
    }

    #[test]
    fn keeps_attachment_only_json_feed_items_with_an_internal_identity() {
        let feed_id = Uuid::now_v7();
        let json = br#"{
          "version": "https://jsonfeed.org/version/1.1",
          "title": "Attachment-only podcast",
          "items": [{
            "id": "episode-1",
            "title": "First episode",
            "content_text": "Show notes",
            "attachments": [
              {"url": "https://cdn.example.com/episode.mp3", "mime_type": "audio/mpeg", "size_in_bytes": 2048, "duration_in_seconds": 90},
              {"url": "https://cdn.example.com/transcript.pdf", "mime_type": "application/pdf"}
            ]
          }]
        }"#;
        let parsed = parse_feed(feed_id, json).unwrap();
        assert_eq!(parsed.articles.len(), 1);
        let article = &parsed.articles[0];
        assert!(article
            .url
            .starts_with(&format!("urn:rusty-feed:article:{feed_id}:")));
        assert_eq!(article.enclosures.len(), 2);
        assert_eq!(article.enclosures[0].media_kind, "audio");
        assert_eq!(article.enclosures[0].duration_seconds, Some(90));
        assert_eq!(article.enclosures[1].media_kind, "file");
    }

    #[test]
    fn preserves_plain_text_structure_and_literal_angle_brackets() {
        let json = br#"{
          "version": "https://jsonfeed.org/version/1.1",
          "title": "Plain text",
          "items": [{
            "id": "https://example.com/plain",
            "title": "Use <literal> text",
            "content_text": "First <literal> & safe\nsecond line\n\nNew paragraph"
          }]
        }"#;
        let parsed = parse_feed(Uuid::now_v7(), json).unwrap();
        let article = &parsed.articles[0];
        assert_eq!(article.title, "Use <literal> text");
        assert!(article
            .content
            .contains("First &lt;literal&gt; &amp; safe<br>second line"));
        assert!(article.content.contains("</p><p>New paragraph</p>"));
        assert_eq!(
            article.excerpt.as_deref(),
            Some("First <literal> & safe second line New paragraph")
        );
    }

    #[test]
    fn renders_atom_markdown_content_as_semantic_html() {
        let atom = br#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom">
          <title>Markdown</title>
          <entry><id>one</id><title type="html">A &lt;strong&gt;useful&lt;/strong&gt; guide</title>
            <link href="https://example.com/one" />
            <content type="text/markdown">## Steps

- one
- two</content>
          </entry>
        </feed>"#;
        let parsed = parse_feed(Uuid::now_v7(), atom).unwrap();
        let article = &parsed.articles[0];
        assert_eq!(article.title, "A useful guide");
        assert!(article.content.contains("<h2>Steps</h2>"));
        assert!(article.content.contains("<ul>"));
    }

    #[test]
    fn strip_tags_and_truncate_work() {
        assert_eq!(strip_tags("<p>a <b>b</b> c</p>"), "a b c");
        assert_eq!(strip_tags("<p>a &amp; b</p>"), "a & b");
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert_eq!(truncate("ab", 3), "ab");
    }
}
