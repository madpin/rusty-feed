//! Instance-wide embedding-space guard (FND-002/FND-106).

use sqlx::PgPool;

use crate::error::{DbError, Result};

/// Claim the space on first use, or reject a configuration that differs from
/// the durable active model/dimension.
pub async fn ensure(pool: &PgPool, model: &str, dimensions: i32) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('rusty-feed:embedding-space'))")
        .execute(&mut *tx)
        .await?;
    let current: Option<(String, i32)> =
        sqlx::query_as("SELECT model, dimensions FROM instance_embedding_space WHERE singleton")
            .fetch_optional(&mut *tx)
            .await?;
    match current {
        Some((active_model, active_dimensions))
            if active_model != model || active_dimensions != dimensions =>
        {
            return Err(DbError::Validation(format!(
                "embedding space is {active_model}/{active_dimensions}, but configuration requests {model}/{dimensions}; run `rusty-feed-cli reindex --confirm`"
            )));
        }
        Some(_) => {}
        None => {
            let existing: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT model FROM article_embeddings ORDER BY model LIMIT 2",
            )
            .fetch_all(&mut *tx)
            .await?;
            if existing.iter().any(|existing| existing != model) {
                return Err(DbError::Validation(format!(
                    "stored vectors use model(s) {}, but configuration requests {model}; run `rusty-feed-cli reindex --confirm`",
                    existing.join(", ")
                )));
            }
            sqlx::query(
                "INSERT INTO instance_embedding_space (singleton, model, dimensions) VALUES (true, $1, $2)",
            )
            .bind(model)
            .bind(dimensions)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Deliberately replace the active space and remove incompatible vectors. The
/// operation is transactional and only exposed through the confirmed CLI.
pub async fn replace(pool: &PgPool, model: &str, dimensions: i32) -> Result<u64> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('rusty-feed:embedding-space'))")
        .execute(&mut *tx)
        .await?;
    let removed = sqlx::query("DELETE FROM article_embeddings")
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query(
        "INSERT INTO instance_embedding_space (singleton, model, dimensions, updated_at) \
         VALUES (true, $1, $2, now()) ON CONFLICT (singleton) DO UPDATE SET \
         model = EXCLUDED.model, dimensions = EXCLUDED.dimensions, updated_at = now()",
    )
    .bind(model)
    .bind(dimensions)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(removed)
}
