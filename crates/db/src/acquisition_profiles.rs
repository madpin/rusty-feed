//! Administrator-owned, source-pinned outbound acquisition policies.

use serde_json::json;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::Result;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AcquisitionProfile {
    pub id: Uuid,
    pub name: String,
    pub allowed_hosts: Vec<String>,
    pub allowed_path_prefix: Option<String>,
    pub secrets_ciphertext: Option<String>,
    pub uses_cookie: bool,
    pub uses_proxy: bool,
    pub allow_invalid_tls: bool,
    pub enabled: bool,
    pub expires_at: Option<OffsetDateTime>,
    pub created_by: Uuid,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AcquisitionAuditEntry {
    pub action: String,
    pub profile_name: Option<String>,
    pub feed_name: Option<String>,
    pub detail: serde_json::Value,
    pub created_at: OffsetDateTime,
}

const COLUMNS: &str = "id, name, allowed_hosts, allowed_path_prefix, secrets_ciphertext, uses_cookie, uses_proxy, allow_invalid_tls, enabled, expires_at, created_by, created_at, updated_at";

#[allow(clippy::too_many_arguments)]
pub async fn create(
    pool: &PgPool,
    actor_id: Uuid,
    name: &str,
    allowed_hosts: &[String],
    allowed_path_prefix: Option<&str>,
    secrets_ciphertext: Option<&str>,
    uses_cookie: bool,
    uses_proxy: bool,
    allow_invalid_tls: bool,
    expires_at: Option<OffsetDateTime>,
) -> Result<AcquisitionProfile> {
    let id = Uuid::now_v7();
    let mut transaction = pool.begin().await?;
    let sql = format!("INSERT INTO acquisition_profiles (id,name,allowed_hosts,allowed_path_prefix,secrets_ciphertext,uses_cookie,uses_proxy,allow_invalid_tls,expires_at,created_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING {COLUMNS}");
    let profile = sqlx::query_as(&sql)
        .bind(id)
        .bind(name)
        .bind(allowed_hosts)
        .bind(allowed_path_prefix)
        .bind(secrets_ciphertext)
        .bind(uses_cookie)
        .bind(uses_proxy)
        .bind(allow_invalid_tls)
        .bind(expires_at)
        .bind(actor_id)
        .fetch_one(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO acquisition_profile_audit (id,actor_id,profile_id,action,detail) VALUES ($1,$2,$3,'created',$4)")
        .bind(Uuid::now_v7()).bind(actor_id).bind(id)
        .bind(json!({"host_count": allowed_hosts.len(), "cookie": uses_cookie, "proxy": uses_proxy, "invalid_tls": allow_invalid_tls, "expires": expires_at.is_some()}))
        .execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(profile)
}

pub async fn list(pool: &PgPool) -> Result<Vec<AcquisitionProfile>> {
    let sql = format!("SELECT {COLUMNS} FROM acquisition_profiles ORDER BY name");
    Ok(sqlx::query_as(&sql).fetch_all(pool).await?)
}

pub async fn find_enabled(pool: &PgPool, id: Uuid) -> Result<Option<AcquisitionProfile>> {
    let sql = format!("SELECT {COLUMNS} FROM acquisition_profiles WHERE id = $1 AND enabled AND (expires_at IS NULL OR expires_at > now())");
    Ok(sqlx::query_as(&sql).bind(id).fetch_optional(pool).await?)
}

pub async fn for_feed(pool: &PgPool, feed_id: Uuid) -> Result<Option<AcquisitionProfile>> {
    let sql = format!("SELECT {} FROM acquisition_profiles p JOIN feed_acquisition_assignments a ON a.profile_id = p.id WHERE a.feed_id = $1 AND p.enabled AND (p.expires_at IS NULL OR p.expires_at > now())", COLUMNS.split(", ").map(|column| format!("p.{column}")).collect::<Vec<_>>().join(", "));
    Ok(sqlx::query_as(&sql)
        .bind(feed_id)
        .fetch_optional(pool)
        .await?)
}

pub async fn assign(
    pool: &PgPool,
    actor_id: Uuid,
    feed_id: Uuid,
    profile_id: Option<Uuid>,
) -> Result<()> {
    let mut transaction = pool.begin().await?;
    sqlx::query("DELETE FROM feed_acquisition_assignments WHERE feed_id = $1")
        .bind(feed_id)
        .execute(&mut *transaction)
        .await?;
    if let Some(profile_id) = profile_id {
        sqlx::query("INSERT INTO feed_acquisition_assignments (feed_id,profile_id,assigned_by) SELECT $1,$2,$3 WHERE EXISTS (SELECT 1 FROM acquisition_profiles WHERE id = $2 AND enabled)")
            .bind(feed_id).bind(profile_id).bind(actor_id).execute(&mut *transaction).await?;
    }
    sqlx::query("INSERT INTO acquisition_profile_audit (id,actor_id,profile_id,feed_id,action) VALUES ($1,$2,$3,$4,'assigned')")
        .bind(Uuid::now_v7()).bind(actor_id).bind(profile_id).bind(feed_id)
        .execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn delete(pool: &PgPool, actor_id: Uuid, id: Uuid) -> Result<bool> {
    let mut transaction = pool.begin().await?;
    let removed = sqlx::query("DELETE FROM acquisition_profiles WHERE id = $1")
        .bind(id)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
        > 0;
    if removed {
        sqlx::query("INSERT INTO acquisition_profile_audit (id,actor_id,profile_id,action) VALUES ($1,$2,$3,'deleted')")
            .bind(Uuid::now_v7()).bind(actor_id).bind(id).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    Ok(removed)
}

pub async fn list_audit(pool: &PgPool, limit: i64) -> Result<Vec<AcquisitionAuditEntry>> {
    Ok(sqlx::query_as(
        "SELECT audit.action, profile.name AS profile_name, feed.name AS feed_name, \
                audit.detail, audit.created_at \
         FROM acquisition_profile_audit audit \
         LEFT JOIN acquisition_profiles profile ON profile.id = audit.profile_id \
         LEFT JOIN feeds feed ON feed.id = audit.feed_id \
         ORDER BY audit.created_at DESC LIMIT $1",
    )
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await?)
}
