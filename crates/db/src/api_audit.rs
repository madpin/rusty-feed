//! Secret-free native API mutation audit.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::Result;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ApiAuditEvent {
    pub operation: String,
    pub user_email: String,
    pub token_name: Option<String>,
    pub created_at: OffsetDateTime,
}

pub async fn record(pool: &PgPool, user_id: Uuid, token_id: Uuid, operation: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO api_audit_events (id, user_id, token_id, operation) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(token_id)
    .bind(operation)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn recent(pool: &PgPool, limit: i64) -> Result<Vec<ApiAuditEvent>> {
    Ok(sqlx::query_as(
        "SELECT event.operation, users.email AS user_email, api_tokens.name AS token_name, event.created_at \
         FROM api_audit_events event JOIN users ON users.id = event.user_id \
         LEFT JOIN api_tokens ON api_tokens.id = event.token_id \
         ORDER BY event.created_at DESC LIMIT $1",
    )
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await?)
}

pub async fn delete_older_than(pool: &PgPool, days: i64) -> Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM api_audit_events WHERE created_at < now() - make_interval(days => $1::int)",
    )
    .bind(i32::try_from(days.clamp(1, 3_650)).unwrap_or(90))
    .execute(pool)
    .await?
    .rows_affected())
}
