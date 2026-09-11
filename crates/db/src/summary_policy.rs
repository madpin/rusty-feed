use sqlx::PgPool;

use crate::error::{DbError, Result};

/// Default readable-text threshold for automatic summaries.
pub const DEFAULT_MIN_CONTENT_CHARS: u32 = 2_000;
/// Largest administrator-configurable readable-text threshold.
pub const MAX_MIN_CONTENT_CHARS: u32 = 1_000_000;

/// Live instance-wide policy for automatic shared summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummaryPolicy {
    /// Whether feed ingestion may enqueue automatic summaries.
    pub enabled: bool,
    /// Minimum normalized plain-text characters in the effective body.
    pub min_content_chars: u32,
}

fn validate_min_content_chars(value: u32) -> Result<i32> {
    if value > MAX_MIN_CONTENT_CHARS {
        return Err(DbError::Validation(format!(
            "summary minimum must be between 0 and {MAX_MIN_CONTENT_CHARS} characters"
        )));
    }
    Ok(value as i32)
}

/// Read the persisted policy, initializing it from deployment defaults once.
pub async fn get_or_init(
    pool: &PgPool,
    default_enabled: bool,
    default_min_content_chars: u32,
) -> Result<SummaryPolicy> {
    let min_content_chars = validate_min_content_chars(default_min_content_chars)?;
    let mut stored: Option<(bool, i32)> = sqlx::query_as(
        "SELECT enabled, min_content_chars FROM instance_summary_policy WHERE singleton = true",
    )
    .fetch_optional(pool)
    .await?;
    if stored.is_none() {
        sqlx::query(
            "INSERT INTO instance_summary_policy (singleton, enabled, min_content_chars) \
             VALUES (true, $1, $2) ON CONFLICT (singleton) DO NOTHING",
        )
        .bind(default_enabled)
        .bind(min_content_chars)
        .execute(pool)
        .await?;
        stored = sqlx::query_as(
            "SELECT enabled, min_content_chars FROM instance_summary_policy WHERE singleton = true",
        )
        .fetch_optional(pool)
        .await?;
    }
    let (enabled, min_content_chars) = stored.ok_or_else(|| {
        DbError::Validation("automatic summary policy could not be initialized".to_string())
    })?;
    Ok(SummaryPolicy {
        enabled,
        min_content_chars: min_content_chars as u32,
    })
}

/// Replace the live policy selected by an administrator.
pub async fn update(pool: &PgPool, enabled: bool, min_content_chars: u32) -> Result<SummaryPolicy> {
    let min_content_chars = validate_min_content_chars(min_content_chars)?;
    let (enabled, min_content_chars): (bool, i32) = sqlx::query_as(
        "INSERT INTO instance_summary_policy (singleton, enabled, min_content_chars) \
         VALUES (true, $1, $2) ON CONFLICT (singleton) DO UPDATE SET \
         enabled = EXCLUDED.enabled, min_content_chars = EXCLUDED.min_content_chars \
         RETURNING enabled, min_content_chars",
    )
    .bind(enabled)
    .bind(min_content_chars)
    .fetch_one(pool)
    .await?;
    Ok(SummaryPolicy {
        enabled,
        min_content_chars: min_content_chars as u32,
    })
}
