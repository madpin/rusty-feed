//! Session repository. The cookie carries an opaque token; we store only its
//! SHA-256 hash (see `crates/db/migrations/0002_sessions.sql`).

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;
use crate::models::{Session, SESSION_COLUMNS};

/// Create a session for a user with the given token hash and expiry.
pub async fn create(
    pool: &PgPool,
    user_id: Uuid,
    token_hash: &str,
    expires_at: OffsetDateTime,
) -> Result<Session> {
    let sql = format!(
        "INSERT INTO sessions (id, user_id, token_hash, expires_at) VALUES ($1, $2, $3, $4) \
         RETURNING {SESSION_COLUMNS}"
    );
    Ok(sqlx::query_as::<_, Session>(&sql)
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(token_hash)
        .bind(expires_at)
        .fetch_one(pool)
        .await?)
}

/// Find a non-expired session by token hash.
pub async fn find_valid(pool: &PgPool, token_hash: &str) -> Result<Option<Session>> {
    let sql = format!(
        "SELECT {SESSION_COLUMNS} FROM sessions WHERE token_hash = $1 AND expires_at > now()"
    );
    Ok(sqlx::query_as::<_, Session>(&sql)
        .bind(token_hash)
        .fetch_optional(pool)
        .await?)
}

/// Update a session's `last_used_at` to now.
pub async fn touch(pool: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query("UPDATE sessions SET last_used_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete a session by token hash (logout). Returns whether one was removed.
pub async fn delete_by_token_hash(pool: &PgPool, token_hash: &str) -> Result<bool> {
    let res = sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
        .bind(token_hash)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Delete all of a user's sessions (e.g. on password change / "log out
/// everywhere").
pub async fn delete_for_user(pool: &PgPool, user_id: Uuid) -> Result<u64> {
    let res = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// List a user's non-expired sessions, most recently active first.
pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<Session>> {
    let sql = format!(
        "SELECT {SESSION_COLUMNS} FROM sessions WHERE user_id = $1 AND expires_at > now() \
         ORDER BY last_used_at DESC, created_at DESC"
    );
    Ok(sqlx::query_as(&sql).bind(user_id).fetch_all(pool).await?)
}

/// Delete one session, scoped to its owner.
pub async fn delete_for_user_by_id(pool: &PgPool, user_id: Uuid, id: Uuid) -> Result<bool> {
    let result = sqlx::query("DELETE FROM sessions WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete every session except the current opaque-token hash.
pub async fn delete_other_sessions(
    pool: &PgPool,
    user_id: Uuid,
    current_token_hash: &str,
) -> Result<u64> {
    let result = sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND token_hash <> $2")
        .bind(user_id)
        .bind(current_token_hash)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

/// Delete expired sessions (housekeeping; called by a maintenance agent later).
pub async fn delete_expired(pool: &PgPool) -> Result<u64> {
    let res = sqlx::query("DELETE FROM sessions WHERE expires_at <= now()")
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}
