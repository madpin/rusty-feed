//! Saved-search CRUD (M11).

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;
use crate::models::{SavedSearch, SAVED_SEARCH_COLUMNS};

/// How many saved searches this user has (for quota enforcement, FND-188).
pub async fn count_for_user(pool: &PgPool, user_id: Uuid) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM saved_searches WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(pool)
            .await?,
    )
}

/// Create a saved search.
pub async fn create(
    pool: &PgPool,
    user_id: Uuid,
    name: &str,
    query: &str,
    threshold: f64,
    notify_on_match: bool,
    notify_threshold: f64,
) -> Result<SavedSearch> {
    let sql = format!(
        "INSERT INTO saved_searches (id, user_id, name, query, threshold, notify_on_match, notify_threshold) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {SAVED_SEARCH_COLUMNS}"
    );
    Ok(sqlx::query_as::<_, SavedSearch>(&sql)
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(name)
        .bind(query)
        .bind(threshold)
        .bind(notify_on_match)
        .bind(notify_threshold)
        .fetch_one(pool)
        .await?)
}

/// A user's non-archived saved searches.
pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<SavedSearch>> {
    let sql = format!(
        "SELECT {SAVED_SEARCH_COLUMNS} FROM saved_searches \
         WHERE user_id = $1 AND NOT archived ORDER BY created_at DESC"
    );
    Ok(sqlx::query_as::<_, SavedSearch>(&sql)
        .bind(user_id)
        .fetch_all(pool)
        .await?)
}

/// All active (non-archived) saved searches across users (for the scan agent).
pub async fn list_active(pool: &PgPool) -> Result<Vec<SavedSearch>> {
    let sql = format!("SELECT {SAVED_SEARCH_COLUMNS} FROM saved_searches WHERE NOT archived");
    Ok(sqlx::query_as::<_, SavedSearch>(&sql)
        .fetch_all(pool)
        .await?)
}

/// Fetch one by id.
pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<SavedSearch>> {
    let sql = format!("SELECT {SAVED_SEARCH_COLUMNS} FROM saved_searches WHERE id = $1");
    Ok(sqlx::query_as::<_, SavedSearch>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// Fetch one owned by `user_id` (authorization).
pub async fn get_for_user(pool: &PgPool, id: Uuid, user_id: Uuid) -> Result<Option<SavedSearch>> {
    let sql =
        format!("SELECT {SAVED_SEARCH_COLUMNS} FROM saved_searches WHERE id = $1 AND user_id = $2");
    Ok(sqlx::query_as::<_, SavedSearch>(&sql)
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?)
}

/// Delete a saved search owned by `user_id`.
pub async fn delete(pool: &PgPool, id: Uuid, user_id: Uuid) -> Result<bool> {
    let res = sqlx::query("DELETE FROM saved_searches WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Update a saved search owned by `user_id`.
pub async fn update(
    pool: &PgPool,
    id: Uuid,
    user_id: Uuid,
    name: &str,
    query: &str,
    notify_on_match: bool,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE saved_searches SET name = $3, query = $4, notify_on_match = $5, \
         updated_at = now() WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .bind(name)
    .bind(query)
    .bind(notify_on_match)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Record a matching pass: bump `total_matches` and `last_matched_at`.
pub async fn record_matches(pool: &PgPool, id: Uuid, new_matches: i64) -> Result<()> {
    sqlx::query(
        "UPDATE saved_searches SET total_matches = total_matches + $2, last_matched_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .bind(new_matches)
    .execute(pool)
    .await?;
    Ok(())
}
