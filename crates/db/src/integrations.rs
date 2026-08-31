//! Named outbound destination profiles and durable delivery attempts.

use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::Result;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct IntegrationProfile {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub provider: String,
    pub endpoint_url: String,
    pub credential_ciphertext: Option<String>,
    pub credential_kind: String,
    pub payload_template: String,
    pub tags: Vec<String>,
    pub event_types: Vec<String>,
    pub max_attempts: i32,
    pub rate_limit_per_minute: i32,
    pub is_default: bool,
    pub enabled: bool,
    pub paused_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct IntegrationDelivery {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub user_id: Uuid,
    pub article_id: Option<Uuid>,
    pub event_type: String,
    pub payload_version: i32,
    pub status: String,
    pub attempt_count: i32,
    pub response_status: Option<i32>,
    pub error_summary: Option<String>,
    pub next_attempt_at: OffsetDateTime,
    pub created_at: OffsetDateTime,
    pub finished_at: Option<OffsetDateTime>,
}

const PROFILE_COLUMNS: &str = "id, user_id, name, provider, endpoint_url, credential_ciphertext, credential_kind, payload_template, tags, event_types, max_attempts, rate_limit_per_minute, is_default, enabled, paused_at, created_at, updated_at";
const DELIVERY_COLUMNS: &str = "id, profile_id, user_id, article_id, event_type, payload_version, status, attempt_count, response_status, error_summary, next_attempt_at, created_at, finished_at";

#[allow(clippy::too_many_arguments)]
pub async fn create_profile(
    pool: &PgPool,
    user_id: Uuid,
    name: &str,
    provider: &str,
    endpoint_url: &str,
    credential_ciphertext: Option<&str>,
    credential_kind: &str,
    payload_template: &str,
    tags: &[String],
    event_types: &[String],
    max_attempts: i32,
    rate_limit_per_minute: i32,
    is_default: bool,
) -> Result<IntegrationProfile> {
    let mut transaction = pool.begin().await?;
    if is_default {
        sqlx::query("UPDATE integration_profiles SET is_default = false, updated_at = now() WHERE user_id = $1")
            .bind(user_id).execute(&mut *transaction).await?;
    }
    let sql = format!("INSERT INTO integration_profiles (id, user_id, name, provider, endpoint_url, credential_ciphertext, credential_kind, payload_template, tags, event_types, max_attempts, rate_limit_per_minute, is_default) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) RETURNING {PROFILE_COLUMNS}");
    let row = sqlx::query_as(&sql)
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(name)
        .bind(provider)
        .bind(endpoint_url)
        .bind(credential_ciphertext)
        .bind(credential_kind)
        .bind(payload_template)
        .bind(tags)
        .bind(event_types)
        .bind(max_attempts.clamp(1, 12))
        .bind(rate_limit_per_minute.clamp(1, 600))
        .bind(is_default)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(row)
}

pub async fn list_profiles(pool: &PgPool, user_id: Uuid) -> Result<Vec<IntegrationProfile>> {
    let sql = format!("SELECT {PROFILE_COLUMNS} FROM integration_profiles WHERE user_id = $1 ORDER BY is_default DESC, created_at DESC");
    Ok(sqlx::query_as(&sql).bind(user_id).fetch_all(pool).await?)
}

pub async fn find_owned(
    pool: &PgPool,
    id: Uuid,
    user_id: Uuid,
) -> Result<Option<IntegrationProfile>> {
    let sql = format!(
        "SELECT {PROFILE_COLUMNS} FROM integration_profiles WHERE id = $1 AND user_id = $2"
    );
    Ok(sqlx::query_as(&sql)
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?)
}

pub async fn find_enabled(pool: &PgPool, id: Uuid) -> Result<Option<IntegrationProfile>> {
    let sql = format!("SELECT {PROFILE_COLUMNS} FROM integration_profiles WHERE id = $1 AND enabled AND paused_at IS NULL");
    Ok(sqlx::query_as(&sql).bind(id).fetch_optional(pool).await?)
}

pub async fn profiles_for_event(
    pool: &PgPool,
    user_id: Uuid,
    event_type: &str,
) -> Result<Vec<IntegrationProfile>> {
    let sql = format!("SELECT {PROFILE_COLUMNS} FROM integration_profiles WHERE user_id = $1 AND enabled AND paused_at IS NULL AND event_types @> ARRAY[$2]::text[] ORDER BY is_default DESC, created_at");
    Ok(sqlx::query_as(&sql)
        .bind(user_id)
        .bind(event_type)
        .fetch_all(pool)
        .await?)
}

pub async fn default_profile(pool: &PgPool, user_id: Uuid) -> Result<Option<IntegrationProfile>> {
    let sql = format!("SELECT {PROFILE_COLUMNS} FROM integration_profiles WHERE user_id = $1 AND enabled AND paused_at IS NULL ORDER BY is_default DESC, created_at LIMIT 1");
    Ok(sqlx::query_as(&sql)
        .bind(user_id)
        .fetch_optional(pool)
        .await?)
}

pub async fn set_paused(pool: &PgPool, id: Uuid, user_id: Uuid, paused: bool) -> Result<bool> {
    Ok(sqlx::query("UPDATE integration_profiles SET paused_at = CASE WHEN $3 THEN now() ELSE NULL END, updated_at = now() WHERE id = $1 AND user_id = $2")
        .bind(id).bind(user_id).bind(paused).execute(pool).await?.rows_affected() > 0)
}

pub async fn rotate_credential(
    pool: &PgPool,
    id: Uuid,
    user_id: Uuid,
    credential_ciphertext: Option<&str>,
    credential_kind: &str,
) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE integration_profiles SET credential_ciphertext = $3, credential_kind = $4, \
         updated_at = now() WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .bind(credential_ciphertext)
    .bind(credential_kind)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

pub async fn delete_profile(pool: &PgPool, id: Uuid, user_id: Uuid) -> Result<bool> {
    Ok(
        sqlx::query("DELETE FROM integration_profiles WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(pool)
            .await?
            .rows_affected()
            > 0,
    )
}

pub async fn queue_delivery(
    pool: &PgPool,
    profile: &IntegrationProfile,
    article_id: Option<Uuid>,
    event_type: &str,
) -> Result<IntegrationDelivery> {
    let sql = format!("INSERT INTO integration_deliveries (id, profile_id, user_id, article_id, event_type) VALUES ($1,$2,$3,$4,$5) RETURNING {DELIVERY_COLUMNS}");
    Ok(sqlx::query_as(&sql)
        .bind(Uuid::now_v7())
        .bind(profile.id)
        .bind(profile.user_id)
        .bind(article_id)
        .bind(event_type)
        .fetch_one(pool)
        .await?)
}

pub async fn find_delivery(pool: &PgPool, id: Uuid) -> Result<Option<IntegrationDelivery>> {
    let sql = format!("SELECT {DELIVERY_COLUMNS} FROM integration_deliveries WHERE id = $1");
    Ok(sqlx::query_as(&sql).bind(id).fetch_optional(pool).await?)
}

pub async fn list_deliveries(
    pool: &PgPool,
    user_id: Uuid,
    limit: i64,
) -> Result<Vec<IntegrationDelivery>> {
    let sql = format!("SELECT {DELIVERY_COLUMNS} FROM integration_deliveries WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2");
    Ok(sqlx::query_as(&sql)
        .bind(user_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(pool)
        .await?)
}

pub async fn mark_delivering(pool: &PgPool, id: Uuid) -> Result<bool> {
    Ok(sqlx::query("UPDATE integration_deliveries SET status = 'delivering', attempt_count = attempt_count + 1 WHERE id = $1 AND status IN ('queued','retrying')")
        .bind(id).execute(pool).await?.rows_affected() > 0)
}

pub async fn deliveries_in_last_minute(pool: &PgPool, profile_id: Uuid) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM integration_deliveries WHERE profile_id = $1 AND attempt_count > 0 AND created_at >= now() - interval '1 minute'",
    )
    .bind(profile_id)
    .fetch_one(pool)
    .await?)
}

pub async fn mark_delivered(pool: &PgPool, id: Uuid, status: i32) -> Result<()> {
    sqlx::query("UPDATE integration_deliveries SET status = 'delivered', response_status = $2, error_summary = NULL, finished_at = now() WHERE id = $1")
        .bind(id).bind(status).execute(pool).await?;
    Ok(())
}

pub async fn mark_failed(
    pool: &PgPool,
    id: Uuid,
    max_attempts: i32,
    status: Option<i32>,
    error: &str,
) -> Result<bool> {
    let retrying: bool = sqlx::query_scalar("UPDATE integration_deliveries SET status = CASE WHEN attempt_count < $2 THEN 'retrying' ELSE 'dead' END, response_status = $3, error_summary = left($4, 500), next_attempt_at = now() + make_interval(secs => least(3600, 15 * power(2, greatest(attempt_count - 1, 0)))::int), finished_at = CASE WHEN attempt_count < $2 THEN NULL ELSE now() END WHERE id = $1 RETURNING status = 'retrying'")
        .bind(id).bind(max_attempts).bind(status).bind(error).fetch_one(pool).await?;
    Ok(retrying)
}

pub async fn payload_for_delivery(pool: &PgPool, delivery: &IntegrationDelivery) -> Result<Value> {
    let article = match delivery.article_id {
        Some(id) => {
            sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(
                "SELECT title, url, author, excerpt FROM articles WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?
        }
        None => None,
    };
    Ok(match article {
        Some((title, url, author, excerpt)) => {
            serde_json::json!({"version": 1, "event_type": delivery.event_type, "delivery_id": delivery.id, "article": {"id": delivery.article_id, "title": title, "url": url, "author": author, "excerpt": excerpt}})
        }
        None => {
            serde_json::json!({"version": 1, "event_type": delivery.event_type, "delivery_id": delivery.id})
        }
    })
}
