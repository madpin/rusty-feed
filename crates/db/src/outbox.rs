//! Transactional outbox primitives and dispatcher. Producers may append an
//! event inside their content transaction; workers atomically translate it to
//! one deduplicated durable queue job.

use serde_json::Value as Json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::Result;

#[derive(sqlx::FromRow)]
struct OutboxEvent {
    id: Uuid,
    kind: String,
    payload: Json,
}

/// Append an event within the caller's transaction.
pub async fn publish(
    transaction: &mut Transaction<'_, Postgres>,
    kind: &str,
    payload: Json,
) -> Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO outbox_events (id, kind, payload) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(kind)
        .bind(payload)
        .execute(&mut **transaction)
        .await?;
    Ok(id)
}

/// Dispatch a bounded batch. Unknown kinds stay pending so deployment skew
/// cannot silently discard work. The event ID is the queue deduplication key,
/// making a retried transaction safe.
pub async fn dispatch(pool: &PgPool, allowed_kinds: &[String], limit: i64) -> Result<u64> {
    if allowed_kinds.is_empty() {
        return Ok(0);
    }
    let mut transaction = pool.begin().await?;
    let events: Vec<OutboxEvent> = sqlx::query_as(
        "SELECT id, kind, payload FROM outbox_events \
         WHERE processed_at IS NULL AND kind = ANY($1) \
         ORDER BY created_at, id FOR UPDATE SKIP LOCKED LIMIT $2",
    )
    .bind(allowed_kinds)
    .bind(limit.clamp(1, 1_000))
    .fetch_all(&mut *transaction)
    .await?;
    for event in &events {
        let dedup_key = event
            .payload
            .get("_dedup_key")
            .and_then(Json::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("outbox:{}", event.id));
        sqlx::query(
            "INSERT INTO job_queue (id, queue_name, kind, payload, dedup_key, max_attempts) \
             VALUES ($1, 'background', $2, $3, $4, 5) ON CONFLICT DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(&event.kind)
        .bind(&event.payload)
        .bind(&dedup_key)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE outbox_events SET processed_at = now() WHERE id = $1")
            .bind(event.id)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(events.len() as u64)
}
