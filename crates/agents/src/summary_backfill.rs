use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use rusty_feed_db::articles;

use crate::{worker, Agent, AgentContext};

const BATCH_SIZE: i64 = 100;

#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    feed_id: Option<Uuid>,
    #[serde(default)]
    ready_only: bool,
    #[serde(default)]
    after: Option<Uuid>,
}

/// Cursor-bounded scanner that queues eligible shared summary revisions.
pub struct SummaryBackfillAgent;

#[async_trait]
impl Agent for SummaryBackfillAgent {
    fn name(&self) -> &'static str {
        "summary_backfill"
    }

    async fn run(&self, ctx: &AgentContext, payload: Value) -> anyhow::Result<Value> {
        let Payload {
            feed_id,
            ready_only,
            after,
        } = serde_json::from_value(payload)?;
        let policy = ctx.summary_policy().await?;
        if !policy.enabled {
            return Ok(json!({ "skipped": "automatic summaries disabled" }));
        }
        let candidates = articles::list_summary_candidates_after(
            ctx.db.pool(),
            feed_id,
            ready_only,
            after,
            BATCH_SIZE,
        )
        .await?;
        let next_after = candidates.last().map(|candidate| candidate.id);
        let has_more = candidates.len() == BATCH_SIZE as usize;
        let mut enqueued = 0u64;
        for candidate in candidates {
            if candidate.meets_minimum(policy.min_content_chars)
                && worker::enqueue_summary(&ctx.db, candidate.id, candidate.content_revision)
                    .await?
            {
                enqueued += 1;
            }
        }
        if has_more {
            worker::enqueue_summary_backfill(&ctx.db, feed_id, ready_only, next_after).await?;
        }
        Ok(json!({
            "feed_id": feed_id,
            "enqueued": enqueued,
            "has_more": has_more,
        }))
    }
}
