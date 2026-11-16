//! Session repository. The cookie carries an opaque token; we store only its
//! SHA-256 hash (see `crates/db/migrations/0002_sessions.sql`).

use std::sync::LazyLock;

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;
use crate::models::{Session, User, SESSION_COLUMNS};
use crate::users::USER_COLUMNS;

/// A valid session together with its owner, loaded by one joined query.
#[derive(Debug, Clone)]
pub struct SessionUser {
    pub session: Session,
    pub user: User,
}

fn aliased(columns: &str, table: &str, prefix: &str) -> String {
    columns
        .split(',')
        .map(|column| format!("{table}.{c} AS {prefix}{c}", c = column.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The joined projection, derived from the shared column constants so it can
/// never drift from [`Session`] / [`User`].
static SESSION_USER_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT {}, {} FROM sessions s JOIN users u ON u.id = s.user_id \
         WHERE s.token_hash = $1 AND s.expires_at > now()",
        aliased(SESSION_COLUMNS, "s", "s_"),
        aliased(USER_COLUMNS, "u", "u_"),
    )
});

fn session_from_row(row: &PgRow) -> sqlx::Result<Session> {
    Ok(Session {
        id: row.try_get("s_id")?,
        user_id: row.try_get("s_user_id")?,
        token_hash: row.try_get("s_token_hash")?,
        created_at: row.try_get("s_created_at")?,
        last_used_at: row.try_get("s_last_used_at")?,
        expires_at: row.try_get("s_expires_at")?,
    })
}

fn user_from_row(row: &PgRow) -> sqlx::Result<User> {
    Ok(User {
        id: row.try_get("u_id")?,
        email: row.try_get("u_email")?,
        name: row.try_get("u_name")?,
        image: row.try_get("u_image")?,
        role: row.try_get("u_role")?,
        password_hash: row.try_get("u_password_hash")?,
        email_verified_at: row.try_get("u_email_verified_at")?,
        created_at: row.try_get("u_created_at")?,
        updated_at: row.try_get("u_updated_at")?,
        last_seen_at: row.try_get("u_last_seen_at")?,
        failed_login_count: row.try_get("u_failed_login_count")?,
        locked_until: row.try_get("u_locked_until")?,
        tos_accepted_at: row.try_get("u_tos_accepted_at")?,
        is_demo: row.try_get("u_is_demo")?,
        onboarded_at: row.try_get("u_onboarded_at")?,
    })
}

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

/// Find a non-expired session and its owner in one round trip. Equivalent to
/// [`find_valid`] followed by `users::find_by_id`, returning `None` when the
/// session is missing/expired or the user no longer exists.
pub async fn find_valid_with_user(pool: &PgPool, token_hash: &str) -> Result<Option<SessionUser>> {
    let row = sqlx::query(SESSION_USER_SQL.as_str())
        .bind(token_hash)
        .fetch_optional(pool)
        .await?;
    row.map(|row| {
        Ok(SessionUser {
            session: session_from_row(&row)?,
            user: user_from_row(&row)?,
        })
    })
    .transpose()
}

/// Update a session's `last_used_at` to now, throttled to at most once per
/// minute per session so busy sessions do not write on every request.
pub async fn touch(pool: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE sessions SET last_used_at = now() \
         WHERE id = $1 AND last_used_at < now() - interval '1 minute'",
    )
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
