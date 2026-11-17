//! Personal repeating Roundup schedules. Scope ownership is enforced both by
//! composite foreign keys and by every repository predicate.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use rusty_feed_core::roundup::{
    roundup_schedule_cap_reached, roundup_weekday, RoundupPeriod, RoundupTiming,
};

use crate::error::{DbError, Result};
use crate::roundups::RoundupScope;

const SCHEDULE_COLUMNS: &str = "schedule.id, schedule.user_id, schedule.scope_kind, \
    schedule.scope_id, schedule.period, schedule.local_hour, schedule.weekday, \
    schedule.timezone, schedule.email_delivery, schedule.enabled, schedule.last_run_at, \
    schedule.created_at, schedule.updated_at, \
    (SELECT run.status FROM roundup_runs run WHERE run.schedule_id = schedule.id \
       ORDER BY run.created_at DESC LIMIT 1) AS last_status, \
    (SELECT run.skip_reason FROM roundup_runs run WHERE run.schedule_id = schedule.id \
       ORDER BY run.created_at DESC LIMIT 1) AS last_skip_reason";

/// One personal schedule plus its latest history state.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RoundupSchedule {
    pub id: Uuid,
    pub user_id: Uuid,
    pub scope_kind: String,
    pub scope_id: Uuid,
    pub period: String,
    pub local_hour: i16,
    pub weekday: Option<i16>,
    pub timezone: String,
    pub email_delivery: bool,
    pub enabled: bool,
    pub last_run_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub last_status: Option<String>,
    pub last_skip_reason: Option<String>,
}

impl RoundupSchedule {
    pub fn scope(&self) -> Option<RoundupScope> {
        RoundupScope::parse(&self.scope_kind, self.scope_id)
    }

    pub fn roundup_period(&self) -> Option<RoundupPeriod> {
        RoundupPeriod::parse(&self.period)
    }

    pub fn timing(&self) -> Option<RoundupTiming> {
        Some(RoundupTiming {
            period: self.roundup_period()?,
            local_hour: u8::try_from(self.local_hour).ok()?,
            weekday: self
                .weekday
                .and_then(|value| u8::try_from(value).ok())
                .and_then(roundup_weekday),
            timezone: self.timezone.clone(),
        })
        .filter(RoundupTiming::is_valid)
    }

    /// Next intended instant after the last actual run (or schedule creation).
    pub fn next_run(&self) -> Option<OffsetDateTime> {
        self.timing()?
            .next_run_after(self.last_run_at.unwrap_or(self.created_at))
    }
}

/// Validated values for creating or replacing a schedule.
pub struct NewRoundupSchedule<'a> {
    pub user_id: Uuid,
    pub scope: RoundupScope,
    pub timing: &'a RoundupTiming,
    pub email_delivery: bool,
    pub enabled: bool,
}

pub enum CreateScheduleOutcome {
    Created(Box<RoundupSchedule>),
    CapReached,
    Conflict,
}

async fn scope_belongs_to_user(
    executor: impl sqlx::PgExecutor<'_>,
    user_id: Uuid,
    scope: RoundupScope,
) -> Result<bool> {
    let exists = match scope {
        RoundupScope::Feed(feed_id) => {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM user_feeds WHERE user_id = $1 AND feed_id = $2)",
            )
            .bind(user_id)
            .bind(feed_id)
            .fetch_one(executor)
            .await?
        }
        RoundupScope::Folder(folder_id) => {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM user_categories WHERE user_id = $1 AND id = $2)",
            )
            .bind(user_id)
            .bind(folder_id)
            .fetch_one(executor)
            .await?
        }
    };
    Ok(exists)
}

/// Create under a per-user write lock so concurrent requests cannot cross the
/// configured cap. `Conflict` means the same scope/period already repeats.
pub async fn create(
    pool: &PgPool,
    new: NewRoundupSchedule<'_>,
    limit: u32,
) -> Result<CreateScheduleOutcome> {
    let mut tx = pool.begin().await?;
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(new.user_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
    if !scope_belongs_to_user(&mut *tx, new.user_id, new.scope).await? {
        return Err(DbError::NotFound);
    }
    let existing: i64 =
        sqlx::query_scalar("SELECT count(*) FROM roundup_schedules WHERE user_id = $1")
            .bind(new.user_id)
            .fetch_one(&mut *tx)
            .await?;
    if roundup_schedule_cap_reached(existing as u64, limit) {
        return Ok(CreateScheduleOutcome::CapReached);
    }
    let (feed_id, folder_id) = match new.scope {
        RoundupScope::Feed(id) => (Some(id), None),
        RoundupScope::Folder(id) => (None, Some(id)),
    };
    let id = Uuid::now_v7();
    let inserted = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO roundup_schedules \
           (id, user_id, scope_kind, feed_id, folder_id, period, local_hour, weekday, \
            timezone, email_delivery, enabled) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
         ON CONFLICT (user_id, scope_kind, scope_id, period) DO NOTHING RETURNING id",
    )
    .bind(id)
    .bind(new.user_id)
    .bind(new.scope.as_str())
    .bind(feed_id)
    .bind(folder_id)
    .bind(new.timing.period.as_str())
    .bind(i16::from(new.timing.local_hour))
    .bind(
        new.timing
            .weekday
            .map(|day| i16::from(day.number_from_monday())),
    )
    .bind(&new.timing.timezone)
    .bind(new.email_delivery)
    .bind(new.enabled)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    match inserted {
        Some(_) => Ok(CreateScheduleOutcome::Created(Box::new(
            find_for_user(pool, new.user_id, id)
                .await?
                .ok_or(DbError::NotFound)?,
        ))),
        None => Ok(CreateScheduleOutcome::Conflict),
    }
}

pub async fn find_for_user(
    pool: &PgPool,
    user_id: Uuid,
    id: Uuid,
) -> Result<Option<RoundupSchedule>> {
    let sql = format!(
        "SELECT {SCHEDULE_COLUMNS} FROM roundup_schedules schedule \
         WHERE schedule.user_id = $1 AND schedule.id = $2"
    );
    Ok(sqlx::query_as(&sql)
        .bind(user_id)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<RoundupSchedule>> {
    let sql = format!(
        "SELECT {SCHEDULE_COLUMNS} FROM roundup_schedules schedule \
         WHERE schedule.user_id = $1 ORDER BY schedule.created_at"
    );
    Ok(sqlx::query_as(&sql).bind(user_id).fetch_all(pool).await?)
}

pub async fn count_for_user(pool: &PgPool, user_id: Uuid) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM roundup_schedules WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(pool)
            .await?,
    )
}

/// Replace scope/cadence and restart its timing window. Existing history stays
/// linked to the schedule; deleting the schedule later only detaches history.
pub async fn update(
    pool: &PgPool,
    user_id: Uuid,
    id: Uuid,
    new: NewRoundupSchedule<'_>,
) -> Result<Option<RoundupSchedule>> {
    if new.user_id != user_id || !scope_belongs_to_user(pool, user_id, new.scope).await? {
        return Err(DbError::NotFound);
    }
    let (feed_id, folder_id) = match new.scope {
        RoundupScope::Feed(id) => (Some(id), None),
        RoundupScope::Folder(id) => (None, Some(id)),
    };
    let updated = sqlx::query_scalar::<_, Uuid>(
        "UPDATE roundup_schedules SET scope_kind = $3, feed_id = $4, folder_id = $5, \
            period = $6, local_hour = $7, weekday = $8, timezone = $9, \
            email_delivery = $10, enabled = $11, last_run_at = NULL \
         WHERE id = $1 AND user_id = $2 RETURNING id",
    )
    .bind(id)
    .bind(user_id)
    .bind(new.scope.as_str())
    .bind(feed_id)
    .bind(folder_id)
    .bind(new.timing.period.as_str())
    .bind(i16::from(new.timing.local_hour))
    .bind(
        new.timing
            .weekday
            .map(|day| i16::from(day.number_from_monday())),
    )
    .bind(&new.timing.timezone)
    .bind(new.email_delivery)
    .bind(new.enabled)
    .fetch_optional(pool)
    .await?;
    match updated {
        Some(_) => find_for_user(pool, user_id, id).await,
        None => Ok(None),
    }
}

pub async fn set_enabled(pool: &PgPool, user_id: Uuid, id: Uuid, enabled: bool) -> Result<bool> {
    let result =
        sqlx::query("UPDATE roundup_schedules SET enabled = $3 WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .bind(enabled)
            .execute(pool)
            .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn delete(pool: &PgPool, user_id: Uuid, id: Uuid) -> Result<bool> {
    let result = sqlx::query("DELETE FROM roundup_schedules WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Bounded schedules whose exact user-local next run is due. SQL applies the
/// coarse age bound; Rust applies timezone/DST-aware cadence before returning.
pub async fn due_candidates(
    pool: &PgPool,
    now: OffsetDateTime,
    limit: i64,
) -> Result<Vec<RoundupSchedule>> {
    let sql = format!(
        "SELECT {SCHEDULE_COLUMNS} FROM roundup_schedules schedule \
         WHERE schedule.enabled AND (schedule.last_run_at IS NULL \
           OR (schedule.period = 'day' AND schedule.last_run_at <= $1 - interval '22 hours') \
           OR (schedule.period = 'week' AND schedule.last_run_at <= $1 - interval '6 days')) \
         ORDER BY schedule.last_run_at ASC NULLS FIRST, schedule.created_at LIMIT $2"
    );
    let schedules = sqlx::query_as::<_, RoundupSchedule>(&sql)
        .bind(now)
        .bind(limit.clamp(1, 2_000))
        .fetch_all(pool)
        .await?;
    Ok(schedules
        .into_iter()
        .filter(|schedule| schedule.next_run().is_some_and(|next| next <= now))
        .collect())
}
