//! Per-user article feedback (M10). One row per (user, article); explicit
//! feedback must not be silently overwritten by implicit signals — the caller
//! checks [`existing_type`] before applying an implicit update.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FeedbackRecord {
    pub feedback_type: String,
    pub feedback_value: f64,
}

/// The current feedback type for (user, article), if any.
pub async fn existing_type(
    pool: &PgPool,
    user_id: Uuid,
    article_id: Uuid,
) -> Result<Option<String>> {
    Ok(get(pool, user_id, article_id)
        .await?
        .map(|record| record.feedback_type))
}

pub async fn get(pool: &PgPool, user_id: Uuid, article_id: Uuid) -> Result<Option<FeedbackRecord>> {
    Ok(sqlx::query_as(
        "SELECT feedback_type, feedback_value FROM article_feedback WHERE user_id = $1 AND article_id = $2",
    )
    .bind(user_id)
    .bind(article_id)
    .fetch_optional(pool)
    .await?)
}

/// Insert or replace a feedback row.
pub async fn upsert(
    pool: &PgPool,
    user_id: Uuid,
    article_id: Uuid,
    feedback_type: &str,
    value: f64,
    time_spent: Option<i32>,
    estimated: Option<i32>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO article_feedback \
           (id, user_id, article_id, feedback_type, feedback_value, time_spent_seconds, estimated_time_seconds) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (user_id, article_id) DO UPDATE SET \
           feedback_type = EXCLUDED.feedback_type, feedback_value = EXCLUDED.feedback_value, \
           time_spent_seconds = EXCLUDED.time_spent_seconds, \
           estimated_time_seconds = EXCLUDED.estimated_time_seconds, updated_at = now()",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(article_id)
    .bind(feedback_type)
    .bind(value)
    .bind(time_spent)
    .bind(estimated)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete all of a user's feedback (part of "reset learning").
pub async fn delete_for_user(pool: &PgPool, user_id: Uuid) -> Result<u64> {
    let res = sqlx::query("DELETE FROM article_feedback WHERE user_id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}
