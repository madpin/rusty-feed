//! Feed blacklist repository (docs/PUBLIC_LAUNCH_PLAN.md §5, FND-192).

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use rusty_feed_core::blocklist::{BlockEntry, BlockKind, Blocklist};

use crate::error::Result;

const COLUMNS: &str = "id, kind, pattern, reason, source, created_by, created_at";

/// A blocklist row, for the admin listing.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BlocklistRow {
    pub id: Uuid,
    pub kind: String,
    pub pattern: String,
    pub reason: Option<String>,
    pub source: String,
    pub created_by: Option<Uuid>,
    pub created_at: OffsetDateTime,
}

/// Add a blocklist entry (idempotent on (kind, pattern)).
pub async fn add(
    pool: &PgPool,
    kind: &str,
    pattern: &str,
    reason: Option<&str>,
    source: &str,
    created_by: Option<Uuid>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO feed_blocklist (id, kind, pattern, reason, source, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (kind, pattern) DO UPDATE SET reason = EXCLUDED.reason",
    )
    .bind(Uuid::now_v7())
    .bind(kind)
    .bind(pattern)
    .bind(reason)
    .bind(source)
    .bind(created_by)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a blocklist entry by id; returns whether a row was deleted.
pub async fn remove(pool: &PgPool, id: Uuid) -> Result<bool> {
    Ok(sqlx::query("DELETE FROM feed_blocklist WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Fetch a single entry by id (to audit a removal).
pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<BlocklistRow>> {
    let sql = format!("SELECT {COLUMNS} FROM feed_blocklist WHERE id = $1");
    Ok(sqlx::query_as(&sql).bind(id).fetch_optional(pool).await?)
}

/// All entries, newest first, for the admin listing.
pub async fn list(pool: &PgPool) -> Result<Vec<BlocklistRow>> {
    let sql = format!("SELECT {COLUMNS} FROM feed_blocklist ORDER BY created_at DESC");
    Ok(sqlx::query_as(&sql).fetch_all(pool).await?)
}

/// Load and compile the immutable [`Blocklist`] for matching.
pub async fn compiled(pool: &PgPool) -> Result<Blocklist> {
    let rows: Vec<BlocklistRow> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM feed_blocklist"))
        .fetch_all(pool)
        .await?;
    Ok(Blocklist::compile(rows.into_iter().filter_map(|row| {
        BlockKind::parse(&row.kind).map(|kind| BlockEntry {
            kind,
            pattern: row.pattern,
            reason: row.reason,
        })
    })))
}

/// All shared feed ids and URLs (used to disable feeds that match a new entry).
pub async fn all_feed_urls(pool: &PgPool) -> Result<Vec<(Uuid, String)>> {
    Ok(sqlx::query_as("SELECT id, url FROM feeds")
        .fetch_all(pool)
        .await?)
}
