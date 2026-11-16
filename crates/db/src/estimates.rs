//! Planner-estimated row counts for display-only totals.
//!
//! An exact `count(*)` over a large shared table is a full index scan on every
//! admin page load. PostgreSQL already keeps an approximate live-row count in
//! `pg_class.reltuples`, maintained by `VACUUM`/`ANALYZE`/autovacuum. Never use
//! these numbers for pagination, filters, quotas or limits.

use sqlx::PgPool;

use crate::error::Result;

/// Tables whose approximate size may be read from planner statistics. A closed
/// enum keeps caller-provided strings out of the SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimatedTable {
    /// Shared `articles` rows.
    Articles,
    /// Per-user `user_feeds` subscriptions.
    UserFeeds,
}

impl EstimatedTable {
    fn relation(self) -> &'static str {
        match self {
            Self::Articles => "articles",
            Self::UserFeeds => "user_feeds",
        }
    }

    fn exact_count_sql(self) -> &'static str {
        match self {
            Self::Articles => "SELECT count(*)::bigint FROM articles",
            Self::UserFeeds => "SELECT count(*)::bigint FROM user_feeds",
        }
    }
}

/// Use the planner statistic only when it is at least one row. A negative value means
/// the table was never vacuumed or analyzed, and zero may simply be stale for a
/// table that has since received rows; both need an exact count.
fn usable_estimate(reltuples: f32) -> Option<i64> {
    (reltuples >= 1.0).then(|| reltuples.round() as i64)
}

async fn estimate_relation(pool: &PgPool, relation: &str) -> Result<Option<i64>> {
    let reltuples: Option<f32> =
        sqlx::query_scalar("SELECT reltuples FROM pg_class WHERE oid = to_regclass($1)")
            .bind(relation)
            .fetch_optional(pool)
            .await?;
    Ok(reltuples.and_then(usable_estimate))
}

/// Approximate live row count of `table` from `pg_class.reltuples`, falling back
/// to an exact `count(*)` when the statistic is missing, negative or zero.
///
/// The table is a closed enum, so no caller-provided string can reach the SQL:
///
/// ```compile_fail
/// # async fn demo(pool: &sqlx::PgPool) {
/// let _ = rusty_feed_db::estimates::estimated_rows(pool, "articles").await;
/// # }
/// ```
pub async fn estimated_rows(pool: &PgPool, table: EstimatedTable) -> Result<i64> {
    if let Some(estimate) = estimate_relation(pool, table.relation()).await? {
        return Ok(estimate);
    }
    Ok(sqlx::query_scalar(table.exact_count_sql())
        .fetch_one(pool)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_positive_statistics_are_trusted() {
        assert_eq!(usable_estimate(-1.0), None);
        assert_eq!(usable_estimate(0.0), None);
        assert_eq!(usable_estimate(0.4), None);
        assert_eq!(usable_estimate(1.0), Some(1));
        assert_eq!(usable_estimate(395_000.4), Some(395_000));
        assert_eq!(usable_estimate(f32::NAN), None);
    }

    async fn scratch_pool() -> Option<PgPool> {
        let url = std::env::var("TEST_DATABASE_URL").ok()?;
        Some(PgPool::connect(&url).await.expect("connect"))
    }

    async fn scratch_table(pool: &PgPool) -> String {
        let name = format!("est_scratch_{}", uuid::Uuid::now_v7().simple());
        sqlx::query(&format!(
            "CREATE TABLE {name} (id int) WITH (autovacuum_enabled = false)"
        ))
        .execute(pool)
        .await
        .unwrap();
        name
    }

    async fn reltuples(pool: &PgPool, name: &str) -> f32 {
        sqlx::query_scalar("SELECT reltuples FROM pg_class WHERE oid = to_regclass($1)")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn estimate_is_exact_after_analyze_and_absent_before() {
        let Some(pool) = scratch_pool().await else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let name = scratch_table(&pool).await;

        sqlx::query(&format!(
            "INSERT INTO {name} SELECT generate_series(1, 250)"
        ))
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            reltuples(&pool, &name).await < 1.0,
            "a table autovacuum has never seen has no usable statistic"
        );
        assert_eq!(estimate_relation(&pool, &name).await.unwrap(), None);

        sqlx::query(&format!("ANALYZE {name}"))
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(estimate_relation(&pool, &name).await.unwrap(), Some(250));

        sqlx::query(&format!("DROP TABLE {name}"))
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(estimate_relation(&pool, &name).await.unwrap(), None);
    }

    #[tokio::test]
    async fn stale_zero_statistic_is_not_trusted() {
        let Some(pool) = scratch_pool().await else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let name = scratch_table(&pool).await;
        sqlx::query(&format!("ANALYZE {name}"))
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(reltuples(&pool, &name).await, 0.0);
        sqlx::query(&format!("INSERT INTO {name} SELECT generate_series(1, 40)"))
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            estimate_relation(&pool, &name).await.unwrap(),
            None,
            "a zero statistic must fall back to the exact count"
        );
        let exact: i64 = sqlx::query_scalar(&format!("SELECT count(*)::bigint FROM {name}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(exact, 40);
        sqlx::query(&format!("DROP TABLE {name}"))
            .execute(&pool)
            .await
            .unwrap();
    }

    #[test]
    fn every_table_maps_to_its_own_relation_and_exact_count() {
        for table in [EstimatedTable::Articles, EstimatedTable::UserFeeds] {
            assert!(table.exact_count_sql().ends_with(table.relation()));
        }
    }
}
