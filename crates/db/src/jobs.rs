//! Durable job queue (M7). Consumers claim work with `FOR UPDATE SKIP LOCKED`
//! so many workers can run in parallel without double-processing, and an optional
//! `dedup_key` guarantees one active job per key (O(1) shared work).

use serde_json::Value as Json;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use rusty_feed_core::html::ARTICLE_PAGE_PROCESSING_VERSION;

use crate::error::Result;
use crate::models::Job;

/// Current queue counters shown to administrators.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct QueueTotals {
    pub pending_due: i64,
    pub pending_scheduled: i64,
    pub running: i64,
    pub stalled: i64,
    pub finished_last_hour: i64,
    pub dead: i64,
    pub background_pending: i64,
    pub background_running: i64,
    pub interactive_pending: i64,
    pub interactive_running: i64,
}

/// Queue counters grouped by safe job kind. Payloads and errors are excluded.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct QueueKindSummary {
    pub queue_name: String,
    pub kind: String,
    pub pending_due: i64,
    pub pending_scheduled: i64,
    pub running: i64,
    pub stalled: i64,
    pub dead: i64,
}

/// Safe queue item projection for active and recently-finished tables.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct QueueItemSummary {
    pub queue_name: String,
    pub kind: String,
    pub status: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub run_at: OffsetDateTime,
    pub locked_by: Option<String>,
    pub worker_online: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub last_error: Option<String>,
    /// Human-readable subject of the job (feed name for `fetch_feed`, article
    /// title for `extract_content`/`embed_article`/`summarize_article`), resolved
    /// from the payload's `feed_id`/`article_id`. Never the raw payload.
    pub target: Option<String>,
}

/// A worker process that has sent a recent heartbeat.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OnlineWorker {
    pub worker_id: String,
    pub concurrency: i32,
    pub interactive_concurrency: i32,
    pub scheduler_enabled: bool,
    pub started_at: OffsetDateTime,
    pub last_heartbeat_at: OffsetDateTime,
}

/// A durable execution lane. Interactive work has reserved consumers so it
/// cannot be starved by feed refreshes or bulk enrichment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueLane {
    Background,
    Interactive,
}

impl QueueLane {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Interactive => "interactive",
        }
    }
}

/// Status projection used by user-facing progress endpoints. Payloads are
/// deliberately excluded.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct JobStatus {
    pub status: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub run_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

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
    enqueue_in_lane(
        pool,
        QueueLane::Background,
        kind,
        payload,
        dedup_key,
        run_at,
        max_attempts,
    )
    .await
}

/// Enqueue a job in an explicit execution lane.
pub async fn enqueue_in_lane(
    pool: &PgPool,
    lane: QueueLane,
    kind: &str,
    payload: Json,
    dedup_key: Option<&str>,
    run_at: Option<OffsetDateTime>,
    max_attempts: i32,
) -> Result<Option<Uuid>> {
    let id: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO job_queue (id, queue_name, kind, payload, dedup_key, run_at, max_attempts) \
         VALUES ($1, $2, $3, $4, $5, COALESCE($6, now()), $7) \
         ON CONFLICT DO NOTHING RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(lane.as_str())
    .bind(kind)
    .bind(payload)
    .bind(dedup_key)
    .bind(run_at)
    .bind(max_attempts)
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

/// Enqueue a bounded batch of missing/expired public-page extractions for one
/// shared feed in one database round-trip. Never-attempted articles are chosen
/// before cooled-down failures so one bad page cannot starve the backfill.
pub async fn enqueue_extractions_for_feed(
    pool: &PgPool,
    feed_id: Uuid,
    limit: usize,
) -> Result<u64> {
    if limit == 0 {
        return Ok(0);
    }
    let job_ids: Vec<Uuid> = (0..limit).map(|_| Uuid::now_v7()).collect();
    let result = sqlx::query(
        "WITH candidates AS ( \
             SELECT article.id AS article_id, article.content_revision, \
                    row_number() OVER (ORDER BY \
                        CASE WHEN article.extraction_status = 'none' THEN 0 ELSE 1 END, \
                        article.created_at DESC) AS ordinal \
             FROM articles article \
             WHERE article.feed_id = $1 \
               AND (article.url LIKE 'http://%' OR article.url LIKE 'https://%') \
               AND (article.article_page_content IS NULL OR article.article_page_expires_at <= now() \
                    OR article.article_page_processing_version < $3) \
               AND (article.extraction_status <> 'failed' \
                    OR article.updated_at < now() - interval '15 minutes') \
               AND NOT EXISTS (SELECT 1 FROM job_queue existing \
                    WHERE existing.dedup_key = 'extract:' || article.id::text || ':' || article.content_revision::text \
                      AND existing.status IN ('pending', 'running')) \
             LIMIT cardinality($2::uuid[]) \
         ), generated_ids AS ( \
             SELECT id, ordinal FROM unnest($2::uuid[]) WITH ORDINALITY AS generated(id, ordinal) \
         ) \
         INSERT INTO job_queue (id, kind, payload, dedup_key, max_attempts) \
         SELECT generated_ids.id, 'extract_content', \
                jsonb_build_object('article_id', candidates.article_id, \
                                   'content_revision', candidates.content_revision), \
                'extract:' || candidates.article_id::text || ':' || candidates.content_revision::text, 3 \
         FROM candidates JOIN generated_ids USING (ordinal) \
         ON CONFLICT DO NOTHING",
    )
    .bind(feed_id)
    .bind(job_ids)
    .bind(ARTICLE_PAGE_PROCESSING_VERSION)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Claim the next due job (optionally restricted to `kinds`), marking it running
/// and incrementing `attempts`. Uses `SKIP LOCKED` for safe concurrency.
pub async fn claim_next(pool: &PgPool, worker_id: &str, kinds: &[String]) -> Result<Option<Job>> {
    claim_next_in_lane(pool, worker_id, QueueLane::Background, kinds).await
}

/// Claim the next due job from one execution lane.
pub async fn claim_next_in_lane(
    pool: &PgPool,
    worker_id: &str,
    lane: QueueLane,
    kinds: &[String],
) -> Result<Option<Job>> {
    let mut tx = pool.begin().await?;

    // `kinds` empty means "any kind".
    let selected: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM job_queue \
         WHERE status = 'pending' AND run_at <= now() AND queue_name = $2 \
           AND (cardinality($1::text[]) = 0 OR kind = ANY($1)) \
         ORDER BY run_at \
         FOR UPDATE SKIP LOCKED \
         LIMIT 1",
    )
    .bind(kinds)
    .bind(lane.as_str())
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
         RETURNING id, queue_name, kind, payload, attempts, max_attempts",
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
            "UPDATE job_queue SET status = 'dead', last_error = $2, last_error_is_safe = true, locked_by = NULL, \
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
            "UPDATE job_queue SET status = 'pending', run_at = $3, last_error = $2, last_error_is_safe = true, \
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

/// Requeue a bounded set of dead-letter jobs for an explicit administrator retry.
pub async fn retry_dead(pool: &PgPool, limit: i64) -> Result<u64> {
    let result = sqlx::query(
        "WITH candidates AS (SELECT id FROM job_queue WHERE status = 'dead' \
             ORDER BY updated_at DESC LIMIT $1 FOR UPDATE SKIP LOCKED) \
         UPDATE job_queue job SET status = 'pending', attempts = 0, run_at = now(), \
             locked_at = NULL, locked_by = NULL, last_error = NULL, updated_at = now() \
         FROM candidates WHERE job.id = candidates.id",
    )
    .bind(limit.clamp(1, 500))
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Make delayed pending jobs for selected active feeds available immediately.
pub async fn run_pending_feed_fetches_now(pool: &PgPool, feed_ids: &[Uuid]) -> Result<u64> {
    if feed_ids.is_empty() {
        return Ok(0);
    }
    let res = sqlx::query(
        "UPDATE job_queue job SET run_at = now(), updated_at = now() \
         FROM feeds feed WHERE job.kind = 'fetch_feed' AND job.status = 'pending' \
           AND job.run_at > now() AND job.payload ->> 'feed_id' = feed.id::text \
           AND feed.id = ANY($1) AND feed.health_status <> 'disabled' AND feed.blocked_at IS NULL",
    )
    .bind(feed_ids)
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

pub async fn count_active(pool: &PgPool) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM job_queue WHERE status IN ('pending', 'running')")
            .fetch_one(pool)
            .await?,
    )
}

/// Register a worker process and its queue-consumer capacity.
pub async fn register_worker(
    pool: &PgPool,
    worker_id: &str,
    concurrency: usize,
    interactive_concurrency: usize,
    scheduler_enabled: bool,
) -> Result<()> {
    let background = i32::try_from(concurrency).unwrap_or(i32::MAX).max(1);
    let interactive = i32::try_from(interactive_concurrency)
        .unwrap_or(i32::MAX)
        .max(1);
    let concurrency = background.saturating_add(interactive);
    sqlx::query(
        "DELETE FROM job_worker_heartbeats \
         WHERE last_heartbeat_at < now() - interval '7 days'",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO job_worker_heartbeats \
         (worker_id, concurrency, interactive_concurrency, scheduler_enabled) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (worker_id) DO UPDATE SET concurrency = EXCLUDED.concurrency, \
             interactive_concurrency = EXCLUDED.interactive_concurrency, \
             scheduler_enabled = EXCLUDED.scheduler_enabled, started_at = now(), \
             last_heartbeat_at = now()",
    )
    .bind(worker_id)
    .bind(concurrency)
    .bind(interactive)
    .bind(scheduler_enabled)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark a registered worker as alive.
pub async fn heartbeat_worker(pool: &PgPool, worker_id: &str) -> Result<()> {
    sqlx::query("UPDATE job_worker_heartbeats SET last_heartbeat_at = now() WHERE worker_id = $1")
        .bind(worker_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Remove a worker registration during graceful shutdown.
pub async fn unregister_worker(pool: &PgPool, worker_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM job_worker_heartbeats WHERE worker_id = $1")
        .bind(worker_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Aggregate the queue without exposing payloads or persisted error text.
pub async fn queue_totals(pool: &PgPool) -> Result<QueueTotals> {
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'pending' AND run_at <= now()) AS pending_due, \
                count(*) FILTER (WHERE status = 'pending' AND run_at > now()) AS pending_scheduled, \
                count(*) FILTER (WHERE status = 'running' AND EXISTS ( \
                    SELECT 1 FROM job_worker_heartbeats worker \
                    WHERE worker.last_heartbeat_at >= now() - interval '15 seconds' \
                      AND starts_with(COALESCE(job_queue.locked_by, ''), worker.worker_id || '#'))) AS running, \
                count(*) FILTER (WHERE status = 'running' AND NOT EXISTS ( \
                    SELECT 1 FROM job_worker_heartbeats worker \
                    WHERE worker.last_heartbeat_at >= now() - interval '15 seconds' \
                      AND starts_with(COALESCE(job_queue.locked_by, ''), worker.worker_id || '#'))) AS stalled, \
                count(*) FILTER (WHERE status = 'done' AND updated_at >= now() - interval '1 hour') AS finished_last_hour, \
                count(*) FILTER (WHERE status = 'dead') AS dead, \
                count(*) FILTER (WHERE queue_name = 'background' AND status = 'pending') AS background_pending, \
                count(*) FILTER (WHERE queue_name = 'background' AND status = 'running') AS background_running, \
                count(*) FILTER (WHERE queue_name = 'interactive' AND status = 'pending') AS interactive_pending, \
                count(*) FILTER (WHERE queue_name = 'interactive' AND status = 'running') AS interactive_running \
         FROM job_queue",
    )
    .fetch_one(pool)
    .await?)
}

/// Aggregate non-finished queue work by job kind.
pub async fn queue_by_kind(pool: &PgPool) -> Result<Vec<QueueKindSummary>> {
    Ok(sqlx::query_as(
        "SELECT queue_name, kind, \
                count(*) FILTER (WHERE status = 'pending' AND run_at <= now()) AS pending_due, \
                count(*) FILTER (WHERE status = 'pending' AND run_at > now()) AS pending_scheduled, \
                count(*) FILTER (WHERE status = 'running' AND EXISTS ( \
                    SELECT 1 FROM job_worker_heartbeats worker \
                    WHERE worker.last_heartbeat_at >= now() - interval '15 seconds' \
                      AND starts_with(COALESCE(job_queue.locked_by, ''), worker.worker_id || '#'))) AS running, \
                count(*) FILTER (WHERE status = 'running' AND NOT EXISTS ( \
                    SELECT 1 FROM job_worker_heartbeats worker \
                    WHERE worker.last_heartbeat_at >= now() - interval '15 seconds' \
                      AND starts_with(COALESCE(job_queue.locked_by, ''), worker.worker_id || '#'))) AS stalled, \
                count(*) FILTER (WHERE status = 'dead') AS dead \
         FROM job_queue WHERE status IN ('pending', 'running', 'dead') \
         GROUP BY queue_name, kind ORDER BY queue_name DESC, \
             count(*) FILTER (WHERE status = 'running') DESC, \
             count(*) FILTER (WHERE status = 'pending' AND run_at <= now()) DESC, kind",
    )
    .fetch_all(pool)
    .await?)
}

/// SQL expression resolving a safe, human-readable subject for a queue row from
/// its payload id references (never the raw payload).
const JOB_TARGET_SQL: &str = "COALESCE(\
    (SELECT f.name FROM feeds f WHERE f.id::text = job_queue.payload->>'feed_id'), \
    (SELECT left(a.title, 120) FROM articles a WHERE a.id::text = job_queue.payload->>'article_id'))";

/// List due, scheduled, and running jobs (with a resolved subject, never the raw payload).
pub async fn list_active(pool: &PgPool, limit: i64) -> Result<Vec<QueueItemSummary>> {
    Ok(sqlx::query_as(&format!(
        "SELECT queue_name, kind, status, attempts, max_attempts, run_at, locked_by, created_at, updated_at, \
                CASE WHEN last_error_is_safe THEN last_error END AS last_error, \
                EXISTS (SELECT 1 FROM job_worker_heartbeats worker \
                    WHERE worker.last_heartbeat_at >= now() - interval '15 seconds' \
                      AND starts_with(COALESCE(job_queue.locked_by, ''), worker.worker_id || '#')) AS worker_online, \
                {JOB_TARGET_SQL} AS target \
         FROM job_queue WHERE status IN ('pending', 'running') \
         ORDER BY CASE WHEN status = 'running' THEN 0 WHEN run_at <= now() THEN 1 ELSE 2 END, \
                  run_at, created_at LIMIT $1",
    ))
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// List the most recently completed or dead-lettered queue items.
pub async fn list_recent_finished(pool: &PgPool, limit: i64) -> Result<Vec<QueueItemSummary>> {
    Ok(sqlx::query_as(&format!(
        "SELECT queue_name, kind, status, attempts, max_attempts, run_at, locked_by, created_at, updated_at, \
                CASE WHEN last_error_is_safe THEN last_error END AS last_error, \
                false AS worker_online, \
                {JOB_TARGET_SQL} AS target \
         FROM job_queue WHERE status IN ('done', 'dead') \
         ORDER BY updated_at DESC LIMIT $1",
    ))
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// List worker processes with heartbeats in the last 15 seconds.
pub async fn list_online_workers(pool: &PgPool) -> Result<Vec<OnlineWorker>> {
    Ok(sqlx::query_as(
        "SELECT worker_id, concurrency, interactive_concurrency, scheduler_enabled, started_at, last_heartbeat_at \
         FROM job_worker_heartbeats \
         WHERE last_heartbeat_at >= now() - interval '15 seconds' \
         ORDER BY started_at, worker_id",
    )
    .fetch_all(pool)
    .await?)
}

/// Rows deleted per statement by the retention purges.
pub const PURGE_BATCH_SIZE: i64 = 5_000;
/// Hard ceiling on rows one purge call deletes from one table; a larger backlog
/// is finished by the next scheduled run.
pub const PURGE_MAX_ROWS_PER_RUN: i64 = 500_000;

/// Batch bounds for a retention purge, so one run on a huge backlog never holds
/// one giant transaction.
#[derive(Debug, Clone, Copy)]
pub struct PurgeLimits {
    pub batch_size: i64,
    pub max_rows: i64,
}

impl Default for PurgeLimits {
    fn default() -> Self {
        Self {
            batch_size: PURGE_BATCH_SIZE,
            max_rows: PURGE_MAX_ROWS_PER_RUN,
        }
    }
}

/// Run a `DELETE` taking `$1` = retention days and `$2` = batch size until a
/// batch comes back short or `limits.max_rows` is reached. Zero days is a no-op.
/// Callers select ids with `id = ANY(ARRAY(... LIMIT $2))`: the equivalent
/// `id IN (subquery)` plans a full-table hash semi join on every batch.
pub(crate) async fn delete_in_batches(
    pool: &PgPool,
    sql: &str,
    retention_days: u32,
    limits: PurgeLimits,
) -> Result<u64> {
    if retention_days == 0 {
        return Ok(0);
    }
    let days = retention_days.min(36_500) as i32;
    let batch_size = limits.batch_size.max(1);
    let mut total = 0i64;
    while total < limits.max_rows {
        let limit = batch_size.min(limits.max_rows - total);
        let deleted = sqlx::query(sql)
            .bind(days)
            .bind(limit)
            .execute(pool)
            .await?
            .rows_affected() as i64;
        total += deleted;
        if deleted < limit {
            break;
        }
    }
    Ok(total as u64)
}

/// Delete `done` jobs last updated more than `retention_days` ago. `pending`,
/// `running` and `dead` jobs are never deleted (dead letters are operator
/// evidence until retried). A finished `summarize_article` job whose article
/// still has no summary is kept so the reader's summary-status endpoint keeps
/// reporting "finished without usable content". Zero days disables the purge.
pub async fn purge_done(pool: &PgPool, retention_days: u32) -> Result<u64> {
    purge_done_with(pool, retention_days, PurgeLimits::default()).await
}

/// [`purge_done`] with explicit batch bounds.
pub async fn purge_done_with(
    pool: &PgPool,
    retention_days: u32,
    limits: PurgeLimits,
) -> Result<u64> {
    delete_in_batches(
        pool,
        "DELETE FROM job_queue WHERE id = ANY(ARRAY( \
             SELECT job.id FROM job_queue job \
             WHERE job.status = 'done' AND job.updated_at < now() - make_interval(days => $1) \
               AND NOT (job.kind = 'summarize_article' AND EXISTS ( \
                   SELECT 1 FROM articles article \
                   WHERE article.id = CASE WHEN job.payload ->> 'article_id' ~* \
                       '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' \
                       THEN (job.payload ->> 'article_id')::uuid END \
                     AND article.summarized_at IS NULL)) \
             ORDER BY job.updated_at LIMIT $2 FOR UPDATE OF job SKIP LOCKED))",
        retention_days,
        limits,
    )
    .await
}

/// Return the most recent durable job for a dedup key.
pub async fn latest_by_dedup(pool: &PgPool, dedup_key: &str) -> Result<Option<JobStatus>> {
    Ok(sqlx::query_as(
        "SELECT status, attempts, max_attempts, run_at, updated_at \
         FROM job_queue WHERE dedup_key = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(dedup_key)
    .fetch_optional(pool)
    .await?)
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
