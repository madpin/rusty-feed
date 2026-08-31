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

/// Create one feed-health notification per subscriber using a set of personal
/// rows while keeping the shared health transition itself O(1). `event_key`
/// makes retries idempotent for the same transition.
pub async fn create_feed_health_batch(
    pool: &PgPool,
    feed_id: Uuid,
    feed_name: &str,
    status: &str,
    event_key: &str,
) -> Result<u64> {
    let user_ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM user_feeds WHERE feed_id = $1 ORDER BY user_id")
            .bind(feed_id)
            .fetch_all(pool)
            .await?;
    let (title, message) = if status == "recovered" {
        (
            "Feed recovered",
            format!("{feed_name} is updating normally again."),
        )
    } else {
        (
            "Feed needs attention",
            format!("{feed_name} has failed three updates. RustSS will keep retrying safely."),
        )
    };
    let metadata = serde_json::json!({
        "feed_id": feed_id,
        "status": status,
        "event_key": event_key,
    });
    let mut created = 0;
    for user_id in user_ids {
        let result = sqlx::query(
            "INSERT INTO user_notifications (id, user_id, kind, title, message, metadata) \
             SELECT $1, $2, 'feed_health', $3, $4, $5 \
             WHERE NOT EXISTS (SELECT 1 FROM user_notifications \
               WHERE user_id = $2 AND kind = 'feed_health' \
                 AND metadata->>'event_key' = $6)",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(title)
        .bind(&message)
        .bind(&metadata)
        .bind(event_key)
        .execute(pool)
        .await?;
        created += result.rows_affected();
    }
    Ok(created)
}
