//! Shared Roundup selection and first-writer-wins storage.
//!
//! Every candidate query enters through the caller's `user_feeds` rows. The
//! projection intentionally excludes `custom_name`, folder names, article
//! bodies, and every other personal field.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use rusty_feed_core::roundup::{
    RoundupCandidate, RoundupKey, RoundupPeriod, ROUNDUP_CANDIDATE_LIMIT, ROUNDUP_PROMPT_VERSION,
};

use crate::error::Result;

const CANDIDATE_COLUMNS: &str = "a.id, a.title, f.name AS feed_title, a.url, \
    left(coalesce(nullif(a.summary, ''), nullif(a.excerpt, ''), \
                  nullif(a.feed_excerpt, ''), ''), 1200) AS summary_or_excerpt, \
    a.published_at, a.created_at, coalesce(story.member_count, 1) AS story_count, \
    (a.summary IS NOT NULL AND a.summarized_at IS NOT NULL) AS has_summary";

const CANDIDATE_JOINS: &str = "JOIN articles a ON a.feed_id = scoped.feed_id \
    JOIN feeds f ON f.id = a.feed_id \
    LEFT JOIN story_cluster_members member ON member.article_id = a.id \
        AND member.content_revision = a.content_revision \
    LEFT JOIN story_clusters story ON story.id = member.cluster_id";

/// A personal feed or recursive folder scope used only to choose shared inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundupScope {
    Feed(Uuid),
    Folder(Uuid),
}

impl RoundupScope {
    pub fn id(self) -> Uuid {
        match self {
            Self::Feed(id) | Self::Folder(id) => id,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Feed(_) => "feed",
            Self::Folder(_) => "folder",
        }
    }

    pub fn parse(kind: &str, id: Uuid) -> Option<Self> {
        match kind {
            "feed" => Some(Self::Feed(id)),
            "folder" => Some(Self::Folder(id)),
            _ => None,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct CandidateRecord {
    id: Uuid,
    title: String,
    feed_title: String,
    url: String,
    summary_or_excerpt: String,
    published_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    story_count: i32,
    has_summary: bool,
}

impl From<CandidateRecord> for RoundupCandidate {
    fn from(row: CandidateRecord) -> Self {
        Self {
            id: row.id,
            title: row.title,
            feed_title: row.feed_title,
            url: row.url,
            summary_or_excerpt: row.summary_or_excerpt,
            published_at: row.published_at,
            created_at: row.created_at,
            story_count: row.story_count,
            has_summary: row.has_summary,
        }
    }
}

/// Load the newest bounded candidate window for one subscribed feed or one
/// folder and all its descendants. Ranking happens in `core::roundup`.
pub async fn candidates_for_user(
    pool: &PgPool,
    user_id: Uuid,
    scope: RoundupScope,
    period: RoundupPeriod,
) -> Result<Vec<RoundupCandidate>> {
    let sql = match scope {
        RoundupScope::Feed(_) => format!(
            "WITH scoped(feed_id) AS ( \
                 SELECT feed_id FROM user_feeds WHERE user_id = $1 AND feed_id = $2 \
             ) SELECT {CANDIDATE_COLUMNS} FROM scoped {CANDIDATE_JOINS} \
             WHERE coalesce(a.published_at, a.created_at) >= now() - make_interval(hours => $3::int) \
             ORDER BY coalesce(a.published_at, a.created_at) DESC, a.id LIMIT $4"
        ),
        RoundupScope::Folder(_) => format!(
            "WITH RECURSIVE category_scope(id) AS ( \
                 SELECT id FROM user_categories WHERE id = $2::uuid AND user_id = $1 \
                 UNION ALL \
                 SELECT child.id FROM user_categories child \
                 JOIN category_scope parent ON child.parent_id = parent.id \
                WHERE child.user_id = $1 \
             ), scoped(feed_id) AS ( \
                 SELECT DISTINCT subscription.feed_id FROM user_feeds subscription \
                 JOIN user_feed_categories filed ON filed.user_feed_id = subscription.id \
                 JOIN category_scope scope ON scope.id = filed.user_category_id \
                WHERE subscription.user_id = $1 \
             ) SELECT {CANDIDATE_COLUMNS} FROM scoped {CANDIDATE_JOINS} \
             WHERE coalesce(a.published_at, a.created_at) >= now() - make_interval(hours => $3::int) \
             ORDER BY coalesce(a.published_at, a.created_at) DESC, a.id LIMIT $4"
        ),
    };
    let rows = sqlx::query_as::<_, CandidateRecord>(&sql)
        .bind(user_id)
        .bind(scope.id())
        .bind(period.window_hours())
        .bind(ROUNDUP_CANDIDATE_LIMIT)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Reload only shared fields for the exact IDs frozen into a queued job.
pub async fn candidates_by_ids(pool: &PgPool, ids: &[Uuid]) -> Result<Vec<RoundupCandidate>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "WITH scoped(feed_id) AS (SELECT id FROM feeds) \
         SELECT {CANDIDATE_COLUMNS} FROM scoped {CANDIDATE_JOINS} \
         WHERE a.id = ANY($1)"
    );
    let rows = sqlx::query_as::<_, CandidateRecord>(&sql)
        .bind(ids)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// One stored shared Roundup. `model_version` is the selected cache dimension;
/// `provider`/`model` record the provider that actually completed the cascade.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Roundup {
    pub id: Uuid,
    pub input_hash: String,
    pub period: String,
    pub prompt_version: String,
    pub model_version: String,
    pub provider: String,
    pub model: String,
    pub headline: String,
    pub body_markdown: String,
    pub source_ids: Vec<Uuid>,
    pub created_at: OffsetDateTime,
}

const ROUNDUP_COLUMNS: &str = "id, input_hash, period, prompt_version, model_version, \
    provider, model, headline, body_markdown, source_ids, created_at";

/// Generated shared content ready for idempotent insertion.
pub struct NewRoundup<'a> {
    pub key: &'a RoundupKey,
    pub provider: &'a str,
    pub model: &'a str,
    pub headline: &'a str,
    pub body_markdown: &'a str,
    pub source_ids: &'a [Uuid],
}

/// Find an existing generation for the complete shared cache key.
pub async fn find_by_key(pool: &PgPool, key: &RoundupKey) -> Result<Option<Roundup>> {
    let sql = format!(
        "SELECT {ROUNDUP_COLUMNS} FROM roundups \
         WHERE input_hash = $1 AND period = $2 AND prompt_version = $3 AND model_version = $4"
    );
    Ok(sqlx::query_as(&sql)
        .bind(&key.input_hash)
        .bind(key.period.as_str())
        .bind(ROUNDUP_PROMPT_VERSION)
        .bind(&key.model_version)
        .fetch_optional(pool)
        .await?)
}

/// Insert a generation once. A concurrent or repeat caller gets `None` and can
/// read the winner with [`find_by_key`].
pub async fn insert(pool: &PgPool, new: NewRoundup<'_>) -> Result<Option<Roundup>> {
    let sql = format!(
        "INSERT INTO roundups \
           (id, input_hash, period, prompt_version, model_version, provider, model, \
            headline, body_markdown, source_ids) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
         ON CONFLICT (input_hash, period, prompt_version, model_version) DO NOTHING \
         RETURNING {ROUNDUP_COLUMNS}"
    );
    Ok(sqlx::query_as(&sql)
        .bind(Uuid::now_v7())
        .bind(&new.key.input_hash)
        .bind(new.key.period.as_str())
        .bind(ROUNDUP_PROMPT_VERSION)
        .bind(&new.key.model_version)
        .bind(new.provider)
        .bind(new.model)
        .bind(new.headline)
        .bind(new.body_markdown)
        .bind(new.source_ids)
        .fetch_optional(pool)
        .await?)
}

/// Shared source metadata for the rendered Sources list, in citation order.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RoundupSource {
    pub id: Uuid,
    pub position: i64,
    pub title: String,
    pub feed_title: String,
    pub url: String,
    pub published_at: Option<OffsetDateTime>,
}

pub async fn sources_by_ids(pool: &PgPool, ids: &[Uuid]) -> Result<Vec<RoundupSource>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT article.id, requested.position, article.title, feed.name AS feed_title, article.url, \
                article.published_at \
         FROM unnest($1::uuid[]) WITH ORDINALITY AS requested(id, position) \
         JOIN articles article ON article.id = requested.id \
         JOIN feeds feed ON feed.id = article.feed_id \
         ORDER BY requested.position",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?)
}
