//! Learned per-user keyword patterns (M10): incremental weight updates plus the
//! decay/prune/top-N maintenance the PatternDecayAgent runs.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;

/// Add `delta` to a keyword's weight (creating it if new) and bump its count.
pub async fn upsert_delta(pool: &PgPool, user_id: Uuid, keyword: &str, delta: f64) -> Result<()> {
    sqlx::query(
        "INSERT INTO user_patterns (id, user_id, keyword, weight, feedback_count) \
         VALUES ($1, $2, $3, $4, 1) \
         ON CONFLICT (user_id, keyword) DO UPDATE SET \
           weight = user_patterns.weight + $4, \
           feedback_count = user_patterns.feedback_count + 1, updated_at = now()",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(keyword)
    .bind(delta)
    .execute(pool)
    .await?;
    Ok(())
}

/// Weights for the given keywords (only those the user has patterns for).
pub async fn weights_for(
    pool: &PgPool,
    user_id: Uuid,
    keywords: &[String],
) -> Result<Vec<(String, f64)>> {
    Ok(sqlx::query_as::<_, (String, f64)>(
        "SELECT keyword, weight FROM user_patterns WHERE user_id = $1 AND keyword = ANY($2)",
    )
    .bind(user_id)
    .bind(keywords)
    .fetch_all(pool)
    .await?)
}

/// How many patterns a user has.
pub async fn count_for(pool: &PgPool, user_id: Uuid) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM user_patterns WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(pool)
            .await?,
    )
}

/// A user's strongest patterns (by |weight|).
pub async fn top(pool: &PgPool, user_id: Uuid, limit: i64) -> Result<Vec<(String, f64)>> {
    Ok(sqlx::query_as::<_, (String, f64)>(
        "SELECT keyword, weight FROM user_patterns WHERE user_id = $1 \
         ORDER BY abs(weight) DESC LIMIT $2",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Delete all of a user's patterns ("reset learning").
pub async fn delete_for_user(pool: &PgPool, user_id: Uuid) -> Result<u64> {
    let res = sqlx::query("DELETE FROM user_patterns WHERE user_id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

// ── Maintenance (PatternDecayAgent) ──────────────────────────────────────────

/// Decay patterns older than 30 days by `0.9^floor(days/30)` (all users).
pub async fn decay_all(pool: &PgPool) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE user_patterns \
         SET weight = weight * power(0.9, floor(extract(epoch FROM (now() - updated_at)) / 2592000.0)), \
             updated_at = now() \
         WHERE extract(epoch FROM (now() - updated_at)) >= 2592000",
    )
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Prune patterns whose |weight| is below `min_abs` (all users).
pub async fn prune_small(pool: &PgPool, min_abs: f64) -> Result<u64> {
    let res = sqlx::query("DELETE FROM user_patterns WHERE abs(weight) < $1")
        .bind(min_abs)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Keep only each user's top-`n` patterns by |weight| (all users).
pub async fn enforce_top_n(pool: &PgPool, n: i64) -> Result<u64> {
    let res = sqlx::query(
        "DELETE FROM user_patterns WHERE id IN ( \
           SELECT id FROM ( \
             SELECT id, row_number() OVER (PARTITION BY user_id ORDER BY abs(weight) DESC) AS rn \
             FROM user_patterns \
           ) t WHERE t.rn > $1 )",
    )
    .bind(n)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}
