//! FetchFeedAgent: fetch + parse + ingest a single shared feed (via rustss-feeds).

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use rustss_db::{articles, feeds, jobs};

use crate::extract_content::feed_extraction;
use crate::{Agent, AgentContext};

/// Cap on extraction jobs enqueued per feed refresh.
const EXTRACT_ENQUEUE_LIMIT: i64 = 50;

#[derive(Deserialize)]
struct Payload {
    feed_id: Uuid,
}

pub struct FetchFeedAgent;

#[async_trait]
impl Agent for FetchFeedAgent {
    fn name(&self) -> &'static str {
        "fetch_feed"
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        payload: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let Payload { feed_id } = serde_json::from_value(payload)?;
        let feed = feeds::find_by_id(ctx.db.pool(), feed_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("feed {feed_id} not found"))?;

        let stats = rustss_feeds::refresh_feed(&ctx.db, &ctx.fetch, &feed).await?;

        // If the feed opts into full-text extraction, enqueue one (deduped)
        // extract job per not-yet-processed article — once per shared article.
        let (extraction_enabled, _) = feed_extraction(&feed.extraction_settings);
        let mut extract_jobs = 0u64;
        if extraction_enabled {
            let pending =
                articles::list_needing_extraction(ctx.db.pool(), feed_id, EXTRACT_ENQUEUE_LIMIT)
                    .await?;
            for article_id in pending {
                let dedup = format!("extract:{article_id}");
                if jobs::enqueue(
                    ctx.db.pool(),
                    "extract_content",
                    json!({ "article_id": article_id }),
                    Some(&dedup),
                    None,
                    3,
                )
                .await?
                .is_some()
                {
                    extract_jobs += 1;
                }
            }
        }

        Ok(json!({
            "feed_id": feed_id,
            "created": stats.created,
            "updated": stats.updated,
            "skipped": stats.skipped,
            "not_modified": stats.not_modified,
            "extract_jobs": extract_jobs,
        }))
    }
}
