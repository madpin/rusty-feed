//! FetchFeedAgent: fetch + parse + ingest a single shared feed (via rustss-feeds).

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::{Agent, AgentContext};
use rustss_core::feed_content::FeedContentSettings;
use rustss_db::{articles, feeds, jobs};

/// Cap on extraction jobs enqueued per feed refresh.
const EXTRACT_ENQUEUE_LIMIT: i64 = 50;

#[derive(Deserialize)]
struct Payload {
    feed_id: Uuid,
}

pub struct FetchFeedAgent;

async fn enqueue_embeddings(
    ctx: &AgentContext,
    pending: Vec<rustss_db::articles::ArticleRevision>,
) -> anyhow::Result<u64> {
    let mut enqueued = 0;
    for article in pending {
        if crate::worker::enqueue_embed(&ctx.db, article.id, article.content_revision).await? {
            enqueued += 1;
        }
    }
    Ok(enqueued)
}

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
        if feed.health_status == "disabled" {
            return Ok(json!({ "feed_id": feed_id, "skipped": "feed is paused" }));
        }

        let stats = rustss_feeds::refresh_feed(&ctx.db, &ctx.fetch, &feed).await?;

        // If the feed opts into full-text extraction, enqueue one (deduped)
        // extract job per not-yet-processed article — once per shared article.
        // (Embedding for those articles is enqueued after extraction.) Otherwise,
        // embed directly on ingest when enabled.
        let content_settings = FeedContentSettings::from_json(feed.extraction_settings.as_ref());
        let mut extract_jobs = 0u64;
        let mut embed_jobs = 0u64;
        if content_settings.mode.fetches_article_pages() {
            let pending =
                articles::list_needing_extraction(ctx.db.pool(), feed_id, EXTRACT_ENQUEUE_LIMIT)
                    .await?;
            for article in pending {
                let dedup = format!("extract:{}:{}", article.id, article.content_revision);
                if jobs::enqueue(
                    ctx.db.pool(),
                    "extract_content",
                    json!({
                        "article_id": article.id,
                        "content_revision": article.content_revision,
                    }),
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
            if ctx.embed_on_ingest {
                let pending = articles::list_ready_for_embedding(
                    ctx.db.pool(),
                    feed_id,
                    EXTRACT_ENQUEUE_LIMIT,
                )
                .await?;
                embed_jobs = enqueue_embeddings(ctx, pending).await?;
            }
        } else if ctx.embed_on_ingest {
            let pending =
                articles::list_needing_embedding(ctx.db.pool(), feed_id, EXTRACT_ENQUEUE_LIMIT)
                    .await?;
            embed_jobs = enqueue_embeddings(ctx, pending).await?;
        }

        Ok(json!({
            "feed_id": feed_id,
            "created": stats.created,
            "updated": stats.updated,
            "skipped": stats.skipped,
            "not_modified": stats.not_modified,
            "extract_jobs": extract_jobs,
            "embed_jobs": embed_jobs,
        }))
    }
}
