use std::collections::BTreeMap;

use rusty_feed_core::feed_content::SubscriptionContentSettings;
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

/// One catalog subscription and its initial personal content preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogSubscriptionChoice {
    pub feed_id: Uuid,
    pub content: SubscriptionContentSettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSubscriptionResult {
    pub attempted: usize,
    pub created: usize,
    /// Feed ids that were newly subscribed by this call (excludes ones the
    /// caller was already subscribed to), so callers can auto-organize only
    /// the new subscriptions without disturbing existing folders/tags.
    pub created_feed_ids: Vec<Uuid>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentProbeStatus {
    Full,
    Partial,
    Http401,
    Http402,
    Http403,
    Http451,
    RobotsDisallowed,
    RateLimited,
    ReaderFailed,
    FetchFailed,
}

#[derive(Debug, Clone)]
pub struct ContentProbeWrite {
    pub article_id: Uuid,
    pub rss_body_chars: Option<i32>,
    pub processed_body_chars: Option<i32>,
    pub status: ContentProbeStatus,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ContentProbeStats {
    pub sampled_article_count: i16,
    pub average_rss_body_chars: Option<i32>,
    pub average_processed_body_chars: Option<i32>,
    pub full_content_count: i16,
    pub partial_content_count: i16,
    pub http_401_count: i16,
    pub http_402_count: i16,
    pub http_403_count: i16,
    pub http_451_count: i16,
    pub robots_disallowed_count: i16,
    pub rate_limited_count: i16,
    pub reader_failed_count: i16,
    pub fetch_failed_count: i16,
    pub checked_at: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ContentProbeCandidate {
    pub id: Uuid,
    pub url: String,
    pub rss_body_chars: Option<i32>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CatalogPreviewArticle {
    pub id: Uuid,
    pub title: String,
    pub url: String,
    pub feed_content: Option<String>,
    pub published_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CatalogPreviewFeed {
    pub title: String,
    pub description: Option<String>,
    pub category: String,
    pub country: String,
    pub language: String,
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

pub async fn preview_feed(pool: &PgPool, feed_id: Uuid) -> Result<Option<CatalogPreviewFeed>> {
    Ok(sqlx::query_as(
        "SELECT title, description, category, country, language \
         FROM feed_catalog_entries WHERE feed_id = $1 AND published",
    )
    .bind(feed_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn probe_needs_refresh(pool: &PgPool, feed_id: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "WITH current_sample AS ( \
           SELECT coalesce(array_agg(id ORDER BY published_at DESC NULLS LAST, created_at DESC, id), \
             ARRAY[]::uuid[]) AS article_ids \
           FROM (SELECT id, published_at, created_at FROM articles WHERE feed_id = $1 \
                 ORDER BY published_at DESC NULLS LAST, created_at DESC, id LIMIT 3) candidates \
         ) \
         SELECT NOT EXISTS (SELECT 1 FROM catalog_content_probes probe CROSS JOIN current_sample \
           WHERE probe.feed_id = $1 AND probe.candidate_article_ids = current_sample.article_ids \
           AND probe.checked_at > now() - CASE WHEN probe.rate_limited_count > 0 \
             THEN interval '1 day' ELSE interval '7 days' END)",
    )
    .bind(feed_id)
    .fetch_one(pool)
    .await?)
}

pub async fn content_probe_stats(
    pool: &PgPool,
    feed_id: Uuid,
) -> Result<Option<ContentProbeStats>> {
    Ok(sqlx::query_as(
        "SELECT sampled_article_count, average_rss_body_chars, average_processed_body_chars, \
         full_content_count, partial_content_count, http_401_count, http_402_count, http_403_count, http_451_count, robots_disallowed_count, \
         rate_limited_count, reader_failed_count, fetch_failed_count, checked_at \
         FROM catalog_content_probes WHERE feed_id = $1",
    )
    .bind(feed_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn preview_articles(pool: &PgPool, feed_id: Uuid) -> Result<Vec<CatalogPreviewArticle>> {
    Ok(sqlx::query_as(
        "SELECT id, title, url, left(feed_content, 16000) AS feed_content, published_at \
         FROM articles WHERE feed_id = $1 \
         ORDER BY published_at DESC NULLS LAST, created_at DESC, id LIMIT 3",
    )
    .bind(feed_id)
    .fetch_all(pool)
    .await?)
}

pub async fn content_probe_candidates(
    pool: &PgPool,
    feed_id: Uuid,
) -> Result<Vec<ContentProbeCandidate>> {
    Ok(sqlx::query_as(
        "SELECT id, url, char_length(feed_content)::int AS rss_body_chars FROM articles \
         WHERE feed_id = $1 ORDER BY published_at DESC NULLS LAST, created_at DESC, id LIMIT 3",
    )
    .bind(feed_id)
    .fetch_all(pool)
    .await?)
}

pub async fn replace_content_probe(
    pool: &PgPool,
    feed_id: Uuid,
    candidates: &[ContentProbeCandidate],
    samples: &[ContentProbeWrite],
) -> Result<()> {
    let candidates = &candidates[..candidates.len().min(3)];
    let candidate_article_ids = candidates
        .iter()
        .map(|candidate| candidate.id)
        .collect::<Vec<_>>();
    let samples = samples
        .iter()
        .filter(|sample| candidate_article_ids.contains(&sample.article_id))
        .take(3)
        .collect::<Vec<_>>();
    let sample_count = samples.len() as i16;
    let count = |status| {
        samples
            .iter()
            .filter(|sample| sample.status == status)
            .count() as i16
    };
    let lengths: Vec<i32> = candidates
        .iter()
        .filter_map(|candidate| candidate.rss_body_chars)
        .collect();
    let average_rss_body_chars = (!lengths.is_empty()).then(|| {
        (lengths.iter().map(|length| i64::from(*length)).sum::<i64>() / lengths.len() as i64) as i32
    });
    let processed_lengths: Vec<i32> = samples
        .iter()
        .filter_map(|sample| sample.processed_body_chars)
        .collect();
    let average_processed_body_chars = (!processed_lengths.is_empty()).then(|| {
        (processed_lengths
            .iter()
            .map(|length| i64::from(*length))
            .sum::<i64>()
            / processed_lengths.len() as i64) as i32
    });
    sqlx::query(
        "INSERT INTO catalog_content_probes (feed_id, candidate_article_ids, sampled_article_count, \
         average_rss_body_chars, average_processed_body_chars, full_content_count, partial_content_count, \
         http_401_count, http_402_count, http_403_count, http_451_count, robots_disallowed_count, rate_limited_count, \
         reader_failed_count, fetch_failed_count, checked_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, now()) \
         ON CONFLICT (feed_id) DO UPDATE SET candidate_article_ids = EXCLUDED.candidate_article_ids, \
         sampled_article_count = EXCLUDED.sampled_article_count, \
         average_rss_body_chars = EXCLUDED.average_rss_body_chars, \
         average_processed_body_chars = EXCLUDED.average_processed_body_chars, \
         full_content_count = EXCLUDED.full_content_count, partial_content_count = EXCLUDED.partial_content_count, \
         http_401_count = EXCLUDED.http_401_count, http_402_count = EXCLUDED.http_402_count, \
         http_403_count = EXCLUDED.http_403_count, http_451_count = EXCLUDED.http_451_count, \
         robots_disallowed_count = EXCLUDED.robots_disallowed_count, \
         rate_limited_count = EXCLUDED.rate_limited_count, reader_failed_count = EXCLUDED.reader_failed_count, \
         fetch_failed_count = EXCLUDED.fetch_failed_count, checked_at = now()",
    )
    .bind(feed_id)
    .bind(candidate_article_ids)
    .bind(sample_count)
    .bind(average_rss_body_chars)
    .bind(average_processed_body_chars)
    .bind(count(ContentProbeStatus::Full))
    .bind(count(ContentProbeStatus::Partial))
    .bind(count(ContentProbeStatus::Http401))
    .bind(count(ContentProbeStatus::Http402))
    .bind(count(ContentProbeStatus::Http403))
    .bind(count(ContentProbeStatus::Http451))
    .bind(count(ContentProbeStatus::RobotsDisallowed))
    .bind(count(ContentProbeStatus::RateLimited))
    .bind(count(ContentProbeStatus::ReaderFailed))
    .bind(count(ContentProbeStatus::FetchFailed))
    .execute(pool)
    .await?;
    Ok(())
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
    let choices = feed_ids
        .iter()
        .copied()
        .map(|feed_id| CatalogSubscriptionChoice {
            feed_id,
            content: SubscriptionContentSettings::default(),
        })
        .collect::<Vec<_>>();
    subscribe_published_many_with_content(pool, user_id, &choices).await
}

/// Subscribe to published catalog feeds with an initial per-subscription content choice.
pub async fn subscribe_published_many_with_content(
    pool: &PgPool,
    user_id: Uuid,
    choices: &[CatalogSubscriptionChoice],
) -> Result<CatalogSubscriptionResult> {
    let choices = choices
        .iter()
        .map(|choice| (choice.feed_id, choice.content))
        .collect::<BTreeMap<_, _>>();
    if choices.is_empty() {
        return Ok(CatalogSubscriptionResult {
            attempted: 0,
            created: 0,
            created_feed_ids: Vec::new(),
        });
    }
    let feed_ids = choices.keys().copied().collect::<Vec<_>>();
    let mut transaction = pool.begin().await?;
    let published: Vec<Uuid> = sqlx::query_scalar(
        "SELECT feed_id FROM feed_catalog_entries \
         WHERE published AND feed_id = ANY($1) FOR SHARE",
    )
    .bind(&feed_ids)
    .fetch_all(&mut *transaction)
    .await?;
    if published.len() != feed_ids.len() {
        return Err(crate::DbError::NotFound);
    }
    let mut created_feed_ids = Vec::new();
    for (feed_id, content) in &choices {
        let result = sqlx::query(
            "INSERT INTO user_feeds (id, user_id, feed_id, settings) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (user_id, feed_id) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(feed_id)
        .bind(content.to_json())
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() > 0 {
            created_feed_ids.push(*feed_id);
        }
    }
    transaction.commit().await?;
    Ok(CatalogSubscriptionResult {
        attempted: choices.len(),
        created: created_feed_ids.len(),
        created_feed_ids,
    })
}

/// After subscribing a user to catalog feeds, place each **newly created**
/// subscription (never ones the caller already had — that would clobber their
/// existing organization) into a personal folder named after its catalog
/// category, and tag it with that category. Without this, a feed's
/// already-shared articles (probed/fetched before this user ever subscribed)
/// would land "Unfiled" and untagged for the new subscriber even though the
/// catalog already knows what the source is about. Best-effort per feed: a
/// feed with no published catalog entry is skipped rather than failing the
/// whole batch.
pub async fn organize_new_subscriptions(
    pool: &PgPool,
    user_id: Uuid,
    feed_ids: &[Uuid],
) -> Result<()> {
    for &feed_id in feed_ids {
        let category: Option<String> = sqlx::query_scalar(
            "SELECT category FROM feed_catalog_entries WHERE feed_id = $1 AND published",
        )
        .bind(feed_id)
        .fetch_optional(pool)
        .await?;
        let Some(category) = category else { continue };
        let category = category.trim();
        if category.is_empty() {
            continue;
        }
        // Mirror the 40-char tag limit enforced on the manual organize path
        // (crate::subscriptions::validate_tags) so this never produces a tag
        // longer than what the UI would otherwise accept.
        let category: String = category.chars().take(40).collect();
        let folder = crate::categories::create(pool, user_id, &category, None).await?;
        crate::subscriptions::update_organization(
            pool,
            user_id,
            feed_id,
            std::slice::from_ref(&category),
            Some(folder.id),
        )
        .await?;
    }
    Ok(())
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
