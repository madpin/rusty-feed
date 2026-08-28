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
    last_modified, last_fetched_at, last_successful_fetch_at, fetch_interval_minutes, health_status, \
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
    pub guid: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub content: String,
    pub excerpt: Option<String>,
    pub content_hash: Option<String>,
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
pub(crate) const ARTICLE_COLUMNS: &str = "id, feed_id, url, guid, title, author, content, excerpt, \
    content_hash, image_url, published_at, extraction_status, extraction_error, summary, key_points, \
    topics, summarized_at, summary_model, created_at, updated_at";

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

/// Input for ingesting an article (before content hashing / persistence).
#[derive(Debug, Clone)]
pub struct NewArticle {
    pub feed_id: Uuid,
    pub url: String,
    pub guid: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub content: String,
    pub excerpt: Option<String>,
    pub image_url: Option<String>,
    pub published_at: Option<OffsetDateTime>,
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
