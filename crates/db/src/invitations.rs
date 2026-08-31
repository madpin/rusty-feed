//! Registration invitation repository. Raw bearer tokens never enter the
//! database; callers generate them and pass only a SHA-256 hash.

use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::error::Result;

/// An administrator-visible invitation without its bearer credential.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Invitation {
    pub id: Uuid,
    pub email: Option<String>,
    pub created_by: Uuid,
    pub expires_at: OffsetDateTime,
    pub accepted_by: Option<Uuid>,
    pub accepted_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

const COLUMNS: &str =
    "id, email, created_by, expires_at, accepted_by, accepted_at, revoked_at, created_at";

/// Create a one-use invitation with a bounded lifetime.
pub async fn create(
    pool: &PgPool,
    created_by: Uuid,
    token_hash: &str,
    email: Option<&str>,
    lifetime_days: i64,
) -> Result<Invitation> {
    let expires_at = OffsetDateTime::now_utc() + Duration::days(lifetime_days.clamp(1, 90));
    let sql = format!(
        "INSERT INTO registration_invitations (id, token_hash, email, created_by, expires_at) \
         VALUES ($1, $2, $3, $4, $5) RETURNING {COLUMNS}"
    );
    Ok(sqlx::query_as(&sql)
        .bind(Uuid::now_v7())
        .bind(token_hash)
        .bind(email)
        .bind(created_by)
        .bind(expires_at)
        .fetch_one(pool)
        .await?)
}

/// List recent invitations for administration. Expired/accepted rows remain as
/// a bounded audit trail until general retention cleanup removes them.
pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<Invitation>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM registration_invitations \
         ORDER BY created_at DESC LIMIT $1"
    );
    Ok(sqlx::query_as(&sql)
        .bind(limit.clamp(1, 500))
        .fetch_all(pool)
        .await?)
}

/// Return whether an invitation is currently usable for `email`.
pub async fn is_valid(pool: &PgPool, token_hash: &str, email: Option<&str>) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM registration_invitations \
         WHERE token_hash = $1 AND revoked_at IS NULL AND accepted_at IS NULL \
           AND expires_at > now() AND ($2::text IS NULL OR email IS NULL OR lower(email) = lower($2)))",
    )
    .bind(token_hash)
    .bind(email)
    .fetch_one(pool)
    .await?)
}

/// Revoke an unaccepted invitation. The actor check belongs to the admin web
/// boundary; all administrators may manage instance invitations.
pub async fn revoke(pool: &PgPool, id: Uuid) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE registration_invitations SET revoked_at = now() \
         WHERE id = $1 AND accepted_at IS NULL AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}
