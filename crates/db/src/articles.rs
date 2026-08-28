//! Article repository — shared articles with ingest deduplication.
//!
//! Dedup order (docs/ARCHITECTURE.md §11.1): GUID (feed-scoped) -> URL (global)
//! -> content hash (feed-scoped). [`upsert`] creates, updates, or skips, keeping
//! exactly one row per article regardless of how many users/feeds reference it.

use rustss_core::hash::content_hash;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DbError, Result};
use crate::models::{Article, NewArticle, UpsertAction, ARTICLE_COLUMNS};

/// Find an article by id.
pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Article>> {
    let sql = format!("SELECT {ARTICLE_COLUMNS} FROM articles WHERE id = $1");
    Ok(sqlx::query_as::<_, Article>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?)
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
    let sql =
        format!("SELECT {ARTICLE_COLUMNS} FROM articles WHERE feed_id = $1 AND content_hash = $2");
    Ok(sqlx::query_as::<_, Article>(&sql)
        .bind(feed_id)
        .bind(hash)
        .fetch_optional(pool)
        .await?)
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
        let hash = content_hash(&new.content);
        if let Some(a) = find_by_feed_and_hash(pool, new.feed_id, &hash).await? {
            return Ok(Some(a));
        }
    }
    Ok(None)
}

/// Whether an existing article should be updated from `new`: the content hash
/// changed, the title changed, or the publish time moved by more than 60s.
pub fn should_update(existing: &Article, new: &NewArticle) -> bool {
    let new_hash = content_hash(&new.content);
    if let Some(existing_hash) = existing.content_hash.as_deref() {
        if existing_hash != new_hash {
            return true;
        }
    }
    if existing.title != new.title {
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
    if let Some(existing) = find_duplicate(pool, new).await? {
        if should_update(&existing, new) {
            let updated = update(pool, existing.id, new).await?;
            return Ok((updated, UpsertAction::Updated));
        }
        return Ok((existing, UpsertAction::Skipped));
    }

    match create(pool, new).await {
        Ok(article) => Ok((article, UpsertAction::Created)),
        // Lost a race on the unique URL: another writer created it first.
        Err(DbError::Sqlx(sqlx::Error::Database(db_err))) if db_err.is_unique_violation() => {
            let article = find_by_url(pool, &new.url)
                .await?
                .ok_or(DbError::NotFound)?;
            Ok((article, UpsertAction::Skipped))
        }
        Err(e) => Err(e),
    }
}

async fn create(pool: &PgPool, new: &NewArticle) -> Result<Article> {
    let hash = content_hash(&new.content);
    let sql = format!(
        "INSERT INTO articles \
         (id, feed_id, url, guid, title, author, content, excerpt, content_hash, image_url, published_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING {ARTICLE_COLUMNS}"
    );
    Ok(sqlx::query_as::<_, Article>(&sql)
        .bind(Uuid::now_v7())
        .bind(new.feed_id)
        .bind(&new.url)
        .bind(&new.guid)
        .bind(&new.title)
        .bind(&new.author)
        .bind(&new.content)
        .bind(&new.excerpt)
        .bind(&hash)
        .bind(&new.image_url)
        .bind(new.published_at)
        .fetch_one(pool)
        .await?)
}

async fn update(pool: &PgPool, id: Uuid, new: &NewArticle) -> Result<Article> {
    let hash = content_hash(&new.content);
    let sql = format!(
        "UPDATE articles SET title = $2, author = $3, content = $4, excerpt = $5, \
         content_hash = $6, image_url = $7, published_at = $8, updated_at = now() \
         WHERE id = $1 RETURNING {ARTICLE_COLUMNS}"
    );
    Ok(sqlx::query_as::<_, Article>(&sql)
        .bind(id)
        .bind(&new.title)
        .bind(&new.author)
        .bind(&new.content)
        .bind(&new.excerpt)
        .bind(&hash)
        .bind(&new.image_url)
        .bind(new.published_at)
        .fetch_one(pool)
        .await?)
}

/// List a feed's articles, newest first.
pub async fn list_for_feed(pool: &PgPool, feed_id: Uuid, limit: i64) -> Result<Vec<Article>> {
    let sql = format!(
        "SELECT {ARTICLE_COLUMNS} FROM articles WHERE feed_id = $1 \
         ORDER BY published_at DESC NULLS LAST, created_at DESC LIMIT $2"
    );
    Ok(sqlx::query_as::<_, Article>(&sql)
        .bind(feed_id)
        .bind(limit)
        .fetch_all(pool)
        .await?)
}

/// A row for the reading list: shared article fields plus this user's personal
/// flags and the source feed name.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ArticleListItem {
    pub id: Uuid,
    pub title: String,
    pub url: String,
    pub excerpt: Option<String>,
    pub image_url: Option<String>,
    pub published_at: Option<time::OffsetDateTime>,
    pub feed_name: String,
    pub is_read: bool,
    pub is_bookmarked: bool,
}

/// List a user's reading feed: articles from their subscribed feeds, newest
/// first, annotated with per-user read/bookmark flags. Optionally filter to one
/// `feed_id`; `unread_only` hides already-read articles.
pub async fn list_for_user(
    pool: &PgPool,
    user_id: Uuid,
    feed_id: Option<Uuid>,
    unread_only: bool,
    limit: i64,
    offset: i64,
) -> Result<Vec<ArticleListItem>> {
    let sql = format!(
        "SELECT a.id, a.title, a.url, a.excerpt, a.image_url, a.published_at, \
                f.name AS feed_name, \
                (r.article_id IS NOT NULL) AS is_read, \
                (b.article_id IS NOT NULL) AS is_bookmarked \
         FROM articles a \
         JOIN feeds f ON f.id = a.feed_id \
         JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
         LEFT JOIN read_articles r ON r.article_id = a.id AND r.user_id = $1 \
         LEFT JOIN bookmarks b ON b.article_id = a.id AND b.user_id = $1 \
         WHERE ($2::uuid IS NULL OR a.feed_id = $2) {read_filter} \
         ORDER BY a.published_at DESC NULLS LAST, a.created_at DESC \
         LIMIT $3 OFFSET $4",
        read_filter = if unread_only {
            "AND r.article_id IS NULL"
        } else {
            ""
        }
    );
    Ok(sqlx::query_as::<_, ArticleListItem>(&sql)
        .bind(user_id)
        .bind(feed_id)
        .bind(limit)
        .bind(offset)
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
