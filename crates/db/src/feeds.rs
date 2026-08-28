//! Feed repository — shared feeds, deduplicated by normalized URL.
//!
//! [`upsert_by_url`] is the write path that guarantees a feed URL maps to
//! exactly one row no matter how many users subscribe (FR-DEDUP-1).

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DbError, Result};
use crate::models::{Feed, FEED_COLUMNS};

/// Find a feed by its (already normalized) URL.
pub async fn find_by_url(pool: &PgPool, url: &str) -> Result<Option<Feed>> {
    let sql = format!("SELECT {FEED_COLUMNS} FROM feeds WHERE url = $1");
    Ok(sqlx::query_as::<_, Feed>(&sql)
        .bind(url)
        .fetch_optional(pool)
        .await?)
}

/// Find a feed by id.
pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Feed>> {
    let sql = format!("SELECT {FEED_COLUMNS} FROM feeds WHERE id = $1");
    Ok(sqlx::query_as::<_, Feed>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// Get-or-create a shared feed by URL. Returns the feed and whether it was newly
/// created. Concurrent callers are handled via `ON CONFLICT DO NOTHING` + reread.
pub async fn upsert_by_url(
    pool: &PgPool,
    url: &str,
    name: &str,
    site_url: Option<&str>,
) -> Result<(Feed, bool)> {
    if let Some(existing) = find_by_url(pool, url).await? {
        return Ok((existing, false));
    }

    let sql = format!(
        "INSERT INTO feeds (id, url, name, site_url) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (url) DO NOTHING RETURNING {FEED_COLUMNS}"
    );
    let inserted = sqlx::query_as::<_, Feed>(&sql)
        .bind(Uuid::now_v7())
        .bind(url)
        .bind(name)
        .bind(site_url)
        .fetch_optional(pool)
        .await?;

    match inserted {
        Some(feed) => Ok((feed, true)),
        // Lost the race with a concurrent insert: the row now exists.
        None => find_by_url(pool, url)
            .await?
            .map(|f| (f, false))
            .ok_or(DbError::NotFound),
    }
}

/// Count feeds with a given URL (0 or 1). Handy for dedup assertions/tests.
pub async fn count_by_url(pool: &PgPool, url: &str) -> Result<i64> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM feeds WHERE url = $1")
            .bind(url)
            .fetch_one(pool)
            .await?,
    )
}
