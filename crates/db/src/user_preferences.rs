//! User preferences repository. Preferences are stored as a single JSONB
//! document (deserialized into a typed struct in the web layer) — see
//! `docs/FINDINGS.md` FND-111.

use serde_json::Value as Json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;

/// Fetch a user's raw preferences document, if any.
pub async fn get(pool: &PgPool, user_id: Uuid) -> Result<Option<Json>> {
    Ok(
        sqlx::query_scalar::<_, Json>(
            "SELECT preferences FROM user_preferences WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await?,
    )
}

/// Insert or replace a user's preferences document.
pub async fn upsert(pool: &PgPool, user_id: Uuid, preferences: &Json) -> Result<()> {
    sqlx::query(
        "INSERT INTO user_preferences (user_id, preferences) VALUES ($1, $2) \
         ON CONFLICT (user_id) DO UPDATE SET preferences = EXCLUDED.preferences, updated_at = now()",
    )
    .bind(user_id)
    .bind(preferences)
    .execute(pool)
    .await?;
    Ok(())
}
