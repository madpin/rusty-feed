//! Durable LLM/embedding cost accounting (M12) and daily budget usage
//! aggregates for public-instance enforcement (FND-187).

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

/// Record a single billable call.
///
/// `billed_to_user` marks spend funded by the user's own enabled key (BYO-key):
/// such rows are excluded from the instance and per-user USD budgets, which only
/// govern instance-funded spend (`docs/PUBLIC_LAUNCH_PLAN.md` §4.1, §8.1).
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
    billed_to_user: bool,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO cost_ledger \
           (id, user_id, provider, model, kind, input_tokens, output_tokens, cost_micro_usd, billed_to_user) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(provider)
    .bind(model)
    .bind(kind)
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(cost_micro_usd)
    .bind(billed_to_user)
    .execute(pool)
    .await?;
    Ok(())
}

/// Total instance-funded spend (micro-USD) since `since`. Excludes BYO-key rows.
pub async fn instance_spend_micro_usd_since(pool: &PgPool, since: OffsetDateTime) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(sum(cost_micro_usd), 0)::bigint FROM cost_ledger \
         WHERE NOT billed_to_user AND occurred_at >= $1",
    )
    .bind(since)
    .fetch_one(pool)
    .await?)
}

/// Instance-funded spend (micro-USD) attributed to one user since `since`.
pub async fn user_spend_micro_usd_since(
    pool: &PgPool,
    user_id: Uuid,
    since: OffsetDateTime,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(sum(cost_micro_usd), 0)::bigint FROM cost_ledger \
         WHERE user_id = $1 AND NOT billed_to_user AND occurred_at >= $2",
    )
    .bind(user_id)
    .bind(since)
    .fetch_one(pool)
    .await?)
}

/// Count of a user's calls of a given `kind` since `since` (funding-agnostic:
/// bounds job/DB abuse regardless of who pays).
pub async fn user_call_count_since(
    pool: &PgPool,
    user_id: Uuid,
    kind: &str,
    since: OffsetDateTime,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*)::bigint FROM cost_ledger \
         WHERE user_id = $1 AND kind = $2 AND occurred_at >= $3",
    )
    .bind(user_id)
    .bind(kind)
    .bind(since)
    .fetch_one(pool)
    .await?)
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
