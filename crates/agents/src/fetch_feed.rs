//! FetchFeedAgent: fetch + parse + ingest a single shared feed (via rusty-feed-feeds).

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::{Agent, AgentContext};
use rusty_feed_db::{articles, feeds, integrations, jobs, subscriptions};

/// Cap on extraction jobs enqueued per feed refresh.
const EXTRACT_ENQUEUE_LIMIT: i64 = 50;

#[derive(Deserialize)]
struct Payload {
    feed_id: Uuid,
}

pub struct FetchFeedAgent;

async fn enqueue_embeddings(
    ctx: &AgentContext,
    pending: Vec<rusty_feed_db::articles::ArticleRevision>,
) -> anyhow::Result<u64> {
    let mut enqueued = 0;
    for article in pending {
        if crate::worker::enqueue_embed(&ctx.db, article.id, article.content_revision).await? {
            enqueued += 1;
        }
    }
    Ok(enqueued)
}

async fn enqueue_summaries(
    ctx: &AgentContext,
    pending: Vec<rusty_feed_db::articles::ArticleRevision>,
) -> anyhow::Result<u64> {
    let mut enqueued = 0;
    for article in pending {
        if crate::worker::enqueue_summary(&ctx.db, article.id, article.content_revision).await? {
            enqueued += 1;
        }
    }
    Ok(enqueued)
}

async fn enqueue_new_entry_deliveries(
    ctx: &AgentContext,
    feed_id: Uuid,
    article_ids: &[Uuid],
) -> anyhow::Result<u64> {
    if article_ids.is_empty() {
        return Ok(0);
    }
    let mut enqueued = 0;
    for user_id in subscriptions::subscriber_ids(ctx.db.pool(), feed_id).await? {
        for profile in
            integrations::profiles_for_event(ctx.db.pool(), user_id, "feed.new_entries").await?
        {
            for article_id in article_ids {
                let delivery = integrations::queue_delivery(
                    ctx.db.pool(),
                    &profile,
                    Some(*article_id),
                    "feed.new_entries",
                )
                .await?;
                if jobs::enqueue(
                    ctx.db.pool(),
                    "integration_delivery",
                    json!({"delivery_id": delivery.id}),
                    Some(&format!("integration:{}", delivery.id)),
                    None,
                    profile.max_attempts,
                )
                .await?
                .is_some()
                {
                    enqueued += 1;
                }
            }
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
        tracing::info!(feed_id = %feed_id, feed = %feed.name, url = %feed.url, "fetch_feed: refreshing feed");
        if feed.health_status == "disabled" {
            return Ok(json!({ "feed_id": feed_id, "skipped": "feed is paused" }));
        }
        // Belt-and-suspenders: never fetch a blocked feed even if a stale job
        // was queued before the block landed (FND-192).
        if feed.blocked_at.is_some() {
            return Ok(json!({ "feed_id": feed_id, "skipped": "feed is blocked" }));
        }

        // Compile the blocklist once per refresh so per-article checks are O(1).
        let blocklist = rusty_feed_db::feed_blocklist::compiled(ctx.db.pool())
            .await
            .unwrap_or_default();
        let acquisition_fetch = ctx.fetch_for_feed(feed_id).await?;
        let stats = rusty_feed_feeds::refresh_feed_with_policies(
            &ctx.db,
            &acquisition_fetch,
            &feed,
            &ctx.tracking_url_policy,
            &ctx.tracking_pixel_policy,
            ctx.feed_max_items_per_refresh,
            &blocklist,
        )
        .await?;
        let integration_jobs =
            enqueue_new_entry_deliveries(ctx, feed_id, &stats.created_article_ids).await?;

        // If any subscriber requests full-text extraction, enqueue one
        // (deduped) extract job per absent/expired shared page copy.
        // (Embedding for those articles is enqueued after extraction.) Otherwise,
        // embed directly on ingest when enabled.
        let article_page_lifetime =
            subscriptions::requested_article_page_lifetime(ctx.db.pool(), feed_id).await?;
        let mut extract_jobs = 0u64;
        let mut embed_jobs = 0u64;
        let mut summarize_jobs = 0u64;
        if article_page_lifetime.is_some() {
            // Only pre-extract public pages in the background when eager
            // extraction is enabled; otherwise a page is fetched lazily on the
            // first reader access (in the interactive lane).
            if ctx.eager_extraction {
                extract_jobs = jobs::enqueue_extractions_for_feed(
                    ctx.db.pool(),
                    feed_id,
                    EXTRACT_ENQUEUE_LIMIT as usize,
                )
                .await?;
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
            if ctx.auto_summarize {
                summarize_jobs = enqueue_summaries(
                    ctx,
                    articles::list_ready_for_summary(ctx.db.pool(), feed_id, EXTRACT_ENQUEUE_LIMIT)
                        .await?,
                )
                .await?;
            }
        } else if ctx.embed_on_ingest {
            let pending =
                articles::list_needing_embedding(ctx.db.pool(), feed_id, EXTRACT_ENQUEUE_LIMIT)
                    .await?;
            embed_jobs = enqueue_embeddings(ctx, pending).await?;
        }
        if article_page_lifetime.is_none() && ctx.auto_summarize {
            summarize_jobs = enqueue_summaries(
                ctx,
                articles::list_needing_summary(ctx.db.pool(), feed_id, EXTRACT_ENQUEUE_LIMIT)
                    .await?,
            )
            .await?;
        }

        Ok(json!({
            "feed_id": feed_id,
            "created": stats.created,
            "updated": stats.updated,
            "skipped": stats.skipped,
            "not_modified": stats.not_modified,
            "extract_jobs": extract_jobs,
            "embed_jobs": embed_jobs,
            "summarize_jobs": summarize_jobs,
            "integration_jobs": integration_jobs,
        }))
    }
}
