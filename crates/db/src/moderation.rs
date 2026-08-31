//! Moderation audit trail (docs/PUBLIC_LAUNCH_PLAN.md §4.6, §5). Mirrors the
//! acquisition-profile audit pattern: every takedown/block records who/what/why.

use serde_json::Value as Json;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

/// One moderation action, joined with the actor email for display.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AuditRow {
    pub id: Uuid,
    pub actor_email: Option<String>,
    pub action: String,
    pub target_kind: Option<String>,
    pub target: Option<String>,
    pub reason: Option<String>,
    pub created_at: OffsetDateTime,
}

/// Record a moderation action.
#[allow(clippy::too_many_arguments)]
pub async fn record(
    pool: &PgPool,
    actor_id: Option<Uuid>,
    action: &str,
    target_kind: Option<&str>,
    target: Option<&str>,
    reason: Option<&str>,
    detail: Json,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO moderation_audit (id, actor_id, action, target_kind, target, reason, detail) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(actor_id)
    .bind(action)
    .bind(target_kind)
    .bind(target)
    .bind(reason)
    .bind(detail)
    .execute(pool)
    .await?;
    Ok(())
}

/// Recent moderation actions for the admin queue (newest first).
pub async fn recent(pool: &PgPool, limit: i64) -> Result<Vec<AuditRow>> {
    let limit = limit.clamp(1, 200);
    Ok(sqlx::query_as(
        "SELECT m.id, u.email AS actor_email, m.action, m.target_kind, m.target, m.reason, \
                m.created_at \
         FROM moderation_audit m LEFT JOIN users u ON u.id = m.actor_id \
         ORDER BY m.created_at DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}
