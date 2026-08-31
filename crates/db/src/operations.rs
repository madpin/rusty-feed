//! Safe instance-level operational statistics for administrator surfaces.

use sqlx::PgPool;

use crate::error::Result;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StorageStats {
    pub database_bytes: i64,
    pub articles_bytes: i64,
    pub embeddings_bytes: i64,
    pub media_cache_bytes: i64,
}

pub async fn storage_stats(pool: &PgPool) -> Result<StorageStats> {
    Ok(sqlx::query_as(
        "SELECT pg_database_size(current_database())::bigint AS database_bytes, \
                pg_total_relation_size('articles')::bigint AS articles_bytes, \
                pg_total_relation_size('article_embeddings')::bigint AS embeddings_bytes, \
                pg_total_relation_size('media_cache')::bigint AS media_cache_bytes",
    )
    .fetch_one(pool)
    .await?)
}
