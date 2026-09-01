//! FeedRefreshAgent: find feeds due for refresh and enqueue exactly one
//! `fetch_feed` job per feed (deduped), regardless of how many users subscribe.

use async_trait::async_trait;
use serde_json::json;
use time::Duration;
use uuid::Uuid;

use rusty_feed_db::{agent_locks, feeds, outbox};

use crate::{Agent, AgentContext};

/// How many due feeds to enqueue per run.
const DUE_LIMIT: i64 = 1000;

pub struct FeedRefreshAgent;

#[async_trait]
impl Agent for FeedRefreshAgent {
    fn name(&self) -> &'static str {
        "feed_refresh"
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        _payload: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let pool = ctx.db.pool();

        // Singleton lease: only one instance scans/enqueues at a time.
        let holder = Uuid::now_v7().to_string();
        if !agent_locks::acquire(pool, "feed_refresh", &holder, Duration::minutes(10)).await? {
            return Ok(json!({ "skipped": "another instance holds the lease" }));
        }

        let due = feeds::list_due(pool, DUE_LIMIT).await?;
        let mut transaction = pool.begin().await?;
        let mut enqueued = 0u64;
        for feed in &due {
            outbox::publish(
                &mut transaction,
                "fetch_feed",
                json!({
                    "feed_id": feed.id,
                    "_dedup_key": format!("fetch_feed:{}", feed.id),
                }),
            )
            .await?;
            enqueued += 1;
        }
        transaction.commit().await?;

        agent_locks::release(pool, "feed_refresh", &holder).await?;
        Ok(json!({ "due": due.len(), "enqueued": enqueued }))
    }
}
