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

/// Change a user's role.
pub async fn set_role(pool: &PgPool, id: Uuid, role: &str) -> Result<()> {
    sqlx::query("UPDATE users SET role = $2 WHERE id = $1")
        .bind(id)
        .bind(role)
        .execute(pool)
        .await?;
    Ok(())
}
