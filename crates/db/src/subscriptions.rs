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

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FeedOrganization {
    pub feed_id: Uuid,
    pub tags: Vec<String>,
    pub category_ids: Vec<Uuid>,
}

pub async fn list_organization(pool: &PgPool, user_id: Uuid) -> Result<Vec<FeedOrganization>> {
    Ok(sqlx::query_as(
        "SELECT uf.feed_id, uf.tags, \
         coalesce(array_agg(c.id ORDER BY c.name) FILTER (WHERE c.id IS NOT NULL), '{}') AS category_ids \
         FROM user_feeds uf \
         LEFT JOIN user_feed_categories ufc ON ufc.user_feed_id = uf.id \
         LEFT JOIN user_categories c ON c.id = ufc.user_category_id AND c.user_id = uf.user_id \
         WHERE uf.user_id = $1 GROUP BY uf.feed_id, uf.tags",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

pub async fn update_organization(
    pool: &PgPool,
    user_id: Uuid,
    feed_id: Uuid,
    tags: &[String],
    category_ids: &[Uuid],
) -> Result<()> {
    let mut transaction = pool.begin().await?;
    let user_feed_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM user_feeds WHERE user_id = $1 AND feed_id = $2 FOR UPDATE",
    )
    .bind(user_id)
    .bind(feed_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(user_feed_id) = user_feed_id else {
        return Err(DbError::NotFound);
    };
    let category_ids: std::collections::BTreeSet<Uuid> = category_ids.iter().copied().collect();
    let valid_category_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_categories WHERE user_id = $1 AND id = ANY($2)",
    )
    .bind(user_id)
    .bind(category_ids.iter().copied().collect::<Vec<_>>())
    .fetch_one(&mut *transaction)
    .await?;
    if valid_category_count != category_ids.len() as i64 {
        return Err(DbError::NotFound);
    }
    sqlx::query(
        "UPDATE user_feeds SET tags = $3, updated_at = now() WHERE id = $1 AND user_id = $2",
    )
    .bind(user_feed_id)
    .bind(user_id)
    .bind(tags)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("DELETE FROM user_feed_categories WHERE user_feed_id = $1")
        .bind(user_feed_id)
        .execute(&mut *transaction)
        .await?;
    for category_id in category_ids {
        sqlx::query(
            "INSERT INTO user_feed_categories (id, user_feed_id, user_category_id) VALUES ($1, $2, $3)",
        )
        .bind(Uuid::now_v7())
        .bind(user_feed_id)
        .bind(category_id)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
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
