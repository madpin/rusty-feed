use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CatalogRecord {
    pub id: Uuid,
    pub feed_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub category: String,
    pub country: String,
    pub language: String,
    pub featured: bool,
    pub sort_order: i32,
    pub published: bool,
}

#[derive(Debug, Clone)]
pub struct CatalogDraft<'a> {
    pub title: &'a str,
    pub description: Option<&'a str>,
    pub category: &'a str,
    pub country: &'a str,
    pub language: &'a str,
    pub featured: bool,
    pub sort_order: i32,
    pub published: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CatalogEntry {
    pub id: Uuid,
    pub feed_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub category: String,
    pub country: String,
    pub language: String,
    pub featured: bool,
    pub url: String,
    pub health_status: String,
    pub article_count: i64,
    pub average_rss_content_length: Option<f64>,
    pub last_post_at: Option<OffsetDateTime>,
    pub average_interval_hours: Option<f64>,
    pub subscribed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogSubscriptionResult {
    pub attempted: usize,
    pub created: usize,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AdminCatalogEntry {
    pub id: Uuid,
    pub feed_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub category: String,
    pub country: String,
    pub language: String,
    pub featured: bool,
    pub sort_order: i32,
    pub published: bool,
    pub url: String,
    pub health_status: String,
    pub article_count: i64,
    pub average_rss_content_length: Option<f64>,
    pub last_post_at: Option<OffsetDateTime>,
    pub average_interval_hours: Option<f64>,
}

const CATALOG_COLUMNS: &str =
    "id, feed_id, title, description, category, country, language, featured, sort_order, published";
const STATS: &str = "WITH catalog_feed_ids AS ( \
    SELECT feed_id FROM feed_catalog_entries \
  ), source_stats AS ( \
    SELECT a.feed_id, count(*)::bigint AS article_count, \
      (avg(char_length(a.feed_content)) FILTER (WHERE a.feed_content IS NOT NULL))::double precision \
        AS average_rss_content_length, max(a.published_at) AS last_post_at \
    FROM articles a JOIN catalog_feed_ids cf ON cf.feed_id = a.feed_id GROUP BY a.feed_id \
  ), post_deltas AS ( \
    SELECT a.feed_id, extract(epoch FROM (a.published_at - lag(a.published_at) over \
      (PARTITION BY a.feed_id ORDER BY a.published_at))) / 3600.0 AS interval_hours \
    FROM articles a JOIN catalog_feed_ids cf ON cf.feed_id = a.feed_id \
    WHERE a.published_at IS NOT NULL \
  ), cadence AS ( \
    SELECT feed_id, avg(interval_hours)::double precision AS average_interval_hours \
    FROM post_deltas WHERE interval_hours > 0 GROUP BY feed_id \
  ) ";

pub async fn count(pool: &PgPool) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM feed_catalog_entries")
            .fetch_one(pool)
            .await?,
    )
}

pub async fn is_published_feed(pool: &PgPool, feed_id: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM feed_catalog_entries WHERE feed_id = $1 AND published",
    )
    .bind(feed_id)
    .fetch_optional(pool)
    .await?
    .is_some())
}

pub async fn categories(pool: &PgPool) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT category FROM feed_catalog_entries WHERE published ORDER BY category",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn countries(pool: &PgPool) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT country FROM feed_catalog_entries WHERE published ORDER BY country",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn languages(pool: &PgPool) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT language FROM feed_catalog_entries WHERE published ORDER BY language",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn list_published_for_user(
    pool: &PgPool,
    user_id: Uuid,
    category: Option<&str>,
    country: Option<&str>,
    language: Option<&str>,
) -> Result<Vec<CatalogEntry>> {
    let sql = format!(
        "{STATS} \
         SELECT c.id, c.feed_id, c.title, c.description, c.category, c.country, c.language, \
           c.featured, f.url, f.health_status, coalesce(s.article_count, 0)::bigint AS article_count, \
           s.average_rss_content_length, s.last_post_at, d.average_interval_hours, \
           EXISTS(SELECT 1 FROM user_feeds uf WHERE uf.user_id = $1 AND uf.feed_id = c.feed_id) AS subscribed \
         FROM feed_catalog_entries c \
         JOIN feeds f ON f.id = c.feed_id \
         LEFT JOIN source_stats s ON s.feed_id = c.feed_id \
         LEFT JOIN cadence d ON d.feed_id = c.feed_id \
         WHERE c.published AND ($2::text IS NULL OR c.category = $2) \
           AND ($3::text IS NULL OR c.country = $3) \
           AND ($4::text IS NULL OR c.language = $4) \
         ORDER BY c.featured DESC, c.sort_order, c.title"
    );
    Ok(sqlx::query_as::<_, CatalogEntry>(&sql)
        .bind(user_id)
        .bind(category)
        .bind(country)
        .bind(language)
        .fetch_all(pool)
        .await?)
}

pub async fn list_admin(pool: &PgPool) -> Result<Vec<AdminCatalogEntry>> {
    let sql = format!(
        "{STATS} \
         SELECT c.id, c.feed_id, c.title, c.description, c.category, c.country, c.language, \
           c.featured, c.sort_order, c.published, f.url, f.health_status, \
           coalesce(s.article_count, 0)::bigint AS article_count, s.average_rss_content_length, \
           s.last_post_at, d.average_interval_hours \
         FROM feed_catalog_entries c \
         JOIN feeds f ON f.id = c.feed_id \
         LEFT JOIN source_stats s ON s.feed_id = c.feed_id \
         LEFT JOIN cadence d ON d.feed_id = c.feed_id \
         ORDER BY c.published DESC, c.featured DESC, c.sort_order, c.title"
    );
    Ok(sqlx::query_as::<_, AdminCatalogEntry>(&sql)
        .fetch_all(pool)
        .await?)
}

pub async fn subscribe_published_many(
    pool: &PgPool,
    user_id: Uuid,
    feed_ids: &[Uuid],
) -> Result<CatalogSubscriptionResult> {
    if feed_ids.is_empty() {
        return Ok(CatalogSubscriptionResult {
            attempted: 0,
            created: 0,
        });
    }
    let mut transaction = pool.begin().await?;
    let published: Vec<Uuid> = sqlx::query_scalar(
        "SELECT feed_id FROM feed_catalog_entries \
         WHERE published AND feed_id = ANY($1) FOR SHARE",
    )
    .bind(feed_ids)
    .fetch_all(&mut *transaction)
    .await?;
    if published.len() != feed_ids.len() {
        return Err(crate::DbError::NotFound);
    }
    let mut created = 0;
    for feed_id in feed_ids {
        let result = sqlx::query(
            "INSERT INTO user_feeds (id, user_id, feed_id) VALUES ($1, $2, $3) \
             ON CONFLICT (user_id, feed_id) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(feed_id)
        .execute(&mut *transaction)
        .await?;
        created += result.rows_affected() as usize;
    }
    transaction.commit().await?;
    Ok(CatalogSubscriptionResult {
        attempted: feed_ids.len(),
        created,
    })
}

pub async fn upsert(
    pool: &PgPool,
    feed_id: Uuid,
    draft: CatalogDraft<'_>,
) -> Result<CatalogRecord> {
    let sql = format!(
        "INSERT INTO feed_catalog_entries \
           (id, feed_id, title, description, category, country, language, featured, sort_order, published) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
         ON CONFLICT (feed_id) DO UPDATE SET title = EXCLUDED.title, \
           description = EXCLUDED.description, category = EXCLUDED.category, country = EXCLUDED.country, \
           language = EXCLUDED.language, featured = EXCLUDED.featured, sort_order = EXCLUDED.sort_order, \
           published = EXCLUDED.published, updated_at = now() \
         RETURNING {CATALOG_COLUMNS}"
    );
    Ok(sqlx::query_as::<_, CatalogRecord>(&sql)
        .bind(Uuid::now_v7())
        .bind(feed_id)
        .bind(draft.title)
        .bind(draft.description)
        .bind(draft.category)
        .bind(draft.country)
        .bind(draft.language)
        .bind(draft.featured)
        .bind(draft.sort_order)
        .bind(draft.published)
        .fetch_one(pool)
        .await?)
}

pub async fn update(pool: &PgPool, id: Uuid, draft: CatalogDraft<'_>) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE feed_catalog_entries SET title = $2, description = $3, category = $4, country = $5, \
         language = $6, featured = $7, sort_order = $8, published = $9, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(draft.title)
    .bind(draft.description)
    .bind(draft.category)
    .bind(draft.country)
    .bind(draft.language)
    .bind(draft.featured)
    .bind(draft.sort_order)
    .bind(draft.published)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}
