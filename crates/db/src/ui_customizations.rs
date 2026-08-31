//! Versioned personal CSS and administrator-managed JavaScript assets.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::Result;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UiCustomization {
    pub id: Uuid,
    pub owner_id: Option<Uuid>,
    pub kind: String,
    pub version: i32,
    pub content: String,
    pub enabled: bool,
    pub created_by: Uuid,
    pub created_at: OffsetDateTime,
}

const COLUMNS: &str = "id, owner_id, kind, version, content, enabled, created_by, created_at";

pub async fn current_css(pool: &PgPool, user_id: Uuid) -> Result<Option<UiCustomization>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM ui_customizations WHERE owner_id = $1 AND kind = 'css' AND enabled ORDER BY version DESC LIMIT 1"
    );
    Ok(sqlx::query_as(&sql)
        .bind(user_id)
        .fetch_optional(pool)
        .await?)
}

pub async fn current_javascript(pool: &PgPool) -> Result<Option<UiCustomization>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM ui_customizations WHERE owner_id IS NULL AND kind = 'javascript' AND enabled ORDER BY version DESC LIMIT 1"
    );
    Ok(sqlx::query_as(&sql).fetch_optional(pool).await?)
}

pub async fn history(
    pool: &PgPool,
    owner_id: Option<Uuid>,
    kind: &str,
    limit: i64,
) -> Result<Vec<UiCustomization>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM ui_customizations WHERE owner_id IS NOT DISTINCT FROM $1 AND kind = $2 ORDER BY version DESC LIMIT $3"
    );
    Ok(sqlx::query_as(&sql)
        .bind(owner_id)
        .bind(kind)
        .bind(limit.clamp(1, 50))
        .fetch_all(pool)
        .await?)
}

pub async fn create_version(
    pool: &PgPool,
    owner_id: Option<Uuid>,
    kind: &str,
    content: &str,
    created_by: Uuid,
) -> Result<UiCustomization> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended(coalesce($1::text, 'instance') || ':' || $2, 0))")
        .bind(owner_id)
        .bind(kind)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "UPDATE ui_customizations SET enabled = false WHERE owner_id IS NOT DISTINCT FROM $1 AND kind = $2 AND enabled",
    )
    .bind(owner_id)
    .bind(kind)
    .execute(&mut *transaction)
    .await?;
    let version: i32 = sqlx::query_scalar(
        "SELECT coalesce(max(version), 0) + 1 FROM ui_customizations WHERE owner_id IS NOT DISTINCT FROM $1 AND kind = $2",
    )
    .bind(owner_id)
    .bind(kind)
    .fetch_one(&mut *transaction)
    .await?;
    let sql = format!(
        "INSERT INTO ui_customizations (id, owner_id, kind, version, content, created_by) VALUES ($1, $2, $3, $4, $5, $6) RETURNING {COLUMNS}"
    );
    let row = sqlx::query_as(&sql)
        .bind(Uuid::now_v7())
        .bind(owner_id)
        .bind(kind)
        .bind(version)
        .bind(content)
        .bind(created_by)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(row)
}

pub async fn restore(
    pool: &PgPool,
    id: Uuid,
    owner_id: Option<Uuid>,
    kind: &str,
    actor_id: Uuid,
) -> Result<Option<UiCustomization>> {
    let content: Option<String> = sqlx::query_scalar(
        "SELECT content FROM ui_customizations WHERE id = $1 AND owner_id IS NOT DISTINCT FROM $2 AND kind = $3",
    )
    .bind(id)
    .bind(owner_id)
    .bind(kind)
    .fetch_optional(pool)
    .await?;
    match content {
        Some(content) => create_version(pool, owner_id, kind, &content, actor_id)
            .await
            .map(Some),
        None => Ok(None),
    }
}

pub async fn disable(pool: &PgPool, owner_id: Option<Uuid>, kind: &str) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE ui_customizations SET enabled = false WHERE owner_id IS NOT DISTINCT FROM $1 AND kind = $2 AND enabled",
    )
    .bind(owner_id)
    .bind(kind)
    .execute(pool)
    .await?
    .rows_affected())
}
