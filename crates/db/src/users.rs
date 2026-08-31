//! User repository.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;
use crate::models::User;

const USER_COLUMNS: &str =
    "id, email, name, image, role, password_hash, email_verified_at, created_at, updated_at, \
     last_seen_at, failed_login_count, locked_until";

/// Create a user. `role` must be one of `admin` | `user` | `guest`.
pub async fn create(
    pool: &PgPool,
    email: &str,
    name: Option<&str>,
    image: Option<&str>,
    role: &str,
    password_hash: Option<&str>,
) -> Result<User> {
    let sql = format!(
        "INSERT INTO users (id, email, name, image, role, password_hash) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING {USER_COLUMNS}"
    );
    let user = sqlx::query_as::<_, User>(&sql)
        .bind(Uuid::now_v7())
        .bind(email)
        .bind(name)
        .bind(image)
        .bind(role)
        .bind(password_hash)
        .fetch_one(pool)
        .await?;
    Ok(user)
}

/// Create a user and idempotently subscribe them to the administrator-curated
/// featured catalog feeds in one transaction. Featured catalog entries are the
/// customizable new-user defaults; shared feed/article rows are never copied.
pub async fn create_with_default_feeds(
    pool: &PgPool,
    email: &str,
    name: Option<&str>,
    image: Option<&str>,
    role: &str,
    password_hash: Option<&str>,
) -> Result<User> {
    let mut transaction = pool.begin().await?;
    let sql = format!(
        "INSERT INTO users (id, email, name, image, role, password_hash) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING {USER_COLUMNS}"
    );
    let user = sqlx::query_as::<_, User>(&sql)
        .bind(Uuid::now_v7())
        .bind(email)
        .bind(name)
        .bind(image)
        .bind(role)
        .bind(password_hash)
        .fetch_one(&mut *transaction)
        .await?;
    subscribe_featured_catalog(&mut transaction, user.id).await?;
    transaction.commit().await?;
    Ok(user)
}

/// Atomically consume an invitation, create its user, and apply the shared
/// featured-feed defaults. Returns `None` when the token is expired, revoked,
/// already used, or restricted to a different email address.
pub async fn create_with_invitation(
    pool: &PgPool,
    invitation_hash: &str,
    email: &str,
    name: Option<&str>,
    password_hash: &str,
) -> Result<Option<User>> {
    let mut transaction = pool.begin().await?;
    let invitation_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM registration_invitations \
         WHERE token_hash = $1 AND revoked_at IS NULL AND accepted_at IS NULL \
           AND expires_at > now() AND (email IS NULL OR lower(email) = lower($2)) \
         FOR UPDATE",
    )
    .bind(invitation_hash)
    .bind(email)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(invitation_id) = invitation_id else {
        return Ok(None);
    };

    let sql = format!(
        "INSERT INTO users (id, email, name, role, password_hash) \
         VALUES ($1, $2, $3, 'user', $4) RETURNING {USER_COLUMNS}"
    );
    let user = sqlx::query_as::<_, User>(&sql)
        .bind(Uuid::now_v7())
        .bind(email)
        .bind(name)
        .bind(password_hash)
        .fetch_one(&mut *transaction)
        .await?;
    subscribe_featured_catalog(&mut transaction, user.id).await?;
    sqlx::query(
        "UPDATE registration_invitations SET accepted_by = $2, accepted_at = now() WHERE id = $1",
    )
    .bind(invitation_id)
    .bind(user.id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Some(user))
}

async fn subscribe_featured_catalog(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> Result<()> {
    let feed_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT feed_id FROM feed_catalog_entries WHERE published AND featured \
         ORDER BY sort_order, title",
    )
    .fetch_all(&mut **transaction)
    .await?;
    for feed_id in feed_ids {
        sqlx::query(
            "INSERT INTO user_feeds (id, user_id, feed_id) VALUES ($1, $2, $3) \
             ON CONFLICT (user_id, feed_id) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(feed_id)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

/// Look up a user by email.
pub async fn find_by_email(pool: &PgPool, email: &str) -> Result<Option<User>> {
    let sql = format!("SELECT {USER_COLUMNS} FROM users WHERE email = $1");
    Ok(sqlx::query_as::<_, User>(&sql)
        .bind(email)
        .fetch_optional(pool)
        .await?)
}

/// Look up a user by id.
pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<User>> {
    let sql = format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1");
    Ok(sqlx::query_as::<_, User>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// Total number of users (used for the "first user becomes admin" bootstrap).
pub async fn count(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users")
        .fetch_one(pool)
        .await?)
}

pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<User>> {
    let sql = format!("SELECT {USER_COLUMNS} FROM users ORDER BY created_at DESC LIMIT $1");
    Ok(sqlx::query_as::<_, User>(&sql)
        .bind(limit)
        .fetch_all(pool)
        .await?)
}

/// Record that a user was just seen, throttled to at most once per minute per
/// user so busy sessions do not write on every request. Best-effort.
pub async fn touch_last_seen(pool: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE users SET last_seen_at = now() \
         WHERE id = $1 AND (last_seen_at IS NULL OR last_seen_at < now() - interval '1 minute')",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark an account's email address as verified (idempotent).
pub async fn mark_email_verified(pool: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE users SET email_verified_at = COALESCE(email_verified_at, now()), updated_at = now() \
         WHERE id = $1",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a failed local login. When `max_failures > 0` and the running count
/// reaches it, lock the account for `lockout_seconds` and reset the counter so
/// the next window starts fresh (FND-191).
pub async fn record_failed_login(
    pool: &PgPool,
    user_id: Uuid,
    max_failures: i32,
    lockout_seconds: i64,
) -> Result<()> {
    if max_failures <= 0 {
        return Ok(());
    }
    sqlx::query(
        "UPDATE users SET \
           failed_login_count = CASE WHEN failed_login_count + 1 >= $2 THEN 0 ELSE failed_login_count + 1 END, \
           locked_until = CASE WHEN failed_login_count + 1 >= $2 THEN now() + make_interval(secs => $3) ELSE locked_until END, \
           updated_at = now() \
         WHERE id = $1",
    )
    .bind(user_id)
    .bind(max_failures)
    .bind(lockout_seconds as f64)
    .execute(pool)
    .await?;
    Ok(())
}

/// Clear the failure counter and any lock after a successful login.
pub async fn clear_failed_logins(pool: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE users SET failed_login_count = 0, locked_until = NULL, updated_at = now() \
         WHERE id = $1 AND (failed_login_count <> 0 OR locked_until IS NOT NULL)",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn count_admins(pool: &PgPool) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'admin'")
            .fetch_one(pool)
            .await?,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleChange {
    Changed,
    TargetNotFound,
    InvalidRole,
    CannotDemoteSelf,
    LastAdmin,
}

pub async fn change_role(
    pool: &PgPool,
    actor_id: Uuid,
    target_id: Uuid,
    role: &str,
) -> Result<RoleChange> {
    if !matches!(role, "admin" | "user") {
        return Ok(RoleChange::InvalidRole);
    }
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(415943191::bigint)")
        .execute(&mut *transaction)
        .await?;
    let sql = format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1 FOR UPDATE");
    let target = sqlx::query_as::<_, User>(&sql)
        .bind(target_id)
        .fetch_optional(&mut *transaction)
        .await?;
    let Some(target) = target else {
        return Ok(RoleChange::TargetNotFound);
    };
    if target.id == actor_id && target.role == "admin" && role != "admin" {
        return Ok(RoleChange::CannotDemoteSelf);
    }
    if target.role == "admin" && role != "admin" {
        let admin_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'admin'")
                .fetch_one(&mut *transaction)
                .await?;
        if admin_count <= 1 {
            return Ok(RoleChange::LastAdmin);
        }
    }
    sqlx::query("UPDATE users SET role = $2, updated_at = now() WHERE id = $1")
        .bind(target_id)
        .bind(role)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(RoleChange::Changed)
}

/// Change a user's role.
pub async fn set_role(pool: &PgPool, id: Uuid, role: &str) -> Result<()> {
    sqlx::query("UPDATE users SET role = $2 WHERE id = $1")
        .bind(id)
        .bind(role)
        .execute(pool)
        .await?;
    Ok(())
}
