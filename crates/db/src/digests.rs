//! Personal daily briefings assembled from shared story summaries.

use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use rusty_feed_core::reading::{format_daily_digest, BriefingStory};

use crate::error::Result;
use crate::stories;

/// A stored personal briefing.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Digest {
    pub id: Uuid,
    pub title: String,
    pub body: String,
    pub period_start: Date,
    pub created_at: OffsetDateTime,
    pub emailed_at: Option<OffsetDateTime>,
}

/// A briefing claimed for one delivery attempt.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DigestMail {
    pub id: Uuid,
    pub email: String,
    pub title: String,
    pub body: String,
}

/// Build or refresh today's briefing for one user from stories they can read.
/// Returns `None` when the user has no multi-source story in the window.
pub async fn write_today(pool: &PgPool, user_id: Uuid) -> Result<Option<Digest>> {
    let stories = stories::briefing_stories(pool, user_id).await?;
    if stories.is_empty() {
        return Ok(None);
    }
    let today = OffsetDateTime::now_utc().date();
    write_for_period(pool, user_id, today, &stories)
        .await
        .map(Some)
}

/// Users who asked for a daily briefing. Demo accounts are excluded.
pub async fn users_wanting_daily(pool: &PgPool, limit: i64) -> Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT users.id FROM users \
         JOIN user_preferences ON user_preferences.user_id = users.id \
         WHERE users.is_demo = false \
           AND coalesce(user_preferences.preferences->>'daily_digest', 'false') = 'true' \
         ORDER BY users.created_at \
         LIMIT $1",
    )
    .bind(limit.clamp(1, 2_000))
    .fetch_all(pool)
    .await?)
}

/// Recent briefings for the signed-in user, newest first.
pub async fn list_for_user(pool: &PgPool, user_id: Uuid, limit: i64) -> Result<Vec<Digest>> {
    Ok(sqlx::query_as(
        "SELECT id, title, body, period_start, created_at, emailed_at \
         FROM digests WHERE user_id = $1 AND kind = 'daily' \
         ORDER BY period_start DESC LIMIT $2",
    )
    .bind(user_id)
    .bind(limit.clamp(1, 30))
    .fetch_all(pool)
    .await?)
}

/// Claim one unsent briefing whose owner asked for email. The claim is the
/// only send permit; release it if delivery fails.
pub async fn claim_pending_email(pool: &PgPool) -> Result<Option<DigestMail>> {
    Ok(sqlx::query_as(
        "WITH candidate AS ( \
             SELECT digests.id FROM digests \
             JOIN users ON users.id = digests.user_id \
             JOIN user_preferences ON user_preferences.user_id = users.id \
             WHERE digests.kind = 'daily' \
               AND digests.period_start = CURRENT_DATE \
               AND digests.emailed_at IS NULL \
               AND users.is_demo = false \
               AND coalesce(user_preferences.preferences->>'digest_email', 'false') = 'true' \
             ORDER BY digests.created_at \
             LIMIT 1 \
             FOR UPDATE OF digests SKIP LOCKED \
         ) \
         UPDATE digests SET emailed_at = now() \
         FROM candidate, users \
         WHERE digests.id = candidate.id AND users.id = digests.user_id \
         RETURNING digests.id, users.email, digests.title, digests.body",
    )
    .fetch_optional(pool)
    .await?)
}

/// Return a claimed briefing to the unsent queue after a delivery failure.
pub async fn release_email_claim(pool: &PgPool, digest_id: Uuid) -> Result<()> {
    sqlx::query("UPDATE digests SET emailed_at = NULL WHERE id = $1")
        .bind(digest_id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn write_for_period(
    pool: &PgPool,
    user_id: Uuid,
    period: Date,
    stories: &[BriefingStory],
) -> Result<Digest> {
    let label = period.to_string();
    let (title, body) = format_daily_digest(&label, stories);
    let id = Uuid::now_v7();
    Ok(sqlx::query_as(
        "INSERT INTO digests (id, user_id, kind, period_start, title, body) \
         VALUES ($1, $2, 'daily', $3, $4, $5) \
         ON CONFLICT (user_id, kind, period_start) DO UPDATE SET \
           title = EXCLUDED.title, \
           body = EXCLUDED.body, \
           emailed_at = CASE \
             WHEN digests.body IS NOT DISTINCT FROM EXCLUDED.body THEN digests.emailed_at \
             ELSE NULL END \
         RETURNING id, title, body, period_start, created_at, emailed_at",
    )
    .bind(id)
    .bind(user_id)
    .bind(period)
    .bind(title)
    .bind(body)
    .fetch_one(pool)
    .await?)
}
