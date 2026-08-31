//! Managed LLM provider profiles. API keys are accepted only as ciphertext;
//! encryption and decryption stay at application boundaries.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{DbError, Result};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LlmProviderProfile {
    pub id: Uuid,
    pub owner_user_id: Option<Uuid>,
    pub scope: String,
    pub name: String,
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub api_key_ciphertext: Option<String>,
    pub enabled: bool,
    pub priority: i32,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

const COLUMNS: &str = "id, owner_user_id, scope, name, provider, model, base_url, \
    api_key_ciphertext, enabled, priority, created_at, updated_at";

#[allow(clippy::too_many_arguments)]
pub async fn create(
    pool: &PgPool,
    owner_user_id: Option<Uuid>,
    scope: &str,
    name: &str,
    provider: &str,
    model: &str,
    base_url: &str,
    api_key_ciphertext: Option<&str>,
    enabled: bool,
    priority: i32,
) -> Result<LlmProviderProfile> {
    let sql = format!(
        "INSERT INTO llm_provider_profiles \
         (id, owner_user_id, scope, name, provider, model, base_url, api_key_ciphertext, enabled, priority) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING {COLUMNS}"
    );
    Ok(sqlx::query_as(&sql)
        .bind(Uuid::now_v7())
        .bind(owner_user_id)
        .bind(scope)
        .bind(name)
        .bind(provider)
        .bind(model)
        .bind(base_url)
        .bind(api_key_ciphertext)
        .bind(enabled)
        .bind(priority)
        .fetch_one(pool)
        .await?)
}

pub async fn list_for_owner(
    pool: &PgPool,
    owner_user_id: Option<Uuid>,
    scope: &str,
) -> Result<Vec<LlmProviderProfile>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM llm_provider_profiles \
         WHERE scope = $1 AND owner_user_id IS NOT DISTINCT FROM $2 \
         ORDER BY priority, created_at"
    );
    Ok(sqlx::query_as(&sql)
        .bind(scope)
        .bind(owner_user_id)
        .fetch_all(pool)
        .await?)
}

/// Resolve enabled profiles in user → administrator order.
pub async fn resolve(pool: &PgPool, user_id: Option<Uuid>) -> Result<Vec<LlmProviderProfile>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM llm_provider_profiles \
         WHERE enabled AND ((scope = 'user' AND owner_user_id = $1) OR scope = 'admin') \
         ORDER BY CASE WHEN scope = 'user' THEN 0 ELSE 1 END, priority, created_at"
    );
    Ok(sqlx::query_as(&sql).bind(user_id).fetch_all(pool).await?)
}

pub async fn any_enabled(pool: &PgPool) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM llm_provider_profiles WHERE enabled)",
    )
    .fetch_one(pool)
    .await?)
}

/// Whether this user has their own enabled personal (BYO-key) provider. Such
/// users pay with their own key and are exempt from the instance LLM budget
/// (`docs/PUBLIC_LAUNCH_PLAN.md` §4.1, §8.1).
pub async fn has_enabled_user_profile(pool: &PgPool, user_id: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM llm_provider_profiles \
         WHERE enabled AND scope = 'user' AND owner_user_id = $1)",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?)
}

pub async fn set_enabled(
    pool: &PgPool,
    id: Uuid,
    owner_user_id: Option<Uuid>,
    scope: &str,
    enabled: bool,
) -> Result<()> {
    let result = sqlx::query(
        "UPDATE llm_provider_profiles SET enabled = $4 \
         WHERE id = $1 AND scope = $3 AND owner_user_id IS NOT DISTINCT FROM $2",
    )
    .bind(id)
    .bind(owner_user_id)
    .bind(scope)
    .bind(enabled)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

pub async fn delete(
    pool: &PgPool,
    id: Uuid,
    owner_user_id: Option<Uuid>,
    scope: &str,
) -> Result<bool> {
    Ok(sqlx::query(
        "DELETE FROM llm_provider_profiles \
         WHERE id = $1 AND scope = $3 AND owner_user_id IS NOT DISTINCT FROM $2",
    )
    .bind(id)
    .bind(owner_user_id)
    .bind(scope)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}
