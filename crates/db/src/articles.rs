//! Article repository — shared articles with ingest deduplication.
//!
//! Dedup order (docs/ARCHITECTURE.md §11.1): GUID (feed-scoped) -> URL (global)
//! -> content hash (feed-scoped). [`upsert`] creates, updates, or skips, keeping
//! exactly one row per article regardless of how many users/feeds reference it.

use rusty_feed_core::{
    feed_content::ContentLifetime,
    hash::content_hash,
    html::{
        sanitize_article_html_with_policies, SanitizedArticleHtml, ARTICLE_PAGE_PROCESSING_VERSION,
    },
    tracking_pixel::TrackingPixelPolicy,
    tracking_url::TrackingUrlPolicy,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DbError, Result};
use crate::models::{
    Article, ArticleEnclosure, NewArticle, NewArticleEnclosure, UpsertAction, ARTICLE_COLUMNS,
    MAX_ARTICLE_ENCLOSURES,
};

/// A content revision queued for extraction or embedding.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ArticleRevision {
    pub id: Uuid,
    pub content_revision: i64,
}

/// Store extracted full content for the expected shared content revision.
pub async fn set_extracted_content(
    pool: &PgPool,
    id: Uuid,
    expected_revision: i64,
    content: &str,
    excerpt: Option<&str>,
    lifetime: ContentLifetime,
) -> Result<Option<i64>> {
    set_extracted_content_with_privacy(pool, id, expected_revision, content, excerpt, lifetime, 0)
        .await
}

/// Store extracted full content and its shared tracker-cleanup count.
pub async fn set_extracted_content_with_privacy(
    pool: &PgPool,
    id: Uuid,
    expected_revision: i64,
    content: &str,
    excerpt: Option<&str>,
    lifetime: ContentLifetime,
    blocked_tracking_pixels: u32,
) -> Result<Option<i64>> {
    let content_hash = rusty_feed_core::hash::content_hash(content);
    let lifetime_days = lifetime.days().map(i32::from);
    Ok(sqlx::query_scalar(
        "UPDATE articles SET content = $3, content_hash = $4, \
         excerpt = COALESCE($5, excerpt), content_source = 'article_page', \
         article_page_content = $3, article_page_content_hash = $4, \
         article_page_excerpt = $5, article_page_fetched_at = now(), \
         article_page_expires_at = CASE WHEN $6::int IS NULL THEN NULL \
              ELSE now() + make_interval(days => $6) END, \
         article_page_processing_version = $7, article_page_blocked_tracking_pixels = $8, \
         content_revision = content_revision + 1, extraction_status = 'success', \
         extraction_error = NULL, summary = NULL, key_points = NULL, topics = '{}', \
         summarized_at = NULL, summary_model = NULL, updated_at = now() \
         WHERE id = $1 AND content_revision = $2 \
           AND (article_page_content IS NULL OR article_page_expires_at <= now() \
                OR article_page_processing_version < $7) \
         RETURNING content_revision",
    )
    .bind(id)
    .bind(expected_revision)
    .bind(content)
    .bind(content_hash)
    .bind(excerpt)
    .bind(lifetime_days)
    .bind(ARTICLE_PAGE_PROCESSING_VERSION)
    .bind(i32::try_from(blocked_tracking_pixels).unwrap_or(i32::MAX))
    .fetch_optional(pool)
    .await?)
}

/// Article revisions whose shared public-page copy is absent or expired.
pub async fn list_needing_extraction(
    pool: &PgPool,
    feed_id: Uuid,
    limit: i64,
) -> Result<Vec<ArticleRevision>> {
    Ok(sqlx::query_as(
        "SELECT id, content_revision FROM articles \
         WHERE feed_id = $1 AND (article_page_content IS NULL \
              OR article_page_expires_at <= now() \
              OR article_page_processing_version < $3) \
         ORDER BY created_at DESC LIMIT $2",
    )
    .bind(feed_id)
    .bind(limit)
    .bind(ARTICLE_PAGE_PROCESSING_VERSION)
    .fetch_all(pool)
    .await?)
}

/// Article revisions in a feed that have no current embedding yet.
pub async fn list_needing_embedding(
    pool: &PgPool,
    feed_id: Uuid,
    limit: i64,
) -> Result<Vec<ArticleRevision>> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.content_revision FROM articles a \
         LEFT JOIN article_embeddings e ON e.article_id = a.id \
           AND e.content_revision = a.content_revision \
         WHERE a.feed_id = $1 AND e.article_id IS NULL \
         ORDER BY a.created_at DESC LIMIT $2",
    )
    .bind(feed_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Shared article revisions lacking the configured model. UUID cursoring keeps
/// memory bounded and remains stable while workers consume the queued jobs.
pub async fn list_needing_embedding_space(
    pool: &PgPool,
    model: &str,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<ArticleRevision>> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.content_revision FROM articles a \
         LEFT JOIN article_embeddings e ON e.article_id = a.id \
           AND e.model = $1 AND e.content_revision = a.content_revision \
         WHERE e.article_id IS NULL AND ($2::uuid IS NULL OR a.id > $2) \
         ORDER BY a.id LIMIT $3",
    )
    .bind(model)
    .bind(after)
    .bind(limit.clamp(1, 5_000))
    .fetch_all(pool)
    .await?)
}

/// Unsummarized shared content considered by a bounded policy scan.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SummaryCandidate {
    pub id: Uuid,
    pub content_revision: i64,
    content: String,
}

impl SummaryCandidate {
    /// Whether the effective body reaches the normalized readable-text minimum.
    pub fn meets_minimum(&self, min_content_chars: u32) -> bool {
        meets_summary_minimum(&self.content, min_content_chars)
    }
}

/// Check automatic-summary eligibility against normalized readable text.
pub fn meets_summary_minimum(content: &str, min_content_chars: u32) -> bool {
    rusty_feed_core::html::article_plain_text_len(content) >= min_content_chars as usize
}

/// Page through unsummarized candidates by stable shared article id.
pub async fn list_summary_candidates_after(
    pool: &PgPool,
    feed_id: Option<Uuid>,
    ready_only: bool,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<SummaryCandidate>> {
    Ok(sqlx::query_as(
        "SELECT id, content_revision, content FROM articles \
         WHERE summarized_at IS NULL AND ($1::uuid IS NULL OR feed_id = $1) \
           AND (NOT $2 OR extraction_status <> 'none') \
           AND ($3::uuid IS NULL OR id > $3) \
         ORDER BY id LIMIT $4",
    )
    .bind(feed_id)
    .bind(ready_only)
    .bind(after)
    .bind(limit.clamp(1, 5_000))
    .fetch_all(pool)
    .await?)
}

/// Load unsummarized candidates changed by one feed consumption.
pub async fn list_summary_candidates_by_ids(
    pool: &PgPool,
    article_ids: &[Uuid],
    ready_only: bool,
) -> Result<Vec<SummaryCandidate>> {
    if article_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT id, content_revision, content FROM articles \
         WHERE id = ANY($1) AND summarized_at IS NULL \
           AND (NOT $2 OR extraction_status <> 'none')",
    )
    .bind(article_ids)
    .bind(ready_only)
    .fetch_all(pool)
    .await?)
}

/// Article revisions ready for embedding after page extraction completed or fell
/// back to RSS content.
pub async fn list_ready_for_embedding(
    pool: &PgPool,
    feed_id: Uuid,
    limit: i64,
) -> Result<Vec<ArticleRevision>> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.content_revision FROM articles a \
         LEFT JOIN article_embeddings e ON e.article_id = a.id \
           AND e.content_revision = a.content_revision \
         WHERE a.feed_id = $1 AND a.extraction_status <> 'none' AND e.article_id IS NULL \
         ORDER BY a.created_at DESC LIMIT $2",
    )
    .bind(feed_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Mark extraction failed for the expected revision and keep the feed fallback.
pub async fn set_extraction_failed(
    pool: &PgPool,
    id: Uuid,
    expected_revision: i64,
    error: &str,
) -> Result<Option<i64>> {
    Ok(sqlx::query_scalar(
        "UPDATE articles SET extraction_status = 'failed', extraction_error = $3, updated_at = now() \
         WHERE id = $1 AND content_revision = $2 \
           AND (article_page_content IS NULL OR article_page_expires_at <= now() \
                OR article_page_processing_version < $4) \
         RETURNING content_revision",
    )
    .bind(id)
    .bind(expected_revision)
    .bind(error)
    .bind(ARTICLE_PAGE_PROCESSING_VERSION)
    .fetch_optional(pool)
    .await?)
}

/// Apply the current union of subscriber lifetime requests to existing public
/// page copies. `None` means nobody currently requests page content, so the
/// copies become eligible for immediate cleanup.
pub async fn apply_article_page_lifetime(
    pool: &PgPool,
    feed_id: Uuid,
    lifetime: Option<ContentLifetime>,
) -> Result<u64> {
    let lifetime_days = lifetime.and_then(ContentLifetime::days).map(i32::from);
    let requested = lifetime.is_some();
    let result = sqlx::query(
        "UPDATE articles SET article_page_expires_at = CASE \
             WHEN NOT $2 THEN now() \
             WHEN $3::int IS NULL THEN NULL \
             ELSE article_page_fetched_at + make_interval(days => $3) END, updated_at = now() \
         WHERE feed_id = $1 AND article_page_content IS NOT NULL",
    )
    .bind(feed_id)
    .bind(requested)
    .bind(lifetime_days)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Remove expired public-page copies while retaining the RSS body. Returns the
/// number of shared articles restored to their RSS representation.
pub async fn clear_expired_article_page_content(pool: &PgPool) -> Result<u64> {
    let result = sqlx::query(
        "UPDATE articles SET content = COALESCE(feed_content, ''), excerpt = feed_excerpt, \
         content_hash = feed_content_hash, content_source = 'feed', \
         article_page_content = NULL, article_page_excerpt = NULL, \
         article_page_content_hash = NULL, article_page_fetched_at = NULL, \
         article_page_expires_at = NULL, article_page_processing_version = 0, \
         article_page_blocked_tracking_pixels = 0, \
         content_revision = content_revision + 1, \
         extraction_status = 'none', extraction_error = NULL, summary = NULL, \
         key_points = NULL, topics = '{}', summarized_at = NULL, summary_model = NULL, \
         updated_at = now() WHERE article_page_content IS NOT NULL \
           AND article_page_expires_at <= now()",
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Delete old shared article rows while preserving anything bookmarked by any
/// user. The age-basis enum is validated configuration, never request input.
pub async fn delete_expired_unbookmarked(
    pool: &PgPool,
    retention_days: u32,
    age_basis: rusty_feed_core::config::RetentionAgeBasis,
) -> Result<u64> {
    if retention_days == 0 {
        return Ok(0);
    }
    let age = match age_basis {
        rusty_feed_core::config::RetentionAgeBasis::Published => {
            "COALESCE(a.published_at, a.created_at)"
        }
        rusty_feed_core::config::RetentionAgeBasis::Created => "a.created_at",
    };
    let sql = format!(
        "DELETE FROM articles a WHERE {age} < now() - make_interval(days => $1::int) \
         AND NOT EXISTS (SELECT 1 FROM bookmarks b WHERE b.article_id = a.id)"
    );
    Ok(sqlx::query(&sql)
        .bind(i32::try_from(retention_days).unwrap_or(i32::MAX))
        .execute(pool)
        .await?
        .rows_affected())
}

/// A stored article summary for display.
#[derive(Debug, Clone)]
pub struct ArticleSummary {
    pub summary: String,
    pub key_points: Vec<String>,
    pub topics: Vec<String>,
}

/// Fetch an article's summary, if it has one.
pub async fn get_summary(pool: &PgPool, id: Uuid) -> Result<Option<ArticleSummary>> {
    let row: Option<(Option<String>, Option<serde_json::Value>, Vec<String>)> = sqlx::query_as(
        "SELECT summary, key_points, topics FROM articles \
         WHERE id = $1 AND summarized_at IS NOT NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(summary, key_points, topics)| ArticleSummary {
        summary: summary.unwrap_or_default(),
        key_points: key_points
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default(),
        topics,
    }))
}

/// Whether an article already has a summary.
pub async fn is_summarized(pool: &PgPool, id: Uuid) -> Result<bool> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM articles WHERE id = $1 AND summarized_at IS NOT NULL")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

/// Store a summary on a shared article, **first-writer-wins** (only if not
/// already summarized). Returns whether this call wrote it.
pub async fn set_summary(
    pool: &PgPool,
    id: Uuid,
    content_revision: i64,
    summary: &str,
    key_points: &[String],
    topics: &[String],
    model: &str,
) -> Result<bool> {
    let written: Option<Uuid> = sqlx::query_scalar(
        "UPDATE articles SET summary = $3, key_points = $4, topics = $5, summary_model = $6, \
         summarized_at = now() WHERE id = $1 AND content_revision = $2 \
         AND summarized_at IS NULL RETURNING id",
    )
    .bind(id)
    .bind(content_revision)
    .bind(summary)
    .bind(serde_json::json!(key_points))
    .bind(topics)
    .bind(model)
    .fetch_optional(pool)
    .await?;
    Ok(written.is_some())
}

/// Find an article by id.
pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Article>> {
    let sql = format!("SELECT {ARTICLE_COLUMNS} FROM articles WHERE id = $1");
    Ok(sqlx::query_as::<_, Article>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// Global takedown of a shared article (FND-192): delete the row everywhere,
/// cascading its embedding, summary columns, enclosures, and per-user state.
/// This is the deliberate, admin-only exception to "unsubscribe never deletes
/// shared content". Returns whether a row was deleted.
pub async fn delete_by_id(pool: &PgPool, id: Uuid) -> Result<bool> {
    Ok(sqlx::query("DELETE FROM articles WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Whether `user_id` is subscribed to the feed that owns `article_id` (used to
/// authorize reading a shared article).
pub async fn user_can_access(pool: &PgPool, user_id: Uuid, article_id: Uuid) -> Result<bool> {
    let exists: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM articles a \
         JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
         WHERE a.id = $2 LIMIT 1",
    )
    .bind(user_id)
    .bind(article_id)
    .fetch_optional(pool)
    .await?;
    Ok(exists.is_some())
}

/// List feed-provided attachments in their stored order.
pub async fn list_enclosures(pool: &PgPool, article_id: Uuid) -> Result<Vec<ArticleEnclosure>> {
    Ok(sqlx::query_as(
        "SELECT article_id, position, url, media_kind, mime_type, title, size_bytes, \
         duration_seconds, width, height FROM article_enclosures \
         WHERE article_id = $1 ORDER BY position",
    )
    .bind(article_id)
    .fetch_all(pool)
    .await?)
}

/// Find one feed-provided attachment by its stable article-local position.
pub async fn find_enclosure(
    pool: &PgPool,
    article_id: Uuid,
    position: i32,
) -> Result<Option<ArticleEnclosure>> {
    Ok(sqlx::query_as(
        "SELECT article_id, position, url, media_kind, mime_type, title, size_bytes, \
         duration_seconds, width, height FROM article_enclosures \
         WHERE article_id = $1 AND position = $2",
    )
    .bind(article_id)
    .bind(position)
    .fetch_optional(pool)
    .await?)
}

/// Find an article by its globally-unique URL.
pub async fn find_by_url(pool: &PgPool, url: &str) -> Result<Option<Article>> {
    let sql = format!("SELECT {ARTICLE_COLUMNS} FROM articles WHERE url = $1");
    Ok(sqlx::query_as::<_, Article>(&sql)
        .bind(url)
        .fetch_optional(pool)
        .await?)
}

async fn find_by_feed_and_guid(
    pool: &PgPool,
    feed_id: Uuid,
    guid: &str,
) -> Result<Option<Article>> {
    let sql = format!("SELECT {ARTICLE_COLUMNS} FROM articles WHERE feed_id = $1 AND guid = $2");
    Ok(sqlx::query_as::<_, Article>(&sql)
        .bind(feed_id)
        .bind(guid)
        .fetch_optional(pool)
        .await?)
}

async fn find_by_feed_and_hash(
    pool: &PgPool,
    feed_id: Uuid,
    hash: &str,
) -> Result<Option<Article>> {
    let sql = format!(
        "SELECT {ARTICLE_COLUMNS} FROM articles WHERE feed_id = $1 AND feed_content_hash = $2"
    );
    Ok(sqlx::query_as::<_, Article>(&sql)
        .bind(feed_id)
        .bind(hash)
        .fetch_optional(pool)
        .await?)
}

fn feed_content(new: &NewArticle) -> SanitizedArticleHtml {
    sanitize_article_html_with_policies(
        &new.content,
        &new.url,
        &TrackingUrlPolicy::default(),
        &TrackingPixelPolicy::default(),
    )
}

fn feed_content_hash(new: &NewArticle) -> String {
    content_hash(&feed_content(new).html)
}

fn feed_content_changed(existing: &Article, feed_hash: &str) -> bool {
    existing
        .feed_content_hash
        .as_deref()
        .is_some_and(|existing_hash| existing_hash != feed_hash)
}

/// Find an existing article that `new` duplicates, using the GUID -> URL -> hash
/// strategy. Content-hash matching is only attempted when there is no GUID
/// (matching the reference implementation).
pub async fn find_duplicate(pool: &PgPool, new: &NewArticle) -> Result<Option<Article>> {
    if let Some(guid) = new.guid.as_deref() {
        if let Some(a) = find_by_feed_and_guid(pool, new.feed_id, guid).await? {
            return Ok(Some(a));
        }
    }
    if let Some(a) = find_by_url(pool, &new.url).await? {
        return Ok(Some(a));
    }
    if new.guid.is_none() {
        let hash = feed_content_hash(new);
        if let Some(a) = find_by_feed_and_hash(pool, new.feed_id, &hash).await? {
            return Ok(Some(a));
        }
    }
    Ok(None)
}

/// Whether an existing article should be updated from `new`: source content or
/// metadata changed, or the feed-source baseline is not known yet.
pub fn should_update(existing: &Article, new: &NewArticle) -> bool {
    should_update_with_original_url(existing, new, None)
}

fn should_update_with_original_url(
    existing: &Article,
    new: &NewArticle,
    original_url: Option<&str>,
) -> bool {
    let feed = feed_content(new);
    let feed_hash = content_hash(&feed.html);
    should_update_fields(
        existing,
        new,
        original_url,
        &feed_hash,
        feed.blocked_tracking_pixels,
    )
}

fn should_update_fields(
    existing: &Article,
    new: &NewArticle,
    original_url: Option<&str>,
    feed_hash: &str,
    blocked_tracking_pixels: u32,
) -> bool {
    if existing.feed_content_hash.is_none() || feed_content_changed(existing, feed_hash) {
        return true;
    }
    if existing.feed_blocked_tracking_pixels
        != i32::try_from(blocked_tracking_pixels).unwrap_or(i32::MAX)
    {
        return true;
    }
    if existing.title != new.title
        || existing.author != new.author
        || existing.original_url != original_url.unwrap_or(&new.url)
        || existing.comments_url != new.comments_url
        || existing.feed_excerpt != new.excerpt
        || existing.image_url != new.image_url
    {
        return true;
    }
    if let (Some(np), Some(ep)) = (new.published_at, existing.published_at) {
        if (np - ep).whole_seconds().abs() > 60 {
            return true;
        }
    }
    false
}

/// Insert, update, or skip an article. Returns the row and which action ran.
pub async fn upsert(pool: &PgPool, new: &NewArticle) -> Result<(Article, UpsertAction)> {
    upsert_internal(pool, new, None, 0).await
}

/// Insert, update, or skip an article while preserving the publisher-provided
/// URL separately from the canonical shared deduplication URL.
pub async fn upsert_with_original_url(
    pool: &PgPool,
    new: &NewArticle,
    original_url: Option<&str>,
) -> Result<(Article, UpsertAction)> {
    upsert_internal(pool, new, original_url, 0).await
}

/// Upsert feed content while preserving shared tracker-cleanup metadata.
pub async fn upsert_with_original_url_and_privacy(
    pool: &PgPool,
    new: &NewArticle,
    original_url: Option<&str>,
    blocked_tracking_pixels: u32,
) -> Result<(Article, UpsertAction)> {
    upsert_internal(pool, new, original_url, blocked_tracking_pixels).await
}

async fn upsert_internal(
    pool: &PgPool,
    new: &NewArticle,
    original_url: Option<&str>,
    blocked_tracking_pixels: u32,
) -> Result<(Article, UpsertAction)> {
    let (article, mut action) = if let Some(existing) = find_duplicate(pool, new).await? {
        let feed = feed_content(new);
        let total_blocked = blocked_tracking_pixels.saturating_add(feed.blocked_tracking_pixels);
        let feed_hash = content_hash(&feed.html);
        if should_update_fields(&existing, new, original_url, &feed_hash, total_blocked) {
            let updated = update(pool, &existing, new, original_url, feed, total_blocked).await?;
            (updated, UpsertAction::Updated)
        } else {
            (existing, UpsertAction::Skipped)
        }
    } else {
        match create(pool, new, original_url, blocked_tracking_pixels).await {
            Ok(article) => (article, UpsertAction::Created),
            // Lost a race on the unique URL: another writer created it first.
            Err(DbError::Sqlx(sqlx::Error::Database(db_err))) if db_err.is_unique_violation() => {
                let article = find_by_url(pool, &new.url)
                    .await?
                    .ok_or(DbError::NotFound)?;
                (article, UpsertAction::Skipped)
            }
            Err(e) => return Err(e),
        }
    };

    if sync_enclosures(pool, article.id, &new.enclosures).await? && action == UpsertAction::Skipped
    {
        action = UpsertAction::Updated;
    }
    Ok((article, action))
}

async fn sync_enclosures(
    pool: &PgPool,
    article_id: Uuid,
    enclosures: &[NewArticleEnclosure],
) -> Result<bool> {
    let enclosures = &enclosures[..enclosures.len().min(MAX_ARTICLE_ENCLOSURES)];
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT id FROM articles WHERE id = $1 FOR UPDATE")
        .bind(article_id)
        .execute(&mut *tx)
        .await?;
    let current: Vec<ArticleEnclosure> = sqlx::query_as(
        "SELECT article_id, position, url, media_kind, mime_type, title, size_bytes, \
         duration_seconds, width, height FROM article_enclosures \
         WHERE article_id = $1 ORDER BY position",
    )
    .bind(article_id)
    .fetch_all(&mut *tx)
    .await?;
    let unchanged = current.len() == enclosures.len()
        && current
            .iter()
            .zip(enclosures)
            .enumerate()
            .all(|(position, (stored, incoming))| {
                stored.position == position as i32
                    && stored.url == incoming.url
                    && stored.media_kind == incoming.media_kind
                    && stored.mime_type == incoming.mime_type
                    && stored.title == incoming.title
                    && stored.size_bytes == incoming.size_bytes
                    && stored.duration_seconds == incoming.duration_seconds
                    && stored.width == incoming.width
                    && stored.height == incoming.height
            });
    if unchanged {
        tx.commit().await?;
        return Ok(false);
    }

    sqlx::query("DELETE FROM article_enclosures WHERE article_id = $1")
        .bind(article_id)
        .execute(&mut *tx)
        .await?;
    for (position, enclosure) in enclosures.iter().enumerate() {
        sqlx::query(
            "INSERT INTO article_enclosures \
             (article_id, position, url, media_kind, mime_type, title, size_bytes, \
              duration_seconds, width, height) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(article_id)
        .bind(position as i32)
        .bind(&enclosure.url)
        .bind(&enclosure.media_kind)
        .bind(&enclosure.mime_type)
        .bind(&enclosure.title)
        .bind(enclosure.size_bytes)
        .bind(enclosure.duration_seconds)
        .bind(enclosure.width)
        .bind(enclosure.height)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

async fn create(
    pool: &PgPool,
    new: &NewArticle,
    original_url: Option<&str>,
    blocked_tracking_pixels: u32,
) -> Result<Article> {
    let feed = feed_content(new);
    let feed_content = feed.html;
    let feed_hash = content_hash(&feed_content);
    let blocked_tracking_pixels =
        blocked_tracking_pixels.saturating_add(feed.blocked_tracking_pixels);
    let sql = format!(
        "INSERT INTO articles \
         (id, feed_id, url, original_url, comments_url, guid, title, author, content, excerpt, content_hash, \
          feed_content, feed_excerpt, feed_content_hash, feed_blocked_tracking_pixels, content_source, content_revision, \
          image_url, published_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $9, $10, $11, $12, 'feed', 1, $13, $14) \
         RETURNING {ARTICLE_COLUMNS}"
    );
    Ok(sqlx::query_as::<_, Article>(&sql)
        .bind(Uuid::now_v7())
        .bind(new.feed_id)
        .bind(&new.url)
        .bind(original_url.unwrap_or(&new.url))
        .bind(&new.comments_url)
        .bind(&new.guid)
        .bind(&new.title)
        .bind(&new.author)
        .bind(&feed_content)
        .bind(&new.excerpt)
        .bind(&feed_hash)
        .bind(i32::try_from(blocked_tracking_pixels).unwrap_or(i32::MAX))
        .bind(&new.image_url)
        .bind(new.published_at)
        .fetch_one(pool)
        .await?)
}

async fn update(
    pool: &PgPool,
    existing: &Article,
    new: &NewArticle,
    original_url: Option<&str>,
    feed: SanitizedArticleHtml,
    blocked_tracking_pixels: u32,
) -> Result<Article> {
    let feed_content = feed.html;
    let feed_hash = content_hash(&feed_content);
    let source_changed = feed_content_changed(existing, &feed_hash);
    let fallback_changed = source_changed
        || (existing.content_source == "feed" && existing.feed_excerpt != new.excerpt);
    let effective_changed = fallback_changed || existing.title != new.title;
    let sql = format!(
        "UPDATE articles SET title = $2, author = $3, original_url = $4, comments_url = $5, feed_content = $6, \
         feed_excerpt = $7, feed_content_hash = $8, feed_blocked_tracking_pixels = $9, image_url = $10, published_at = $11, \
         content = CASE WHEN $12 THEN $6 ELSE content END, \
         excerpt = CASE WHEN $12 THEN $7 ELSE excerpt END, \
         content_hash = CASE WHEN $12 THEN $8 ELSE content_hash END, \
         content_source = CASE WHEN $12 THEN 'feed' ELSE content_source END, \
         article_page_content = CASE WHEN $12 THEN NULL ELSE article_page_content END, \
         article_page_excerpt = CASE WHEN $12 THEN NULL ELSE article_page_excerpt END, \
         article_page_content_hash = CASE WHEN $12 THEN NULL ELSE article_page_content_hash END, \
         article_page_fetched_at = CASE WHEN $12 THEN NULL ELSE article_page_fetched_at END, \
         article_page_expires_at = CASE WHEN $12 THEN NULL ELSE article_page_expires_at END, \
         article_page_processing_version = CASE WHEN $12 THEN 0 ELSE article_page_processing_version END, \
         article_page_blocked_tracking_pixels = CASE WHEN $12 THEN 0 ELSE article_page_blocked_tracking_pixels END, \
         extraction_status = CASE WHEN $12 THEN 'none' ELSE extraction_status END, \
         extraction_error = CASE WHEN $12 THEN NULL ELSE extraction_error END, \
         content_revision = CASE WHEN $13 THEN content_revision + 1 ELSE content_revision END, \
         summary = CASE WHEN $13 THEN NULL ELSE summary END, \
         key_points = CASE WHEN $13 THEN NULL ELSE key_points END, \
         topics = CASE WHEN $13 THEN '{{}}' ELSE topics END, \
         summarized_at = CASE WHEN $13 THEN NULL ELSE summarized_at END, \
         summary_model = CASE WHEN $13 THEN NULL ELSE summary_model END, updated_at = now() \
         WHERE id = $1 RETURNING {ARTICLE_COLUMNS}"
    );
    Ok(sqlx::query_as::<_, Article>(&sql)
        .bind(existing.id)
        .bind(&new.title)
        .bind(&new.author)
        .bind(original_url.unwrap_or(&new.url))
        .bind(&new.comments_url)
        .bind(&feed_content)
        .bind(&new.excerpt)
        .bind(&feed_hash)
        .bind(i32::try_from(blocked_tracking_pixels).unwrap_or(i32::MAX))
        .bind(&new.image_url)
        .bind(new.published_at)
        .bind(fallback_changed)
        .bind(effective_changed)
        .fetch_one(pool)
        .await?)
}

/// List a feed's articles, newest first.
pub async fn list_for_feed(pool: &PgPool, feed_id: Uuid, limit: i64) -> Result<Vec<Article>> {
    let sql = format!(
        "SELECT {ARTICLE_COLUMNS} FROM articles WHERE feed_id = $1 \
         ORDER BY published_at DESC NULLS LAST, created_at DESC, id LIMIT $2"
    );
    Ok(sqlx::query_as::<_, Article>(&sql)
        .bind(feed_id)
        .bind(limit)
        .fetch_all(pool)
        .await?)
}

/// Mark existing shared public-page copies for one deduplicated re-extraction
/// after administrator-owned processing rules change. RSS fallbacks remain
/// available while the queue catches up.
pub async fn invalidate_article_page_processing_for_feed(
    pool: &PgPool,
    feed_id: Uuid,
) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE articles SET article_page_processing_version = 0, updated_at = now() \
         WHERE feed_id = $1 AND article_page_content IS NOT NULL \
         AND article_page_processing_version <> 0",
    )
    .bind(feed_id)
    .execute(pool)
    .await?
    .rows_affected())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LibraryFilter<'a> {
    pub feed_id: Option<Uuid>,
    pub category_id: Option<Uuid>,
    pub unfiled: bool,
    pub tag: Option<&'a str>,
    /// Hide articles the user has already opened. Independent of read state.
    pub unseen_only: bool,
}

/// A row for the reading list: shared article fields plus this user's personal
/// flags and the source feed name.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ArticleListItem {
    pub id: Uuid,
    pub feed_id: Uuid,
    pub title: String,
    pub url: String,
    pub author: Option<String>,
    pub excerpt: Option<String>,
    pub image_url: Option<String>,
    pub published_at: Option<time::OffsetDateTime>,
    pub feed_name: String,
    pub is_read: bool,
    pub is_seen: bool,
    pub is_bookmarked: bool,
    pub has_summary: bool,
    pub story_count: i32,
    pub feedback_value: Option<f64>,
    pub subscription_settings: Option<serde_json::Value>,
}

/// List a user's reading feed: articles from their subscribed feeds, newest
/// first, annotated with per-user read/bookmark flags. Optionally filter to one
/// `feed_id`; `unread_only` hides already-read articles; `summary_only` hides
/// articles that don't have a ready AI summary yet (so a reader who queued
/// several summaries and moved on can jump straight to the ones now ready).
///
/// Order is `published_at DESC NULLS LAST, created_at DESC, id`. The page is
/// chosen first from each subscribed feed's newest `limit + offset` rows
/// (served by `articles_feed_published_nl_idx`) with every filter applied, and
/// only the chosen rows are decorated with personal state (FND-234).
pub async fn list_for_user(
    pool: &PgPool,
    user_id: Uuid,
    filter: LibraryFilter<'_>,
    unread_only: bool,
    summary_only: bool,
    limit: i64,
    offset: i64,
) -> Result<Vec<ArticleListItem>> {
    let sql = format!(
        "WITH RECURSIVE category_scope(id) AS ( \
             SELECT id FROM user_categories WHERE id = $3::uuid AND user_id = $1 \
             UNION \
             SELECT child.id FROM user_categories child \
             JOIN category_scope scope ON child.parent_id = scope.id \
             WHERE child.user_id = $1 \
         ), \
         page AS ( \
             SELECT candidate.id, candidate.feed_id, candidate.title, candidate.url, \
                    candidate.author, candidate.excerpt, candidate.image_url, \
                    candidate.published_at, candidate.created_at, candidate.has_summary, \
                    scoped.id AS user_feed_id \
             FROM user_feeds scoped \
             CROSS JOIN LATERAL ( \
                 SELECT a.id, a.feed_id, a.title, a.url, a.author, a.excerpt, a.image_url, \
                        a.published_at, a.created_at, \
                        (a.summary IS NOT NULL AND a.summarized_at IS NOT NULL) AS has_summary \
                 FROM articles a \
                 WHERE a.feed_id = scoped.feed_id \
                   AND (NOT $8 OR (a.summary IS NOT NULL AND a.summarized_at IS NOT NULL)) \
                   {seen_filter} {read_filter} \
                 ORDER BY a.published_at DESC NULLS LAST, a.created_at DESC, a.id \
                 LIMIT $9 \
             ) candidate \
             WHERE scoped.user_id = $1 \
               AND ($2::uuid IS NULL OR scoped.feed_id = $2) \
               AND ($3::uuid IS NULL OR EXISTS (SELECT 1 FROM user_feed_categories ufc \
                    JOIN category_scope scope ON scope.id = ufc.user_category_id \
                    WHERE ufc.user_feed_id = scoped.id)) \
               AND (NOT $4 OR NOT EXISTS (SELECT 1 FROM user_feed_categories ufc \
                    WHERE ufc.user_feed_id = scoped.id)) \
               AND ($5::text IS NULL OR scoped.tags @> ARRAY[$5]::text[] \
                   OR EXISTS (SELECT 1 FROM unnest(scoped.tags) stored_tag \
                      WHERE lower(stored_tag) = lower($5))) \
             ORDER BY candidate.published_at DESC NULLS LAST, candidate.created_at DESC, \
                      candidate.id \
             LIMIT $6 OFFSET $7 \
         ) \
         SELECT p.id, p.feed_id, p.title, p.url, p.author, p.excerpt, p.image_url, p.published_at, \
                coalesce(nullif(uf.custom_name, ''), f.name) AS feed_name, \
                (r.article_id IS NOT NULL) AS is_read, \
                (seen.article_id IS NOT NULL) AS is_seen, \
                (b.article_id IS NOT NULL) AS is_bookmarked, \
                p.has_summary, \
                coalesce(sc.member_count, 0) AS story_count, \
                af.feedback_value, uf.settings AS subscription_settings \
         FROM page p \
         JOIN feeds f ON f.id = p.feed_id \
         JOIN user_feeds uf ON uf.id = p.user_feed_id \
         LEFT JOIN read_articles r ON r.article_id = p.id AND r.user_id = $1 \
         LEFT JOIN seen_articles seen ON seen.article_id = p.id AND seen.user_id = $1 \
         LEFT JOIN bookmarks b ON b.article_id = p.id AND b.user_id = $1 \
         LEFT JOIN article_feedback af ON af.article_id = p.id AND af.user_id = $1 \
         LEFT JOIN story_cluster_members scm ON scm.article_id = p.id \
         LEFT JOIN story_clusters sc ON sc.id = scm.cluster_id \
         ORDER BY p.published_at DESC NULLS LAST, p.created_at DESC, p.id",
        seen_filter = if filter.unseen_only {
            "AND NOT EXISTS (SELECT 1 FROM seen_articles s \
             WHERE s.user_id = $1 AND s.article_id = a.id)"
        } else {
            ""
        },
        read_filter = if unread_only {
            "AND NOT EXISTS (SELECT 1 FROM read_articles r \
             WHERE r.user_id = $1 AND r.article_id = a.id)"
        } else {
            ""
        }
    );
    Ok(sqlx::query_as::<_, ArticleListItem>(&sql)
        .bind(user_id)
        .bind(filter.feed_id)
        .bind(filter.category_id)
        .bind(filter.unfiled)
        .bind(filter.tag)
        .bind(limit)
        .bind(offset)
        .bind(summary_only)
        .bind(limit.saturating_add(offset.max(0)))
        .fetch_all(pool)
        .await?)
}

pub async fn count(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM articles")
        .fetch_one(pool)
        .await?)
}

/// Count unread articles per subscribed feed for a user. "Unread" means the
/// article has no matching row in `read_articles`. Feeds with zero unread are
/// omitted, so the caller should treat a missing feed as `0`.
pub async fn unread_counts_by_feed(pool: &PgPool, user_id: Uuid) -> Result<Vec<(Uuid, i64)>> {
    Ok(sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT a.feed_id, count(*)::bigint \
         FROM articles a \
         JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
         LEFT JOIN read_articles r ON r.article_id = a.id AND r.user_id = $1 \
         WHERE r.article_id IS NULL \
         GROUP BY a.feed_id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

/// Count articles with a given URL (0 or 1). Handy for dedup assertions.
pub async fn count_by_url(pool: &PgPool, url: &str) -> Result<i64> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM articles WHERE url = $1")
            .bind(url)
            .fetch_one(pool)
            .await?,
    )
}
