//! Lease-based singleton locks (M7) so a scheduled agent runs on at most one
//! instance at a time. Leases expire, so a crashed holder can't deadlock others.

use sqlx::PgPool;
use time::Duration;

use crate::error::Result;

/// Try to acquire (or steal, if expired) the lease for `agent_name`. Returns
/// `true` if this `holder` now owns it.
pub async fn acquire(pool: &PgPool, agent_name: &str, holder: &str, ttl: Duration) -> Result<bool> {
    let ttl_secs = ttl.whole_seconds().max(1);
    let acquired: Option<String> = sqlx::query_scalar(
        "INSERT INTO agent_locks (agent_name, locked_by, locked_at, expires_at) \
         VALUES ($1, $2, now(), now() + make_interval(secs => $3)) \
         ON CONFLICT (agent_name) DO UPDATE \
           SET locked_by = EXCLUDED.locked_by, locked_at = now(), expires_at = EXCLUDED.expires_at \
           WHERE agent_locks.expires_at < now() \
         RETURNING agent_name",
    )
    .bind(agent_name)
    .bind(holder)
    .bind(ttl_secs as f64)
    .fetch_optional(pool)
    .await?;
    Ok(acquired.is_some())
}

/// Release a lease if held by `holder`.
pub async fn release(pool: &PgPool, agent_name: &str, holder: &str) -> Result<()> {
    sqlx::query("DELETE FROM agent_locks WHERE agent_name = $1 AND locked_by = $2")
        .bind(agent_name)
        .bind(holder)
        .execute(pool)
        .await?;
    Ok(())
}
