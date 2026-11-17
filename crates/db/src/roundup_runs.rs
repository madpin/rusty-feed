//! Per-user Roundup execution history. Rows reference shared content by ID/key
//! and never duplicate its headline, body, or source metadata.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use rusty_feed_core::roundup::{RoundupKey, RoundupPeriod};

use crate::error::{DbError, Result};
use crate::roundup_schedules::RoundupSchedule;
use crate::roundups::RoundupScope;

const RUN_COLUMNS: &str = "id, user_id, schedule_id, scope_kind, scope_id, period, \
    roundup_id, input_hash, model_version, status, skip_reason, scheduled_for, emailed_at, \
    created_at, updated_at, \
    (SELECT headline FROM roundups WHERE roundups.id = roundup_runs.roundup_id) AS roundup_headline";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RoundupRun {
    pub id: Uuid,
    pub user_id: Uuid,
    pub schedule_id: Option<Uuid>,
    pub scope_kind: String,
    pub scope_id: Uuid,
    pub period: String,
    pub roundup_id: Option<Uuid>,
    pub input_hash: Option<String>,
    pub model_version: Option<String>,
    pub status: String,
    pub skip_reason: Option<String>,
    pub scheduled_for: Option<OffsetDateTime>,
    pub emailed_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub roundup_headline: Option<String>,
}

impl RoundupRun {
    pub fn scope(&self) -> Option<RoundupScope> {
        RoundupScope::parse(&self.scope_kind, self.scope_id)
    }

    pub fn roundup_period(&self) -> Option<RoundupPeriod> {
        RoundupPeriod::parse(&self.period)
    }

    pub fn key(&self) -> Option<RoundupKey> {
        Some(RoundupKey {
            input_hash: self.input_hash.clone()?,
            period: self.roundup_period()?,
            model_version: self.model_version.clone()?,
        })
    }
}

/// Personal history values. `status='ready'` requires `roundup_id`; skipped or
/// failed states require a safe, user-facing reason.
pub struct NewRoundupRun<'a> {
    pub user_id: Uuid,
    pub schedule_id: Option<Uuid>,
    pub scope: RoundupScope,
    pub period: RoundupPeriod,
    pub status: &'a str,
    pub key: Option<&'a RoundupKey>,
    pub roundup_id: Option<Uuid>,
    pub skip_reason: Option<&'a str>,
    pub scheduled_for: Option<OffsetDateTime>,
}

/// Append one manual run or a fully prepared scheduled result.
pub async fn record(pool: &PgPool, new: NewRoundupRun<'_>) -> Result<RoundupRun> {
    let id = Uuid::now_v7();
    insert(pool, id, &new).await?;
    find_for_user(pool, new.user_id, id)
        .await?
        .ok_or(DbError::NotFound)
}

async fn insert(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    new: &NewRoundupRun<'_>,
) -> Result<Option<Uuid>> {
    let (feed_id, folder_id) = match new.scope {
        RoundupScope::Feed(id) => (Some(id), None),
        RoundupScope::Folder(id) => (None, Some(id)),
    };
    Ok(sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO roundup_runs \
           (id, user_id, schedule_id, scope_kind, feed_id, folder_id, period, roundup_id, \
            input_hash, model_version, status, skip_reason, scheduled_for) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
         ON CONFLICT (schedule_id, scheduled_for) DO NOTHING RETURNING id",
    )
    .bind(id)
    .bind(new.user_id)
    .bind(new.schedule_id)
    .bind(new.scope.as_str())
    .bind(feed_id)
    .bind(folder_id)
    .bind(new.period.as_str())
    .bind(new.roundup_id)
    .bind(new.key.map(|key| &key.input_hash))
    .bind(new.key.map(|key| &key.model_version))
    .bind(new.status)
    .bind(new.skip_reason)
    .bind(new.scheduled_for)
    .fetch_optional(executor)
    .await?)
}

/// Atomically claim one intended schedule window. The unique schedule/window
/// row prevents double processing; `last_run_at` moves to actual attempt time
/// so missed windows are not replayed in a catch-up storm after downtime.
pub async fn claim_window(
    pool: &PgPool,
    schedule: &RoundupSchedule,
    scheduled_for: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<Option<RoundupRun>> {
    let Some(scope) = schedule.scope() else {
        return Ok(None);
    };
    let Some(period) = schedule.roundup_period() else {
        return Ok(None);
    };
    let id = Uuid::now_v7();
    let new = NewRoundupRun {
        user_id: schedule.user_id,
        schedule_id: Some(schedule.id),
        scope,
        period,
        status: "pending",
        key: None,
        roundup_id: None,
        skip_reason: None,
        scheduled_for: Some(scheduled_for),
    };
    let mut tx = pool.begin().await?;
    let inserted = insert(&mut *tx, id, &new).await?;
    if inserted.is_none() {
        return Ok(None);
    }
    let touched = sqlx::query(
        "UPDATE roundup_schedules SET last_run_at = $3 \
         WHERE id = $1 AND user_id = $2 AND enabled",
    )
    .bind(schedule.id)
    .bind(schedule.user_id)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if touched == 0 {
        return Ok(None);
    }
    tx.commit().await?;
    find_for_user(pool, schedule.user_id, id).await
}

pub async fn find_for_user(pool: &PgPool, user_id: Uuid, id: Uuid) -> Result<Option<RoundupRun>> {
    let sql = format!("SELECT {RUN_COLUMNS} FROM roundup_runs WHERE user_id = $1 AND id = $2");
    Ok(sqlx::query_as(&sql)
        .bind(user_id)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// Move a claimed run to its prepared state, scoped to its owner.
pub async fn finish(
    pool: &PgPool,
    user_id: Uuid,
    id: Uuid,
    status: &str,
    key: Option<&RoundupKey>,
    roundup_id: Option<Uuid>,
    skip_reason: Option<&str>,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE roundup_runs SET status = $3, input_hash = $4, model_version = $5, \
            roundup_id = $6, skip_reason = $7 \
         WHERE id = $1 AND user_id = $2 AND status IN ('pending', 'queued')",
    )
    .bind(id)
    .bind(user_id)
    .bind(status)
    .bind(key.map(|key| &key.input_hash))
    .bind(key.map(|key| &key.model_version))
    .bind(roundup_id)
    .bind(skip_reason)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list_for_user(pool: &PgPool, user_id: Uuid, limit: i64) -> Result<Vec<RoundupRun>> {
    let sql = format!(
        "SELECT {RUN_COLUMNS} FROM roundup_runs WHERE user_id = $1 \
         ORDER BY created_at DESC, id DESC LIMIT $2"
    );
    Ok(sqlx::query_as(&sql)
        .bind(user_id)
        .bind(limit.clamp(1, 50))
        .fetch_all(pool)
        .await?)
}

/// Bounded cross-user pending work used only for state reconciliation. It
/// never causes generation; a shared cache hit updates each personal row.
pub async fn unfinished(pool: &PgPool, limit: i64) -> Result<Vec<RoundupRun>> {
    let sql = format!(
        "SELECT {RUN_COLUMNS} FROM roundup_runs WHERE status IN ('pending', 'queued') \
         ORDER BY created_at LIMIT $1"
    );
    Ok(sqlx::query_as(&sql)
        .bind(limit.clamp(1, 500))
        .fetch_all(pool)
        .await?)
}

/// Mark every matching personal wait ready after the one shared row appears.
pub async fn mark_ready_for_key(
    pool: &PgPool,
    user_id: Uuid,
    key: &RoundupKey,
    roundup_id: Uuid,
) -> Result<u64> {
    let result = sqlx::query(
        "UPDATE roundup_runs SET status = 'ready', roundup_id = $5, skip_reason = NULL \
         WHERE user_id = $1 AND input_hash = $2 AND period = $3 AND model_version = $4 \
           AND status IN ('pending', 'queued')",
    )
    .bind(user_id)
    .bind(&key.input_hash)
    .bind(key.period.as_str())
    .bind(&key.model_version)
    .bind(roundup_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Delete one bounded batch of old personal history. Shared Roundups remain.
pub async fn purge_older_than(pool: &PgPool, days: u32) -> Result<u64> {
    let result = sqlx::query(
        "WITH old AS (SELECT id FROM roundup_runs \
            WHERE created_at < now() - make_interval(days => $1::int) \
            ORDER BY created_at LIMIT 5000 FOR UPDATE SKIP LOCKED) \
         DELETE FROM roundup_runs run USING old WHERE run.id = old.id",
    )
    .bind(i32::try_from(days).unwrap_or(i32::MAX))
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Shared content needed for one opted-in delivery, plus the personal key that
/// constructs the off-site Roundup link. No content is copied into history.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RoundupMail {
    pub id: Uuid,
    pub user_id: Uuid,
    pub email: String,
    pub scope_kind: String,
    pub scope_id: Uuid,
    pub period: String,
    pub input_hash: String,
    pub model_version: String,
    pub headline: String,
    pub body_markdown: String,
}

impl RoundupMail {
    pub fn key(&self) -> Option<RoundupKey> {
        Some(RoundupKey {
            input_hash: self.input_hash.clone(),
            period: RoundupPeriod::parse(&self.period)?,
            model_version: self.model_version.clone(),
        })
    }

    pub fn scope(&self) -> Option<RoundupScope> {
        RoundupScope::parse(&self.scope_kind, self.scope_id)
    }
}

/// Claim one ready run for email. Disabling the schedule or opting out before
/// this claim prevents delivery; a transport failure releases the claim.
pub async fn claim_pending_email(pool: &PgPool) -> Result<Option<RoundupMail>> {
    Ok(sqlx::query_as(
        "WITH candidate AS ( \
             SELECT run.id FROM roundup_runs run \
             JOIN roundup_schedules schedule ON schedule.id = run.schedule_id \
               AND schedule.user_id = run.user_id \
             JOIN users ON users.id = run.user_id \
             WHERE run.status = 'ready' AND run.emailed_at IS NULL \
               AND schedule.enabled AND schedule.email_delivery AND users.is_demo = false \
             ORDER BY run.created_at LIMIT 1 FOR UPDATE OF run SKIP LOCKED \
         ) UPDATE roundup_runs run SET emailed_at = now() \
         FROM candidate, users, roundups \
         WHERE run.id = candidate.id AND users.id = run.user_id \
           AND roundups.id = run.roundup_id \
         RETURNING run.id, run.user_id, users.email, run.scope_kind, run.scope_id, run.period, \
           run.input_hash, run.model_version, roundups.headline, roundups.body_markdown",
    )
    .fetch_optional(pool)
    .await?)
}

pub async fn release_email_claim(pool: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query("UPDATE roundup_runs SET emailed_at = NULL WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
