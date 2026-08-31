//! WebAuthn credentials and one-use server-side challenge state.

use serde_json::Value;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::Result;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PasskeyCredential {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub credential_id: Vec<u8>,
    pub passkey: Value,
    pub created_at: OffsetDateTime,
    pub last_used_at: Option<OffsetDateTime>,
}

const COLUMNS: &str = "id, user_id, name, credential_id, passkey, created_at, last_used_at";

pub async fn list(pool: &PgPool, user_id: Uuid) -> Result<Vec<PasskeyCredential>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM passkey_credentials WHERE user_id = $1 ORDER BY created_at DESC"
    );
    Ok(sqlx::query_as(&sql).bind(user_id).fetch_all(pool).await?)
}

pub async fn create(
    pool: &PgPool,
    user_id: Uuid,
    name: &str,
    credential_id: &[u8],
    passkey: &Value,
) -> Result<PasskeyCredential> {
    let sql = format!(
        "INSERT INTO passkey_credentials (id, user_id, name, credential_id, passkey) \
         VALUES ($1,$2,$3,$4,$5) RETURNING {COLUMNS}"
    );
    Ok(sqlx::query_as(&sql)
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(name)
        .bind(credential_id)
        .bind(passkey)
        .fetch_one(pool)
        .await?)
}

pub async fn delete(pool: &PgPool, id: Uuid, user_id: Uuid) -> Result<bool> {
    Ok(
        sqlx::query("DELETE FROM passkey_credentials WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(pool)
            .await?
            .rows_affected()
            > 0,
    )
}

pub async fn update_after_authentication(
    pool: &PgPool,
    id: Uuid,
    user_id: Uuid,
    passkey: &Value,
) -> Result<()> {
    sqlx::query(
        "UPDATE passkey_credentials SET passkey = $3, last_used_at = now() \
         WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .bind(passkey)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn create_ceremony(
    pool: &PgPool,
    user_id: Uuid,
    kind: &str,
    state: &Value,
) -> Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO passkey_ceremonies (id, user_id, kind, state, expires_at) \
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(id)
    .bind(user_id)
    .bind(kind)
    .bind(state)
    .bind(OffsetDateTime::now_utc() + Duration::minutes(5))
    .execute(pool)
    .await?;
    Ok(id)
}

/// Atomically consume a challenge so successful or failed finish requests
/// cannot replay it.
pub async fn take_ceremony(
    pool: &PgPool,
    id: Uuid,
    user_id: Uuid,
    kind: &str,
) -> Result<Option<Value>> {
    Ok(sqlx::query_scalar(
        "DELETE FROM passkey_ceremonies WHERE id = $1 AND user_id = $2 AND kind = $3 \
         AND expires_at > now() RETURNING state",
    )
    .bind(id)
    .bind(user_id)
    .bind(kind)
    .fetch_optional(pool)
    .await?)
}

pub async fn take_authentication_ceremony(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<(Uuid, Value)>> {
    Ok(sqlx::query_as(
        "DELETE FROM passkey_ceremonies WHERE id = $1 AND kind = 'authentication' \
         AND expires_at > now() RETURNING user_id, state",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

pub async fn cleanup_ceremonies(pool: &PgPool) -> Result<u64> {
    Ok(
        sqlx::query("DELETE FROM passkey_ceremonies WHERE expires_at <= now()")
            .execute(pool)
            .await?
            .rows_affected(),
    )
}
