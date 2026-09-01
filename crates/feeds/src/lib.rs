//! # rusty-feed-feeds
//!
//! Feed fetching and ingest: the SSRF-guarded [`SafeFetchClient`], `feed-rs`
//! parsing, URL normalization, conditional GET (ETag/Last-Modified), and
//! synchronous refresh that upserts articles once per shared feed
//! (`docs/ARCHITECTURE.md` §11.1, §12). Background scheduling is M7.

mod discovery;
pub mod error;
mod icons;
mod normalize;
pub mod opml;
mod parse;
pub mod safe_fetch;

pub use discovery::{discover_feed_urls, DiscoveredFeed};
pub use error::{FeedError, Result};
pub use normalize::normalize_url;
pub use opml::{
    export_opml, parse_opml, OpmlDocument, OpmlError, OpmlExportEntry, OpmlSubscription,
};
pub use parse::{
    parse_feed, parse_feed_with_policies, parse_feed_with_policies_and_rules,
    parse_feed_with_policy, ParsedFeed,
};
pub use safe_fetch::{
    ensure_http_scheme, is_global_ip, AcquisitionPolicy, SafeFetchClient, SafeFetchOptions,
    DEFAULT_USER_AGENT,
};

use futures_util::StreamExt;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};

use rusty_feed_core::{tracking_pixel::TrackingPixelPolicy, tracking_url::TrackingUrlPolicy};
use rusty_feed_db::{jobs, Db, Feed};
use uuid::Uuid;

use crate::icons::{favicon_url, resolve_icon_url, try_refresh_feed_icon};

/// Max feed body we will buffer (protects against huge/hostile responses).
const MAX_FEED_BYTES: usize = 10 * 1024 * 1024;

/// One queued fetch represents one health attempt; `list_due` owns retry pacing.
pub const FETCH_FEED_MAX_ATTEMPTS: i32 = 1;

pub async fn enqueue_fetch_feed(db: &Db, feed_id: Uuid) -> rusty_feed_db::Result<Option<Uuid>> {
    let dedup = format!("fetch_feed:{feed_id}");
    jobs::enqueue(
        db.pool(),
        "fetch_feed",
        serde_json::json!({ "feed_id": feed_id }),
        Some(&dedup),
        None,
        FETCH_FEED_MAX_ATTEMPTS,
    )
    .await
}

/// The result of a conditional fetch.
pub struct FetchOutcome {
    pub not_modified: bool,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub final_url: String,
}

/// Fetch a feed with conditional-GET headers. Returns `not_modified` on HTTP 304.
pub async fn fetch_feed(
    client: &SafeFetchClient,
    url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<FetchOutcome> {
    let mut req = client.get(url).await?;
    if let Some(e) = etag {
        req = req.header(IF_NONE_MATCH, e);
    }
    if let Some(lm) = last_modified {
        req = req.header(IF_MODIFIED_SINCE, lm);
    }

    let resp = req.send().await.map_err(FeedError::Http)?;
    let status = resp.status();

    if status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(FetchOutcome {
            not_modified: true,
            body: Vec::new(),
            etag: etag.map(String::from),
            last_modified: last_modified.map(String::from),
            final_url: url.to_string(),
        });
    }
    if !status.is_success() {
        return Err(FeedError::Status(status.as_u16()));
    }

    let etag_out = header_string(resp.headers().get(ETAG));
    let lm_out = header_string(resp.headers().get(LAST_MODIFIED));
    let final_url = resp.url().to_string();
    let body = read_capped(resp, MAX_FEED_BYTES).await?;

    Ok(FetchOutcome {
        not_modified: false,
        body,
        etag: etag_out,
        last_modified: lm_out,
        final_url,
    })
}

fn header_string(value: Option<&reqwest::header::HeaderValue>) -> Option<String> {
    value.and_then(|v| v.to_str().ok()).map(String::from)
}

fn fetch_failure_kind(error: &FeedError) -> rusty_feed_db::feeds::FetchFailure {
    match error {
        FeedError::Http(_) => rusty_feed_db::feeds::FetchFailure::Network,
        FeedError::InvalidUrl(_) => rusty_feed_db::feeds::FetchFailure::InvalidUrl,
        FeedError::UnsupportedScheme(_) => rusty_feed_db::feeds::FetchFailure::UnsupportedScheme,
        FeedError::TooLarge => rusty_feed_db::feeds::FetchFailure::ResponseTooLarge,
        FeedError::Status(status) => rusty_feed_db::feeds::FetchFailure::Http(*status as i32),
        FeedError::Parse(_) => rusty_feed_db::feeds::FetchFailure::Parse,
        FeedError::Db(_) => rusty_feed_db::feeds::FetchFailure::Database,
    }
}

async fn read_capped(resp: reqwest::Response, max: usize) -> Result<Vec<u8>> {
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(FeedError::Http)?;
        if buf.len() + chunk.len() > max {
            return Err(FeedError::TooLarge);
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Stats from a single feed refresh.
#[derive(Debug, Default, Clone)]
pub struct RefreshStats {
    pub not_modified: bool,
    pub created: u64,
    pub updated: u64,
    pub skipped: u64,
    /// Shared article identifiers first inserted by this refresh.
    pub created_article_ids: Vec<uuid::Uuid>,
}

/// Refresh one shared feed: conditional fetch → parse → upsert articles → update
/// the feed's health/validators. Runs once per feed regardless of subscribers.
pub async fn refresh_feed(db: &Db, client: &SafeFetchClient, feed: &Feed) -> Result<RefreshStats> {
    refresh_feed_with_policy(db, client, feed, &TrackingUrlPolicy::default()).await
}

/// Refresh one shared feed using the immutable instance URL privacy policy.
pub async fn refresh_feed_with_policy(
    db: &Db,
    client: &SafeFetchClient,
    feed: &Feed,
    tracking_url_policy: &TrackingUrlPolicy,
) -> Result<RefreshStats> {
    refresh_feed_with_policies(
        db,
        client,
        feed,
        tracking_url_policy,
        &TrackingPixelPolicy::default(),
        0,
        &rusty_feed_core::blocklist::Blocklist::default(),
    )
    .await
}

/// Refresh one shared feed using immutable URL and tracking-image policies.
///
/// `max_items` caps how many articles (newest-first) a single refresh may
/// ingest; `0` means unlimited. This is a **shared** bound (O(1) regardless of
/// subscribers) that blunts feed fan-out amplification (FND-189). `blocklist`
/// skips article URLs taken down by moderation so they cannot re-ingest
/// (FND-192).
pub async fn refresh_feed_with_policies(
    db: &Db,
    client: &SafeFetchClient,
    feed: &Feed,
    tracking_url_policy: &TrackingUrlPolicy,
    tracking_pixel_policy: &TrackingPixelPolicy,
    max_items: usize,
    blocklist: &rusty_feed_core::blocklist::Blocklist,
) -> Result<RefreshStats> {
    let outcome = match fetch_feed(
        client,
        &feed.url,
        feed.etag.as_deref(),
        feed.last_modified.as_deref(),
    )
    .await
    {
        Ok(o) => o,
        Err(error) => {
            let _ = rusty_feed_db::feeds::record_fetch_failure(
                db.pool(),
                feed.id,
                fetch_failure_kind(&error),
            )
            .await;
            return Err(error);
        }
    };

    if outcome.not_modified {
        if let Some(icon_url) = feed
            .image_url
            .clone()
            .or_else(|| favicon_url(feed.site_url.as_deref(), &feed.url))
        {
            if try_refresh_feed_icon(db, client, feed.id, &icon_url)
                .await
                .is_err()
            {
                tracing::debug!(feed_id = %feed.id, "feed icon refresh failed");
            }
        }
        rusty_feed_db::feeds::record_fetch_success(
            db.pool(),
            feed.id,
            outcome.etag.as_deref(),
            outcome.last_modified.as_deref(),
        )
        .await?;
        return Ok(RefreshStats {
            not_modified: true,
            ..Default::default()
        });
    }

    let content_settings = rusty_feed_core::feed_content::FeedContentSettings::from_json(
        feed.extraction_settings.as_ref(),
    );
    let parsed = match parse_feed_with_policies_and_rules(
        feed.id,
        &outcome.body,
        tracking_url_policy,
        tracking_pixel_policy,
        &content_settings.rewrite_rules,
    ) {
        Ok(parsed) => parsed,
        Err(error) => {
            let _ = rusty_feed_db::feeds::record_fetch_failure(
                db.pool(),
                feed.id,
                fetch_failure_kind(&error),
            )
            .await;
            return Err(error);
        }
    };

    // Keep the canonical feed name/site fresh from the source (per-user custom
    // names are stored separately and untouched).
    if let Some(title) = parsed.title.as_deref() {
        let _ = rusty_feed_db::feeds::update_metadata(
            db.pool(),
            feed.id,
            title,
            parsed.site_url.as_deref(),
        )
        .await;
    }
    let icon_url = parsed
        .icon_url
        .as_deref()
        .and_then(|url| resolve_icon_url(url, &outcome.final_url))
        .or_else(|| feed.image_url.clone())
        .or_else(|| {
            favicon_url(
                parsed.site_url.as_deref().or(feed.site_url.as_deref()),
                &outcome.final_url,
            )
        });
    let mut stats = RefreshStats::default();
    // Cap items ingested per refresh (newest-first), dropping the older tail with
    // a counted, logged skip. Uses the original article index for the per-index
    // tracking-pixel lookup so alignment is preserved (FND-189).
    let mut order: Vec<usize> = (0..parsed.articles.len()).collect();
    if max_items > 0 && order.len() > max_items {
        order.sort_by(|&a, &b| {
            parsed.articles[b]
                .published_at
                .cmp(&parsed.articles[a].published_at)
        });
        let dropped = order.len() - max_items;
        order.truncate(max_items);
        tracing::info!(
            feed_id = %feed.id,
            dropped,
            max_items,
            "capped feed items ingested this refresh (FND-189)"
        );
    }
    for article_index in order {
        let article = &parsed.articles[article_index];
        // Skip article URLs blocklisted by moderation (taken-down content).
        if !blocklist.is_empty() {
            if let Some(reason) = blocklist.blocked_reason(&article.url) {
                stats.skipped += 1;
                tracing::info!(feed_id = %feed.id, url = %article.url, reason, "skipped blocklisted article URL (FND-192)");
                continue;
            }
        }
        match rusty_feed_db::articles::upsert_with_original_url_and_privacy(
            db.pool(),
            article,
            parsed.original_url_for(article),
            parsed.blocked_tracking_pixels_for(article_index),
        )
        .await
        {
            Ok((stored, rusty_feed_db::UpsertAction::Created)) => {
                stats.created += 1;
                stats.created_article_ids.push(stored.id);
            }
            Ok((_, rusty_feed_db::UpsertAction::Updated)) => stats.updated += 1,
            Ok((_, rusty_feed_db::UpsertAction::Skipped)) => stats.skipped += 1,
            Err(e) => tracing::warn!(feed_id = %feed.id, error = %e, "article upsert failed"),
        }
    }

    rusty_feed_db::feeds::record_fetch_success(
        db.pool(),
        feed.id,
        outcome.etag.as_deref(),
        outcome.last_modified.as_deref(),
    )
    .await?;
    if let Some(icon_url) = icon_url {
        let _ = rusty_feed_db::feeds::update_image_url(db.pool(), feed.id, &icon_url).await;
        if try_refresh_feed_icon(db, client, feed.id, &icon_url)
            .await
            .is_err()
        {
            tracing::debug!(feed_id = %feed.id, "feed icon refresh failed");
        }
    }

    Ok(stats)
}
