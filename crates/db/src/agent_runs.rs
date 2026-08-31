//! Agent run records (M7): one row per agent execution for observability.

use serde_json::Value as Json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;

/// Start a run; returns its id.
pub async fn start(pool: &PgPool, agent_name: &str, trigger: &str) -> Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agent_runs (id, agent_name, status, trigger) VALUES ($1, $2, 'running', $3)",
    )
    .bind(id)
    .bind(agent_name)
    .bind(trigger)
    .execute(pool)
    .await?;
    Ok(id)
}

/// Finish a run with `success` or `failed`, its duration, stats, and any error.
pub async fn finish(
    pool: &PgPool,
    id: Uuid,
    status: &str,
    duration_ms: i64,
    stats: Option<Json>,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE agent_runs SET status = $2, completed_at = now(), duration_ms = $3, \
         stats = $4, error_message = $5, error_message_is_safe = ($5 IS NOT NULL) WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(duration_ms)
    .bind(stats)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RecentAgentRun {
    pub agent_name: String,
    pub status: String,
    pub started_at: time::OffsetDateTime,
    pub duration_ms: Option<i64>,
    pub stats: Option<Json>,
    pub has_error: bool,
    pub error_message: Option<String>,
}

pub async fn list_recent(pool: &PgPool, limit: i64) -> Result<Vec<RecentAgentRun>> {
    Ok(sqlx::query_as(
        "SELECT agent_name, status, started_at, duration_ms, stats, \
         error_message IS NOT NULL AS has_error, \
         CASE WHEN error_message_is_safe THEN error_message END AS error_message \
         FROM agent_runs ORDER BY started_at DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Count runs for an agent with a given status (tests/inspection).
pub async fn count_by_status(pool: &PgPool, agent_name: &str, status: &str) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM agent_runs WHERE agent_name = $1 AND status = $2")
            .bind(agent_name)
            .bind(status)
            .fetch_one(pool)
            .await?,
    )
}
