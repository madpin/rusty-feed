//! API token repository (native API + GReader ClientLogin). Only token hashes
//! are stored; the raw token is shown to the user once at creation time.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;
use crate::models::{ApiToken, API_TOKEN_COLUMNS};

/// Create an API token for a user.
pub async fn create(
    pool: &PgPool,
    user_id: Uuid,
    name: &str,
    token_hash: &str,
    scopes: &[String],
) -> Result<ApiToken> {
    let sql = format!(
        "INSERT INTO api_tokens (id, user_id, name, token_hash, scopes) VALUES ($1, $2, $3, $4, $5) \
         RETURNING {API_TOKEN_COLUMNS}"
    );
    Ok(sqlx::query_as::<_, ApiToken>(&sql)
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(name)
        .bind(token_hash)
        .bind(scopes)
        .fetch_one(pool)
        .await?)
}

/// Find an active (non-revoked) token by its hash.
pub async fn find_active_by_hash(pool: &PgPool, token_hash: &str) -> Result<Option<ApiToken>> {
    let sql = format!(
        "SELECT {API_TOKEN_COLUMNS} FROM api_tokens WHERE token_hash = $1 AND revoked_at IS NULL"
    );
    Ok(sqlx::query_as::<_, ApiToken>(&sql)
        .bind(token_hash)
        .fetch_optional(pool)
        .await?)
}

/// List a user's tokens, newest first.
pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<ApiToken>> {
    let sql = format!(
        "SELECT {API_TOKEN_COLUMNS} FROM api_tokens WHERE user_id = $1 ORDER BY created_at DESC"
    );
    Ok(sqlx::query_as::<_, ApiToken>(&sql)
        .bind(user_id)
        .fetch_all(pool)
        .await?)
}

/// Revoke a token owned by `user_id`. Returns whether it was revoked.
pub async fn revoke(pool: &PgPool, id: Uuid, user_id: Uuid) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE api_tokens SET revoked_at = now() WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Update a token's `last_used_at` to now.
pub async fn touch(pool: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query("UPDATE api_tokens SET last_used_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
