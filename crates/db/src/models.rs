//! Row structs mapped from the database via `sqlx::FromRow`.
//!
//! These are DB-facing representations. Pure domain/DTO types for the web layer
//! are introduced as needed in later milestones.

use serde_json::Value as Json;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

/// A registered user.
#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub image: Option<String>,
    /// One of `admin` | `user` | `guest`.
    pub role: String,
    pub password_hash: Option<String>,
    pub email_verified_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// A shared feed (one row per normalized URL, regardless of subscribers).
#[derive(Debug, Clone, FromRow)]
pub struct Feed {
    pub id: Uuid,
    pub url: String,
    pub name: String,
    pub site_url: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub last_fetched_at: Option<OffsetDateTime>,
    pub last_successful_fetch_at: Option<OffsetDateTime>,
    pub failure_started_at: Option<OffsetDateTime>,
    pub fetch_interval_minutes: i32,
    pub health_status: String,
    pub consecutive_failures: i32,
    pub error_count: i32,
    pub last_error: Option<String>,
    pub auto_disable_threshold: i32,
    pub notify_on_error: bool,
    pub http_status: Option<i32>,
    pub redirect_url: Option<String>,
    pub extraction_settings: Option<Json>,
    pub respect_robots_txt: bool,
    pub extraction_delay_ms: Option<i32>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Columns for `feeds` (excludes nothing; kept in sync with [`Feed`]).
pub(crate) const FEED_COLUMNS: &str = "id, url, name, site_url, description, image_url, etag, \
    last_modified, last_fetched_at, last_successful_fetch_at, failure_started_at, fetch_interval_minutes, health_status, \
    consecutive_failures, error_count, last_error, auto_disable_threshold, notify_on_error, \
    http_status, redirect_url, extraction_settings, respect_robots_txt, extraction_delay_ms, \
    created_at, updated_at";

/// A shared article (one row per URL). Enrichment columns (summary, embedding)
/// are computed once and shared across all subscribers.
#[derive(Debug, Clone, FromRow)]
pub struct Article {
    pub id: Uuid,
    pub feed_id: Uuid,
    pub url: String,
    pub original_url: String,
    pub comments_url: Option<String>,
    pub guid: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub content: String,
    pub excerpt: Option<String>,
    pub content_hash: Option<String>,
    pub feed_content: Option<String>,
    pub feed_excerpt: Option<String>,
    pub feed_content_hash: Option<String>,
    pub feed_blocked_tracking_pixels: i32,
    pub article_page_content: Option<String>,
    pub article_page_excerpt: Option<String>,
    pub article_page_content_hash: Option<String>,
    pub article_page_fetched_at: Option<OffsetDateTime>,
    pub article_page_expires_at: Option<OffsetDateTime>,
    pub article_page_processing_version: i32,
    pub article_page_blocked_tracking_pixels: i32,
    pub content_source: String,
    pub content_revision: i64,
    pub image_url: Option<String>,
    pub published_at: Option<OffsetDateTime>,
    /// One of `none` | `success` | `failed`.
    pub extraction_status: String,
    pub extraction_error: Option<String>,
    pub summary: Option<String>,
    pub key_points: Option<Json>,
    pub topics: Vec<String>,
    pub summarized_at: Option<OffsetDateTime>,
    pub summary_model: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Columns for `articles` (deliberately excludes the generated `search_vector`).
pub(crate) const ARTICLE_COLUMNS: &str =
    "id, feed_id, url, original_url, comments_url, guid, title, author, content, excerpt, \
    content_hash, feed_content, feed_excerpt, feed_content_hash, feed_blocked_tracking_pixels, article_page_content, \
    article_page_excerpt, article_page_content_hash, article_page_fetched_at, \
    article_page_expires_at, article_page_processing_version, article_page_blocked_tracking_pixels, content_source, content_revision, \
    image_url, published_at, extraction_status, extraction_error, summary, key_points, topics, \
    summarized_at, summary_model, created_at, updated_at";

/// A per-user subscription to a shared feed.
#[derive(Debug, Clone, FromRow)]
pub struct UserFeed {
    pub id: Uuid,
    pub user_id: Uuid,
    pub feed_id: Uuid,
    pub custom_name: Option<String>,
    pub tags: Vec<String>,
    pub settings: Option<Json>,
    pub subscribed_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub(crate) const USER_FEED_COLUMNS: &str =
    "id, user_id, feed_id, custom_name, tags, settings, subscribed_at, updated_at";

/// A server-side session (the cookie carries an opaque token; only its hash is
/// stored here).
#[derive(Debug, Clone, FromRow)]
pub struct Session {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: String,
    pub created_at: OffsetDateTime,
    pub last_used_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

pub(crate) const SESSION_COLUMNS: &str =
    "id, user_id, token_hash, created_at, last_used_at, expires_at";

/// A saved search (persistent boolean+semantic query) (M11).
#[derive(Debug, Clone, FromRow)]
pub struct SavedSearch {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub query: String,
    pub threshold: f64,
    pub notify_on_match: bool,
    pub notify_threshold: f64,
    pub archived: bool,
}

pub(crate) const SAVED_SEARCH_COLUMNS: &str =
    "id, user_id, name, query, threshold, notify_on_match, notify_threshold, archived";

/// Per-domain extraction politeness/backoff state (M8).
#[derive(Debug, Clone, FromRow)]
pub struct DomainState {
    pub domain: String,
    pub last_extracted_at: OffsetDateTime,
    pub failure_count: i32,
    pub rate_limit_count: i32,
    pub crawl_delay_ms: Option<i32>,
    pub last_error: Option<String>,
    pub updated_at: OffsetDateTime,
}

pub(crate) const DOMAIN_STATE_COLUMNS: &str =
    "domain, last_extracted_at, failure_count, rate_limit_count, crawl_delay_ms, last_error, updated_at";

/// A claimed unit of work from the durable job queue (M7).
#[derive(Debug, Clone, FromRow)]
pub struct Job {
    pub id: Uuid,
    pub queue_name: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub attempts: i32,
    pub max_attempts: i32,
}

/// A linked third-party identity (OAuth). Minimal projection used at login.
#[derive(Debug, Clone, FromRow)]
pub struct OAuthAccount {
    pub id: Uuid,
    pub user_id: Uuid,
    pub provider: String,
    pub provider_account_id: String,
    pub created_at: OffsetDateTime,
}

pub(crate) const OAUTH_ACCOUNT_COLUMNS: &str =
    "id, user_id, provider, provider_account_id, created_at";

/// An app-specific API token (native API + GReader). Only the hash is stored.
#[derive(Debug, Clone, FromRow)]
pub struct ApiToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub token_hash: String,
    pub scopes: Vec<String>,
    pub last_used_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
}

pub(crate) const API_TOKEN_COLUMNS: &str =
    "id, user_id, name, token_hash, scopes, last_used_at, created_at, revoked_at";

/// Personal authorization state for one public view of a shared article.
#[derive(Debug, Clone, FromRow)]
pub struct PublicArticleShare {
    pub id: Uuid,
    pub user_id: Uuid,
    pub article_id: Uuid,
    pub token_hash: String,
    pub token_ciphertext: String,
    pub slug: String,
    pub content_source: String,
    pub show_title: bool,
    pub show_metadata: bool,
    pub show_source_link: bool,
    pub include_images: bool,
    pub expires_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub(crate) const PUBLIC_ARTICLE_SHARE_COLUMNS: &str = "id, user_id, article_id, token_hash, \
    token_ciphertext, slug, content_source, show_title, show_metadata, show_source_link, \
    include_images, expires_at, revoked_at, created_at, updated_at";

/// Maximum number of feed-provided attachments retained for one shared article.
pub const MAX_ARTICLE_ENCLOSURES: usize = 64;

/// Input for ingesting an article (before content hashing / persistence).
#[derive(Debug, Clone)]
pub struct NewArticle {
    pub feed_id: Uuid,
    pub url: String,
    pub comments_url: Option<String>,
    pub guid: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub content: String,
    pub excerpt: Option<String>,
    pub image_url: Option<String>,
    pub enclosures: Vec<NewArticleEnclosure>,
    pub published_at: Option<OffsetDateTime>,
}

/// Feed-provided attachment metadata before it is associated with a stored article.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArticleEnclosure {
    pub url: String,
    /// One of `audio`, `video`, `image`, or `file`.
    pub media_kind: String,
    pub mime_type: Option<String>,
    pub title: Option<String>,
    pub size_bytes: Option<i64>,
    pub duration_seconds: Option<i64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

/// Ordered attachment metadata stored once with a shared article.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct ArticleEnclosure {
    pub article_id: Uuid,
    pub position: i32,
    pub url: String,
    /// One of `audio`, `video`, `image`, or `file`.
    pub media_kind: String,
    pub mime_type: Option<String>,
    pub title: Option<String>,
    pub size_bytes: Option<i64>,
    pub duration_seconds: Option<i64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

/// One shared positive or negative media-proxy cache entry.
#[derive(Debug, Clone, FromRow)]
pub struct MediaCacheEntry {
    pub cache_key: String,
    pub source_url: String,
    /// `ready` or `failed`.
    pub status: String,
    pub content_type: Option<String>,
    pub body: Option<Vec<u8>>,
    pub failure_kind: Option<String>,
    pub fetched_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

/// The result of [`crate::articles::upsert`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertAction {
    /// A new article row was created.
    Created,
    /// An existing article row was updated (content changed).
    Updated,
    /// A duplicate was found with no meaningful change; nothing written.
    Skipped,
}
