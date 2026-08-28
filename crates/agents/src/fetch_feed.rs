//! FetchFeedAgent: fetch + parse + ingest a single shared feed (via rustss-feeds).

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use rustss_db::feeds;

use crate::{Agent, AgentContext};

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
        Ok(json!({
            "feed_id": feed_id,
            "created": stats.created,
            "updated": stats.updated,
            "skipped": stats.skipped,
            "not_modified": stats.not_modified,
        }))
    }
}
