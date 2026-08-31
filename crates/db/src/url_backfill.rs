//! Collision-aware canonical URL backfill for legacy article rows.

use std::collections::{HashMap, HashSet};

use rustss_core::tracking_url::TrackingUrlPolicy;
use sqlx::PgPool;
use uuid::Uuid;

use crate::Result;

#[derive(Debug, Clone, sqlx::FromRow)]
struct ArticleUrl {
    id: Uuid,
    url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlBackfillChange {
    pub article_id: Uuid,
    pub current_url: String,
    pub canonical_url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UrlBackfillPlan {
    pub changes: Vec<UrlBackfillChange>,
    pub collisions: Vec<UrlBackfillChange>,
    pub unchanged: usize,
}

fn plan_rows(rows: Vec<ArticleUrl>, policy: &TrackingUrlPolicy) -> UrlBackfillPlan {
    let existing: HashMap<_, _> = rows.iter().map(|row| (row.url.as_str(), row.id)).collect();
    let candidates: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            let canonical_url = policy.clean(&row.url);
            (canonical_url != row.url).then(|| UrlBackfillChange {
                article_id: row.id,
                current_url: row.url.clone(),
                canonical_url,
            })
        })
        .collect();
    let candidate_count = candidates.len();
    let mut duplicate_targets = HashSet::new();
    let mut first_target = HashSet::new();
    for candidate in &candidates {
        if !first_target.insert(candidate.canonical_url.clone()) {
            duplicate_targets.insert(candidate.canonical_url.clone());
        }
    }
    let (collisions, changes) = candidates.into_iter().partition(|candidate| {
        duplicate_targets.contains(candidate.canonical_url.as_str())
            || existing
                .get(candidate.canonical_url.as_str())
                .is_some_and(|owner| *owner != candidate.article_id)
    });
    UrlBackfillPlan {
        changes,
        collisions,
        unchanged: rows.len() - candidate_count,
    }
}

/// Inspect all legacy article keys without changing data.
pub async fn plan(pool: &PgPool, policy: &TrackingUrlPolicy) -> Result<UrlBackfillPlan> {
    let rows = sqlx::query_as("SELECT id, url FROM articles ORDER BY id")
        .fetch_all(pool)
        .await?;
    Ok(plan_rows(rows, policy))
}

/// Apply only collision-free changes from a previously generated plan. The
/// old URL predicate makes concurrent modifications visible instead of
/// overwriting them.
pub async fn apply(pool: &PgPool, plan: &UrlBackfillPlan) -> Result<u64> {
    let mut transaction = pool.begin().await?;
    let mut changed = 0;
    for change in &plan.changes {
        changed += sqlx::query(
            "UPDATE articles SET url = $2, updated_at = now() WHERE id = $1 AND url = $3",
        )
        .bind(change.article_id)
        .bind(&change.canonical_url)
        .bind(&change.current_url)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
    }
    transaction.commit().await?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_existing_and_many_to_one_collisions() {
        let rows = vec![
            ArticleUrl {
                id: Uuid::now_v7(),
                url: "https://example.test/a?utm_source=one".into(),
            },
            ArticleUrl {
                id: Uuid::now_v7(),
                url: "https://example.test/a".into(),
            },
            ArticleUrl {
                id: Uuid::now_v7(),
                url: "https://example.test/b?utm_source=one".into(),
            },
            ArticleUrl {
                id: Uuid::now_v7(),
                url: "https://example.test/b?utm_source=two".into(),
            },
            ArticleUrl {
                id: Uuid::now_v7(),
                url: "https://example.test/c?utm_source=one".into(),
            },
        ];
        let plan = plan_rows(rows, &TrackingUrlPolicy::default());
        assert_eq!(plan.changes.len(), 1);
        assert_eq!(plan.changes[0].canonical_url, "https://example.test/c");
        assert_eq!(plan.collisions.len(), 3);
    }
}
