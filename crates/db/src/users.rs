//! User repository.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;
use crate::models::User;

const USER_COLUMNS: &str =
    "id, email, name, image, role, password_hash, email_verified_at, created_at, updated_at";

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
