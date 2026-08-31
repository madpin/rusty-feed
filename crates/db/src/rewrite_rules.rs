//! Version history for administrator-owned shared content rewrite rules.

use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::Result;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RewriteRuleVersion {
    pub id: Uuid,
    pub feed_id: Uuid,
    pub version: i32,
    pub rules: Value,
    pub created_by: Uuid,
    pub created_at: OffsetDateTime,
}

const COLUMNS: &str = "id, feed_id, version, rules, created_by, created_at";

pub async fn create(
    pool: &PgPool,
    feed_id: Uuid,
    actor_id: Uuid,
    rules: &Value,
) -> Result<RewriteRuleVersion> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text || ':rewrite-rules', 0))")
        .bind(feed_id)
        .execute(&mut *transaction)
        .await?;
    let version: i32 = sqlx::query_scalar(
        "SELECT coalesce(max(version),0)+1 FROM feed_rewrite_rule_versions WHERE feed_id = $1",
    )
    .bind(feed_id)
    .fetch_one(&mut *transaction)
    .await?;
    let sql = format!("INSERT INTO feed_rewrite_rule_versions (id,feed_id,version,rules,created_by) VALUES ($1,$2,$3,$4,$5) RETURNING {COLUMNS}");
    let row = sqlx::query_as(&sql)
        .bind(Uuid::now_v7())
        .bind(feed_id)
        .bind(version)
        .bind(rules)
        .bind(actor_id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(row)
}

pub async fn list(pool: &PgPool, feed_id: Uuid, limit: i64) -> Result<Vec<RewriteRuleVersion>> {
    let sql = format!("SELECT {COLUMNS} FROM feed_rewrite_rule_versions WHERE feed_id = $1 ORDER BY version DESC LIMIT $2");
    Ok(sqlx::query_as(&sql)
        .bind(feed_id)
        .bind(limit.clamp(1, 50))
        .fetch_all(pool)
        .await?)
}

pub async fn find(pool: &PgPool, id: Uuid, feed_id: Uuid) -> Result<Option<RewriteRuleVersion>> {
    let sql =
        format!("SELECT {COLUMNS} FROM feed_rewrite_rule_versions WHERE id = $1 AND feed_id = $2");
    Ok(sqlx::query_as(&sql)
        .bind(id)
        .bind(feed_id)
        .fetch_optional(pool)
        .await?)
}
