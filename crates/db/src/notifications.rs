//! In-app user notifications (M11).

use serde_json::Value as Json;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

/// A notification for display.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Notification {
    pub id: Uuid,
    pub kind: String,
    pub title: String,
    pub message: String,
    pub read: bool,
    pub created_at: OffsetDateTime,
}

/// Create a notification.
pub async fn create(
    pool: &PgPool,
    user_id: Uuid,
    kind: &str,
    title: &str,
    message: &str,
    metadata: Option<Json>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO user_notifications (id, user_id, kind, title, message, metadata) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(kind)
    .bind(title)
    .bind(message)
    .bind(metadata)
    .execute(pool)
    .await?;
    Ok(())
}

/// Count a user's unread notifications.
pub async fn unread_count(pool: &PgPool, user_id: Uuid) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM user_notifications WHERE user_id = $1 AND NOT read",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?)
}

/// List a user's notifications, newest first.
pub async fn list(pool: &PgPool, user_id: Uuid, limit: i64) -> Result<Vec<Notification>> {
    Ok(sqlx::query_as::<_, Notification>(
        "SELECT id, kind, title, message, read, created_at FROM user_notifications \
         WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Mark all of a user's notifications read.
pub async fn mark_all_read(pool: &PgPool, user_id: Uuid) -> Result<u64> {
    let res =
        sqlx::query("UPDATE user_notifications SET read = true WHERE user_id = $1 AND NOT read")
            .bind(user_id)
            .execute(pool)
            .await?;
    Ok(res.rows_affected())
}
