//! Durable LLM/embedding cost accounting (M12).

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;

/// Record a single billable call.
#[allow(clippy::too_many_arguments)]
pub async fn record(
    pool: &PgPool,
    user_id: Option<Uuid>,
    provider: &str,
    model: &str,
    kind: &str,
    input_tokens: i64,
    output_tokens: i64,
    cost_micro_usd: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO cost_ledger \
           (id, user_id, provider, model, kind, input_tokens, output_tokens, cost_micro_usd) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(provider)
    .bind(model)
    .bind(kind)
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(cost_micro_usd)
    .execute(pool)
    .await?;
    Ok(())
}

/// Aggregated cost per provider/model/kind (for the admin view).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CostRow {
    pub provider: String,
    pub model: String,
    pub kind: String,
    pub calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_micro_usd: i64,
}

/// Cost aggregates grouped by provider/model/kind, most expensive first.
pub async fn by_model(pool: &PgPool) -> Result<Vec<CostRow>> {
    Ok(sqlx::query_as::<_, CostRow>(
        "SELECT provider, model, kind, count(*) AS calls, \
                sum(input_tokens)::bigint AS input_tokens, \
                sum(output_tokens)::bigint AS output_tokens, \
                sum(cost_micro_usd)::bigint AS cost_micro_usd \
         FROM cost_ledger GROUP BY provider, model, kind ORDER BY cost_micro_usd DESC",
    )
    .fetch_all(pool)
    .await?)
}

/// Total cost across all calls, in micro-USD.
pub async fn total_micro_usd(pool: &PgPool) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COALESCE(sum(cost_micro_usd), 0)::bigint FROM cost_ledger")
            .fetch_one(pool)
            .await?,
    )
}
