//! User abuse reports → admin review queue (docs/PUBLIC_LAUNCH_PLAN.md §4.6).

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

/// A report joined with the reporter email, for the admin queue.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReportRow {
    pub id: Uuid,
    pub reporter_email: Option<String>,
    pub target_kind: String,
    pub target_id: Uuid,
    pub target_url: Option<String>,
    pub reason: String,
    pub status: String,
    pub created_at: OffsetDateTime,
}

/// File a report. The partial unique index makes a duplicate open report by the
/// same user for the same target a no-op (returns `false`).
pub async fn create(
    pool: &PgPool,
    reporter_id: Uuid,
    target_kind: &str,
    target_id: Uuid,
    target_url: Option<&str>,
    reason: &str,
) -> Result<bool> {
    let affected = sqlx::query(
        "INSERT INTO abuse_reports (id, reporter_id, target_kind, target_id, target_url, reason) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (reporter_id, target_kind, target_id) WHERE status = 'open' DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(reporter_id)
    .bind(target_kind)
    .bind(target_id)
    .bind(target_url)
    .bind(reason)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(affected > 0)
}

/// Open + reviewing reports for the admin queue (newest first).
pub async fn list_active(pool: &PgPool, limit: i64) -> Result<Vec<ReportRow>> {
    let limit = limit.clamp(1, 500);
    Ok(sqlx::query_as(
        "SELECT r.id, u.email AS reporter_email, r.target_kind, r.target_id, r.target_url, \
                r.reason, r.status, r.created_at \
         FROM abuse_reports r LEFT JOIN users u ON u.id = r.reporter_id \
         WHERE r.status IN ('open', 'reviewing') \
         ORDER BY r.created_at DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Count of open reports (for the admin nav badge).
pub async fn count_open(pool: &PgPool) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM abuse_reports WHERE status = 'open'")
            .fetch_one(pool)
            .await?,
    )
}

/// Resolve/dismiss a report.
pub async fn set_status(pool: &PgPool, id: Uuid, resolver_id: Uuid, status: &str) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE abuse_reports SET status = $3, resolved_by = $2, \
                resolved_at = CASE WHEN $3 IN ('resolved', 'dismissed') THEN now() ELSE NULL END \
         WHERE id = $1",
    )
    .bind(id)
    .bind(resolver_id)
    .bind(status)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}
