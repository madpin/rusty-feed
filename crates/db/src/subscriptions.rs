//! Subscription repository — the per-user link to a shared feed (`user_feeds`).

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DbError, Result};
use crate::models::{UserFeed, USER_FEED_COLUMNS};

/// Subscribe a user to a feed (idempotent). Returns the subscription and whether
/// it was newly created.
pub async fn subscribe(
    pool: &PgPool,
    user_id: Uuid,
    feed_id: Uuid,
    custom_name: Option<&str>,
) -> Result<(UserFeed, bool)> {
    let sql = format!(
        "INSERT INTO user_feeds (id, user_id, feed_id, custom_name) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (user_id, feed_id) DO NOTHING RETURNING {USER_FEED_COLUMNS}"
    );
    let inserted = sqlx::query_as::<_, UserFeed>(&sql)
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(feed_id)
        .bind(custom_name)
        .fetch_optional(pool)
        .await?;

    match inserted {
        Some(uf) => Ok((uf, true)),
        None => find(pool, user_id, feed_id)
            .await?
            .map(|uf| (uf, false))
            .ok_or(DbError::NotFound),
    }
}

/// Find a specific subscription.
pub async fn find(pool: &PgPool, user_id: Uuid, feed_id: Uuid) -> Result<Option<UserFeed>> {
    let sql =
        format!("SELECT {USER_FEED_COLUMNS} FROM user_feeds WHERE user_id = $1 AND feed_id = $2");
    Ok(sqlx::query_as::<_, UserFeed>(&sql)
        .bind(user_id)
        .bind(feed_id)
        .fetch_optional(pool)
        .await?)
}

/// Whether a user is subscribed to a feed.
pub async fn is_subscribed(pool: &PgPool, user_id: Uuid, feed_id: Uuid) -> Result<bool> {
    Ok(find(pool, user_id, feed_id).await?.is_some())
}

/// Unsubscribe a user from a feed. Returns whether a subscription was removed.
/// Only the per-user row is deleted; the shared feed/articles are untouched.
pub async fn unsubscribe(pool: &PgPool, user_id: Uuid, feed_id: Uuid) -> Result<bool> {
    let res = sqlx::query("DELETE FROM user_feeds WHERE user_id = $1 AND feed_id = $2")
        .bind(user_id)
        .bind(feed_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// List a user's subscriptions, newest first.
pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<UserFeed>> {
    let sql = format!(
        "SELECT {USER_FEED_COLUMNS} FROM user_feeds WHERE user_id = $1 ORDER BY subscribed_at DESC"
    );
    Ok(sqlx::query_as::<_, UserFeed>(&sql)
        .bind(user_id)
        .fetch_all(pool)
        .await?)
}

/// Count subscriptions to a feed (across all users). Handy for dedup assertions.
pub async fn count_for_feed(pool: &PgPool, feed_id: Uuid) -> Result<i64> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_feeds WHERE feed_id = $1")
            .bind(feed_id)
            .fetch_one(pool)
            .await?,
    )
}
