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
         stats = $4, error_message = $5 WHERE id = $1",
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
