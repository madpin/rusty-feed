//! # rustss-feeds
//!
//! Feed fetching and ingest: the SSRF-guarded [`SafeFetchClient`], `feed-rs`
//! parsing, URL normalization, conditional GET (ETag/Last-Modified), and
//! synchronous refresh that upserts articles once per shared feed
//! (`docs/ARCHITECTURE.md` §11.1, §12). Background scheduling is M7.

pub mod error;
mod normalize;
mod parse;
pub mod safe_fetch;

pub use error::{FeedError, Result};
pub use normalize::normalize_url;
pub use parse::{parse_feed, ParsedFeed};
pub use safe_fetch::{ensure_http_scheme, is_global_ip, SafeFetchClient};

use futures_util::StreamExt;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};

use rustss_db::{Db, Feed};

/// Max feed body we will buffer (protects against huge/hostile responses).
const MAX_FEED_BYTES: usize = 10 * 1024 * 1024;

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
    client.validate_url(url)?;

    let mut req = client.client().get(url);
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
#[derive(Debug, Default, Clone, Copy)]
pub struct RefreshStats {
    pub not_modified: bool,
    pub created: u64,
    pub updated: u64,
    pub skipped: u64,
}

/// Refresh one shared feed: conditional fetch → parse → upsert articles → update
/// the feed's health/validators. Runs once per feed regardless of subscribers.
pub async fn refresh_feed(db: &Db, client: &SafeFetchClient, feed: &Feed) -> Result<RefreshStats> {
    let outcome = match fetch_feed(
        client,
        &feed.url,
        feed.etag.as_deref(),
        feed.last_modified.as_deref(),
    )
    .await
    {
        Ok(o) => o,
        Err(e) => {
            let _ =
                rustss_db::feeds::record_fetch_failure(db.pool(), feed.id, &e.to_string()).await;
            return Err(e);
        }
    };

    if outcome.not_modified {
        rustss_db::feeds::record_fetch_success(
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

    let parsed = parse_feed(feed.id, &outcome.body)?;

    // Keep the canonical feed name/site fresh from the source (per-user custom
    // names are stored separately and untouched).
    if let Some(title) = parsed.title.as_deref() {
        let _ = rustss_db::feeds::update_metadata(
            db.pool(),
            feed.id,
            title,
            parsed.site_url.as_deref(),
        )
        .await;
    }

    let mut stats = RefreshStats::default();
    for article in &parsed.articles {
        match rustss_db::articles::upsert(db.pool(), article).await {
            Ok((_, rustss_db::UpsertAction::Created)) => stats.created += 1,
            Ok((_, rustss_db::UpsertAction::Updated)) => stats.updated += 1,
            Ok((_, rustss_db::UpsertAction::Skipped)) => stats.skipped += 1,
            Err(e) => tracing::warn!(feed_id = %feed.id, error = %e, "article upsert failed"),
        }
    }

    rustss_db::feeds::record_fetch_success(
        db.pool(),
        feed.id,
        outcome.etag.as_deref(),
        outcome.last_modified.as_deref(),
    )
    .await?;

    Ok(stats)
}
