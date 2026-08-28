//! Durable job queue (M7). Consumers claim work with `FOR UPDATE SKIP LOCKED`
//! so many workers can run in parallel without double-processing, and an optional
//! `dedup_key` guarantees one active job per key (O(1) shared work).

use serde_json::Value as Json;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::error::Result;
use crate::models::Job;

/// Enqueue a job. When `dedup_key` is set and an active (pending/running) job
/// already exists for it, this is a no-op and returns `None`.
pub async fn enqueue(
    pool: &PgPool,
    kind: &str,
    payload: Json,
    dedup_key: Option<&str>,
    run_at: Option<OffsetDateTime>,
    max_attempts: i32,
) -> Result<Option<Uuid>> {
    let id: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO job_queue (id, kind, payload, dedup_key, run_at, max_attempts) \
         VALUES ($1, $2, $3, $4, COALESCE($5, now()), $6) \
         ON CONFLICT DO NOTHING RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(kind)
    .bind(payload)
    .bind(dedup_key)
    .bind(run_at)
    .bind(max_attempts)
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

/// Claim the next due job (optionally restricted to `kinds`), marking it running
/// and incrementing `attempts`. Uses `SKIP LOCKED` for safe concurrency.
pub async fn claim_next(pool: &PgPool, worker_id: &str, kinds: &[String]) -> Result<Option<Job>> {
    let mut tx = pool.begin().await?;

    // `kinds` empty means "any kind".
    let selected: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM job_queue \
         WHERE status = 'pending' AND run_at <= now() \
           AND (cardinality($1::text[]) = 0 OR kind = ANY($1)) \
         ORDER BY run_at \
         FOR UPDATE SKIP LOCKED \
         LIMIT 1",
    )
    .bind(kinds)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(id) = selected else {
        tx.commit().await?;
        return Ok(None);
    };

    let job = sqlx::query_as::<_, Job>(
        "UPDATE job_queue \
         SET status = 'running', locked_by = $2, locked_at = now(), \
             attempts = attempts + 1, updated_at = now() \
         WHERE id = $1 \
         RETURNING id, kind, payload, attempts, max_attempts",
    )
    .bind(id)
    .bind(worker_id)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Some(job))
}

/// Mark a job done.
pub async fn complete(pool: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE job_queue SET status = 'done', locked_by = NULL, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Outcome of failing a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailOutcome {
    /// Rescheduled for a future retry.
    Retried,
    /// Exhausted attempts; moved to the dead-letter state.
    Dead,
}

/// Record a job failure. Reschedules with the given backoff if attempts remain,
/// otherwise moves it to `dead`.
pub async fn fail(pool: &PgPool, job: &Job, error: &str, backoff: Duration) -> Result<FailOutcome> {
    if job.attempts >= job.max_attempts {
        sqlx::query(
            "UPDATE job_queue SET status = 'dead', last_error = $2, locked_by = NULL, \
             updated_at = now() WHERE id = $1",
        )
        .bind(job.id)
        .bind(error)
        .execute(pool)
        .await?;
        Ok(FailOutcome::Dead)
    } else {
        let run_at = OffsetDateTime::now_utc() + backoff;
        sqlx::query(
            "UPDATE job_queue SET status = 'pending', run_at = $3, last_error = $2, \
             locked_by = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(job.id)
        .bind(error)
        .bind(run_at)
        .execute(pool)
        .await?;
        Ok(FailOutcome::Retried)
    }
}

/// Reset jobs stuck in `running` (e.g. a worker crashed) back to `pending` so
/// they can be reclaimed. Returns how many were reset.
pub async fn reclaim_stale(pool: &PgPool, older_than: Duration) -> Result<u64> {
    let secs = older_than.whole_seconds().max(1) as f64;
    let res = sqlx::query(
        "UPDATE job_queue SET status = 'pending', locked_by = NULL, updated_at = now() \
         WHERE status = 'running' AND locked_at < now() - make_interval(secs => $1)",
    )
    .bind(secs)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Fetch a job's current status (tests/inspection).
pub async fn get_status(pool: &PgPool, id: Uuid) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT status FROM job_queue WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// Count active (pending/running) jobs for a dedup key — for tests/inspection.
pub async fn count_active_by_dedup(pool: &PgPool, dedup_key: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM job_queue \
         WHERE dedup_key = $1 AND status IN ('pending', 'running')",
    )
    .bind(dedup_key)
    .fetch_one(pool)
    .await?)
}
