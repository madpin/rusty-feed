//! Subscription repository — the per-user link to a shared feed (`user_feeds`).

use std::collections::BTreeSet;

use rustss_core::feed_content::{
    ContentLifetime, FeedContentMode, SubscriptionContentSettings, SubscriptionEnclosureSettings,
};
use rustss_core::youtube::SubscriptionYouTubeMode;
use serde_json::Value as Json;
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

/// Personal subscriber identifiers for notification fan-out after one shared fetch.
pub async fn subscriber_ids(pool: &PgPool, feed_id: Uuid) -> Result<Vec<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT user_id FROM user_feeds WHERE feed_id = $1 ORDER BY user_id")
            .bind(feed_id)
            .fetch_all(pool)
            .await?,
    )
}

/// Return the normalized URLs among `urls` that this user already subscribes to.
pub async fn subscribed_urls(pool: &PgPool, user_id: Uuid, urls: &[String]) -> Result<Vec<String>> {
    if urls.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar(
        "SELECT f.url FROM user_feeds uf JOIN feeds f ON f.id = uf.feed_id \
         WHERE uf.user_id = $1 AND f.url = ANY($2)",
    )
    .bind(user_id)
    .bind(urls)
    .fetch_all(pool)
    .await?)
}

/// Whether every unique selected feed belongs to the user.
pub async fn all_subscribed(pool: &PgPool, user_id: Uuid, feed_ids: &[Uuid]) -> Result<bool> {
    let feed_ids: Vec<_> = feed_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if feed_ids.is_empty() {
        return Ok(false);
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_feeds WHERE user_id = $1 AND feed_id = ANY($2)",
    )
    .bind(user_id)
    .bind(&feed_ids)
    .fetch_one(pool)
    .await?;
    Ok(count == feed_ids.len() as i64)
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

/// Set the caller-owned title override for one subscription.
pub async fn set_custom_name(
    pool: &PgPool,
    user_id: Uuid,
    feed_id: Uuid,
    custom_name: Option<&str>,
) -> Result<()> {
    let result = sqlx::query(
        "UPDATE user_feeds SET custom_name = $3, updated_at = now() \
         WHERE user_id = $1 AND feed_id = $2",
    )
    .bind(user_id)
    .bind(feed_id)
    .bind(custom_name)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
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

/// Load the content choice owned by one subscription.
pub async fn content_settings_for_user(
    pool: &PgPool,
    user_id: Uuid,
    feed_id: Uuid,
) -> Result<Option<SubscriptionContentSettings>> {
    let settings: Option<Option<Json>> =
        sqlx::query_scalar("SELECT settings FROM user_feeds WHERE user_id = $1 AND feed_id = $2")
            .bind(user_id)
            .bind(feed_id)
            .fetch_optional(pool)
            .await?;
    Ok(settings.map(|settings| SubscriptionContentSettings::from_json(settings.as_ref())))
}

/// Merge content fields into one user's subscription settings.
pub async fn update_content_settings(
    pool: &PgPool,
    user_id: Uuid,
    feed_id: Uuid,
    settings: SubscriptionContentSettings,
) -> Result<()> {
    let result = sqlx::query(
        "UPDATE user_feeds SET settings = COALESCE(settings, '{}'::jsonb) || $3, \
         updated_at = now() WHERE user_id = $1 AND feed_id = $2",
    )
    .bind(user_id)
    .bind(feed_id)
    .bind(settings.to_json())
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Atomically merge content ownership and YouTube rendering fields for one subscription.
pub async fn update_content_and_youtube_settings(
    pool: &PgPool,
    user_id: Uuid,
    feed_id: Uuid,
    content: SubscriptionContentSettings,
    youtube_mode: SubscriptionYouTubeMode,
) -> Result<()> {
    let patch = content.to_json();
    let patch = merge_json_objects(patch, youtube_mode.to_json());
    let result = sqlx::query(
        "UPDATE user_feeds SET settings = COALESCE(settings, '{}'::jsonb) || $3, \
         updated_at = now() WHERE user_id = $1 AND feed_id = $2",
    )
    .bind(user_id)
    .bind(feed_id)
    .bind(patch)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Atomically merge content, YouTube, and attachment-rendering fields for one
/// subscription without disturbing folder, tag, or future settings.
pub async fn update_reader_settings(
    pool: &PgPool,
    user_id: Uuid,
    feed_id: Uuid,
    content: SubscriptionContentSettings,
    youtube_mode: SubscriptionYouTubeMode,
    enclosures: SubscriptionEnclosureSettings,
) -> Result<()> {
    let patch = merge_json_objects(content.to_json(), youtube_mode.to_json());
    let patch = merge_json_objects(patch, enclosures.to_json());
    let result = sqlx::query(
        "UPDATE user_feeds SET settings = COALESCE(settings, '{}'::jsonb) || $3, \
         updated_at = now() WHERE user_id = $1 AND feed_id = $2",
    )
    .bind(user_id)
    .bind(feed_id)
    .bind(patch)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Merge validated personal entry filters without disturbing other subscription settings.
pub async fn update_entry_filters(
    pool: &PgPool,
    user_id: Uuid,
    feed_id: Uuid,
    filters: &[rustss_core::content_rules::EntryFilter],
) -> Result<()> {
    let patch = serde_json::json!({ "entry_filters": filters });
    let result = sqlx::query(
        "UPDATE user_feeds SET settings = COALESCE(settings, '{}'::jsonb) || $3, \
         updated_at = now() WHERE user_id = $1 AND feed_id = $2",
    )
    .bind(user_id)
    .bind(feed_id)
    .bind(patch)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Load the optional YouTube player override owned by one subscription.
pub async fn youtube_mode_for_user(
    pool: &PgPool,
    user_id: Uuid,
    feed_id: Uuid,
) -> Result<Option<SubscriptionYouTubeMode>> {
    let settings: Option<Option<Json>> =
        sqlx::query_scalar("SELECT settings FROM user_feeds WHERE user_id = $1 AND feed_id = $2")
            .bind(user_id)
            .bind(feed_id)
            .fetch_optional(pool)
            .await?;
    Ok(settings.map(|settings| SubscriptionYouTubeMode::from_json(settings.as_ref())))
}

/// Merge a YouTube player override without disturbing other subscription settings.
pub async fn update_youtube_mode(
    pool: &PgPool,
    user_id: Uuid,
    feed_id: Uuid,
    mode: SubscriptionYouTubeMode,
) -> Result<()> {
    let result = sqlx::query(
        "UPDATE user_feeds SET settings = COALESCE(settings, '{}'::jsonb) || $3, \
         updated_at = now() WHERE user_id = $1 AND feed_id = $2",
    )
    .bind(user_id)
    .bind(feed_id)
    .bind(mode.to_json())
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

fn merge_json_objects(mut left: Json, right: Json) -> Json {
    if let (Some(left), Some(right)) = (left.as_object_mut(), right.as_object()) {
        left.extend(right.clone());
    }
    left
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportSubscriptionOutcome {
    Created,
    Updated,
    Unchanged,
}

/// Import one subscription and its personal folder atomically.
///
/// Existing subscriptions are left untouched unless `replace_personalization`
/// is requested. Shared feed creation is deliberately handled by the feed
/// repository before this personal transaction.
pub async fn import_subscription(
    pool: &PgPool,
    user_id: Uuid,
    feed_id: Uuid,
    custom_name: Option<&str>,
    category_id: Option<Uuid>,
    replace_personalization: bool,
) -> Result<ImportSubscriptionOutcome> {
    let mut transaction = pool.begin().await?;
    if let Some(category_id) = category_id {
        let category_exists: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM user_categories WHERE user_id = $1 AND id = $2")
                .bind(user_id)
                .bind(category_id)
                .fetch_optional(&mut *transaction)
                .await?;
        if category_exists.is_none() {
            return Err(DbError::NotFound);
        }
    }

    let subscription_id: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO user_feeds (id, user_id, feed_id, custom_name) \
         VALUES ($1, $2, $3, $4) ON CONFLICT (user_id, feed_id) DO NOTHING \
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(feed_id)
    .bind(custom_name)
    .fetch_optional(&mut *transaction)
    .await?;

    let (subscription_id, outcome) = match subscription_id {
        Some(subscription_id) => (subscription_id, ImportSubscriptionOutcome::Created),
        None if !replace_personalization => {
            transaction.commit().await?;
            return Ok(ImportSubscriptionOutcome::Unchanged);
        }
        None => {
            let subscription_id: Uuid = sqlx::query_scalar(
                "SELECT id FROM user_feeds WHERE user_id = $1 AND feed_id = $2 FOR UPDATE",
            )
            .bind(user_id)
            .bind(feed_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(DbError::NotFound)?;
            sqlx::query(
                "UPDATE user_feeds SET custom_name = $3, updated_at = now() \
                 WHERE user_id = $1 AND id = $2",
            )
            .bind(user_id)
            .bind(subscription_id)
            .bind(custom_name)
            .execute(&mut *transaction)
            .await?;
            (subscription_id, ImportSubscriptionOutcome::Updated)
        }
    };

    sqlx::query("DELETE FROM user_feed_categories WHERE user_feed_id = $1")
        .bind(subscription_id)
        .execute(&mut *transaction)
        .await?;
    if let Some(category_id) = category_id {
        sqlx::query(
            "INSERT INTO user_feed_categories (id, user_feed_id, user_category_id) \
             VALUES ($1, $2, $3)",
        )
        .bind(Uuid::now_v7())
        .bind(subscription_id)
        .bind(category_id)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(outcome)
}

/// Resolve all personal public-page requests into one lifetime for the shared
/// copy. Infinite dominates; otherwise the longest lifetime wins.
pub async fn requested_article_page_lifetime(
    pool: &PgPool,
    feed_id: Uuid,
) -> Result<Option<ContentLifetime>> {
    let settings: Vec<Option<Json>> =
        sqlx::query_scalar("SELECT settings FROM user_feeds WHERE feed_id = $1")
            .bind(feed_id)
            .fetch_all(pool)
            .await?;
    Ok(settings
        .iter()
        .map(|value| SubscriptionContentSettings::from_json(value.as_ref()))
        .filter(|settings| settings.mode == FeedContentMode::ArticlePage)
        .map(|settings| settings.lifetime)
        .reduce(ContentLifetime::merge))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FeedOrganization {
    pub feed_id: Uuid,
    pub custom_name: Option<String>,
    pub tags: Vec<String>,
    pub category_id: Option<Uuid>,
    pub settings: Option<Json>,
}

pub async fn list_organization(pool: &PgPool, user_id: Uuid) -> Result<Vec<FeedOrganization>> {
    Ok(sqlx::query_as(
        "SELECT uf.feed_id, uf.custom_name, uf.tags, category.id AS category_id, uf.settings FROM user_feeds uf \
         LEFT JOIN LATERAL (SELECT c.id FROM user_feed_categories ufc \
              JOIN user_categories c ON c.id = ufc.user_category_id AND c.user_id = uf.user_id \
              WHERE ufc.user_feed_id = uf.id ORDER BY ufc.created_at, c.name LIMIT 1) category ON true \
         WHERE uf.user_id = $1",
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
    category_id: Option<Uuid>,
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
    if let Some(category_id) = category_id {
        let valid: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM user_categories WHERE user_id = $1 AND id = $2")
                .bind(user_id)
                .bind(category_id)
                .fetch_optional(&mut *transaction)
                .await?;
        if valid.is_none() {
            return Err(DbError::NotFound);
        }
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
    if let Some(category_id) = category_id {
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

/// Bulk tag update applied only to the caller's subscriptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkTagOperation {
    Add,
    Remove,
    Replace,
}

#[derive(sqlx::FromRow)]
struct SubscriptionTags {
    id: Uuid,
    tags: Vec<String>,
}

fn validate_tags(tags: &[String]) -> Result<()> {
    if tags.len() > 20
        || tags.iter().any(|tag| {
            tag.is_empty() || tag.chars().count() > 40 || tag.chars().any(char::is_control)
        })
    {
        return Err(DbError::Validation(
            "Each feed can have at most 20 tags of up to 40 printable characters.".to_string(),
        ));
    }
    Ok(())
}

/// Move several of the user's subscriptions into one folder, preserving tags.
pub async fn move_many_to_category(
    pool: &PgPool,
    user_id: Uuid,
    feed_ids: &[Uuid],
    category_id: Option<Uuid>,
) -> Result<usize> {
    let feed_ids: Vec<_> = feed_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if feed_ids.is_empty() {
        return Ok(0);
    }
    let mut transaction = pool.begin().await?;
    let user_feed_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM user_feeds WHERE user_id = $1 AND feed_id = ANY($2) FOR UPDATE",
    )
    .bind(user_id)
    .bind(&feed_ids)
    .fetch_all(&mut *transaction)
    .await?;
    if user_feed_ids.len() != feed_ids.len() {
        return Err(DbError::NotFound);
    }
    if let Some(category_id) = category_id {
        let valid: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM user_categories WHERE user_id = $1 AND id = $2")
                .bind(user_id)
                .bind(category_id)
                .fetch_optional(&mut *transaction)
                .await?;
        if valid.is_none() {
            return Err(DbError::NotFound);
        }
    }
    sqlx::query("DELETE FROM user_feed_categories WHERE user_feed_id = ANY($1)")
        .bind(&user_feed_ids)
        .execute(&mut *transaction)
        .await?;
    if let Some(category_id) = category_id {
        for user_feed_id in &user_feed_ids {
            sqlx::query(
                "INSERT INTO user_feed_categories (id, user_feed_id, user_category_id) VALUES ($1, $2, $3)",
            )
            .bind(Uuid::now_v7())
            .bind(user_feed_id)
            .bind(category_id)
            .execute(&mut *transaction)
            .await?;
        }
    }
    sqlx::query("UPDATE user_feeds SET updated_at = now() WHERE id = ANY($1)")
        .bind(&user_feed_ids)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(user_feed_ids.len())
}

/// Update tags on several of the user's subscriptions atomically.
pub async fn update_tags_many(
    pool: &PgPool,
    user_id: Uuid,
    feed_ids: &[Uuid],
    operation: BulkTagOperation,
    tags: &[String],
) -> Result<usize> {
    validate_tags(tags)?;
    let feed_ids: Vec<_> = feed_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if feed_ids.is_empty() {
        return Ok(0);
    }
    let mut transaction = pool.begin().await?;
    let subscriptions: Vec<SubscriptionTags> = sqlx::query_as(
        "SELECT id, tags FROM user_feeds WHERE user_id = $1 AND feed_id = ANY($2) FOR UPDATE",
    )
    .bind(user_id)
    .bind(&feed_ids)
    .fetch_all(&mut *transaction)
    .await?;
    if subscriptions.len() != feed_ids.len() {
        return Err(DbError::NotFound);
    }
    for subscription in &subscriptions {
        let updated = match operation {
            BulkTagOperation::Add => {
                let mut updated = subscription.tags.clone();
                for tag in tags {
                    if !updated.contains(tag) {
                        updated.push(tag.clone());
                    }
                }
                updated
            }
            BulkTagOperation::Remove => subscription
                .tags
                .iter()
                .filter(|tag| !tags.contains(*tag))
                .cloned()
                .collect(),
            BulkTagOperation::Replace => tags.to_vec(),
        };
        validate_tags(&updated)?;
        sqlx::query("UPDATE user_feeds SET tags = $2, updated_at = now() WHERE id = $1")
            .bind(subscription.id)
            .bind(updated)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(subscriptions.len())
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
