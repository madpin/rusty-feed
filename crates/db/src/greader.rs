//! Stable protocol identifiers and read models for the Google Reader adapter.
//!
//! The aliases are shared because feeds and articles are shared. Every query
//! that exposes content still joins through `user_feeds`, keeping authorization
//! and mutable state personal.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Subscription {
    pub feed_id: Uuid,
    pub greader_id: i64,
    pub title: String,
    pub url: String,
    pub site_url: Option<String>,
    pub image_url: Option<String>,
    pub category_name: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Item {
    pub article_id: Uuid,
    pub greader_id: i64,
    pub feed_greader_id: i64,
    pub feed_title: String,
    pub feed_site_url: Option<String>,
    pub title: String,
    pub url: String,
    pub author: Option<String>,
    pub content: String,
    pub published_at: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
    pub is_read: bool,
    pub is_bookmarked: bool,
    pub category_name: Option<String>,
}

/// List the caller's subscriptions with their single personal folder, if any.
pub async fn subscriptions(pool: &PgPool, user_id: Uuid) -> Result<Vec<Subscription>> {
    Ok(sqlx::query_as(
        "SELECT f.id AS feed_id, gid.greader_id, \
                coalesce(nullif(uf.custom_name, ''), f.name) AS title, \
                f.url, f.site_url, f.image_url, category.name AS category_name \
         FROM user_feeds uf JOIN feeds f ON f.id = uf.feed_id \
         JOIN greader_feed_ids gid ON gid.feed_id = f.id \
         LEFT JOIN LATERAL (SELECT c.name FROM user_feed_categories ufc \
             JOIN user_categories c ON c.id = ufc.user_category_id AND c.user_id = uf.user_id \
             WHERE ufc.user_feed_id = uf.id ORDER BY ufc.created_at, c.name LIMIT 1) category ON true \
         WHERE uf.user_id = $1 ORDER BY title, gid.greader_id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

/// Resolve a protocol feed ID while proving the caller subscribes to it.
pub async fn resolve_feed(pool: &PgPool, user_id: Uuid, id: i64) -> Result<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT gid.feed_id FROM greader_feed_ids gid \
         JOIN user_feeds uf ON uf.feed_id = gid.feed_id AND uf.user_id = $1 \
         WHERE gid.greader_id = $2",
    )
    .bind(user_id)
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// Resolve protocol item IDs while proving every result is visible to the caller.
pub async fn resolve_articles(pool: &PgPool, user_id: Uuid, ids: &[i64]) -> Result<Vec<Uuid>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT gid.article_id FROM greader_article_ids gid \
         JOIN articles a ON a.id = gid.article_id \
         JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
         WHERE gid.greader_id = ANY($2)",
    )
    .bind(user_id)
    .bind(ids)
    .fetch_all(pool)
    .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn item_ids(
    pool: &PgPool,
    user_id: Uuid,
    feed_id: Option<Uuid>,
    only_read: bool,
    only_starred: bool,
    unread_only: bool,
    after: Option<OffsetDateTime>,
    before: Option<OffsetDateTime>,
    ascending: bool,
    limit: i64,
    offset: i64,
) -> Result<Vec<i64>> {
    let order = if ascending { "ASC" } else { "DESC" };
    let sql = format!(
        "SELECT gid.greader_id FROM articles a \
         JOIN greader_article_ids gid ON gid.article_id = a.id \
         JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
         LEFT JOIN read_articles r ON r.article_id = a.id AND r.user_id = $1 \
         LEFT JOIN bookmarks b ON b.article_id = a.id AND b.user_id = $1 \
         WHERE ($2::uuid IS NULL OR a.feed_id = $2) \
           AND (NOT $3 OR r.article_id IS NOT NULL) \
           AND (NOT $4 OR b.article_id IS NOT NULL) \
           AND (NOT $5 OR r.article_id IS NULL) \
           AND ($6::timestamptz IS NULL OR coalesce(a.published_at, a.created_at) > $6) \
           AND ($7::timestamptz IS NULL OR coalesce(a.published_at, a.created_at) < $7) \
         ORDER BY coalesce(a.published_at, a.created_at) {order}, gid.greader_id {order} \
         LIMIT $8 OFFSET $9"
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(feed_id)
        .bind(only_read)
        .bind(only_starred)
        .bind(unread_only)
        .bind(after)
        .bind(before)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?)
}

/// Load selected visible items in the caller-requested direction.
pub async fn items(
    pool: &PgPool,
    user_id: Uuid,
    ids: &[i64],
    ascending: bool,
) -> Result<Vec<Item>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let order = if ascending { "ASC" } else { "DESC" };
    let sql = format!(
        "SELECT a.id AS article_id, aid.greader_id, fid.greader_id AS feed_greader_id, \
                coalesce(nullif(uf.custom_name, ''), f.name) AS feed_title, \
                f.site_url AS feed_site_url, a.title, a.url, a.author, a.content, \
                a.published_at, a.updated_at, (r.article_id IS NOT NULL) AS is_read, \
                (b.article_id IS NOT NULL) AS is_bookmarked, category.name AS category_name \
         FROM greader_article_ids aid JOIN articles a ON a.id = aid.article_id \
         JOIN feeds f ON f.id = a.feed_id JOIN greader_feed_ids fid ON fid.feed_id = f.id \
         JOIN user_feeds uf ON uf.feed_id = f.id AND uf.user_id = $1 \
         LEFT JOIN read_articles r ON r.article_id = a.id AND r.user_id = $1 \
         LEFT JOIN bookmarks b ON b.article_id = a.id AND b.user_id = $1 \
         LEFT JOIN LATERAL (SELECT c.name FROM user_feed_categories ufc \
             JOIN user_categories c ON c.id = ufc.user_category_id AND c.user_id = uf.user_id \
             WHERE ufc.user_feed_id = uf.id ORDER BY ufc.created_at, c.name LIMIT 1) category ON true \
         WHERE aid.greader_id = ANY($2) \
         ORDER BY coalesce(a.published_at, a.created_at) {order}, aid.greader_id {order}"
    );
    Ok(sqlx::query_as(&sql)
        .bind(user_id)
        .bind(ids)
        .fetch_all(pool)
        .await?)
}
