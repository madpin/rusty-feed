//! ExtractContentAgent: enrich a shared article with full text (once per
//! article), via `rusty-feed-extract` and the SSRF-safe client.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use rusty_feed_core::{
    content_rules::apply_rewrite_rules,
    feed_content::FeedContentSettings,
    html::{sanitize_article_html_with_policies, ARTICLE_PAGE_PROCESSING_VERSION},
};
use rusty_feed_db::{articles, feeds, subscriptions};
use rusty_feed_extract::{ExtractOutcome, ExtractionService};

use crate::{Agent, AgentContext};

#[derive(Deserialize)]
struct Payload {
    article_id: Uuid,
    #[serde(default)]
    content_revision: Option<i64>,
}

pub struct ExtractContentAgent {
    service: Arc<ExtractionService>,
}

impl ExtractContentAgent {
    pub fn new(service: Arc<ExtractionService>) -> Self {
        Self { service }
    }
}

impl Default for ExtractContentAgent {
    fn default() -> Self {
        Self::new(Arc::new(ExtractionService::default()))
    }
}

async fn store_failure(
    ctx: &AgentContext,
    article_id: Uuid,
    content_revision: i64,
    failure_class: &str,
) -> anyhow::Result<Value> {
    let revision =
        articles::set_extraction_failed(ctx.db.pool(), article_id, content_revision, failure_class)
            .await?;
    if ctx.embed_on_ingest {
        if let Some(revision) = revision {
            crate::worker::enqueue_embed(&ctx.db, article_id, revision).await?;
        }
    }
    if ctx.auto_summarize {
        if let Some(revision) = revision {
            crate::worker::enqueue_summary(&ctx.db, article_id, revision).await?;
        }
    }
    Ok(json!({ "status": "failed", "failure_class": failure_class }))
}

#[async_trait]
impl Agent for ExtractContentAgent {
    fn name(&self) -> &'static str {
        "extract_content"
    }

    async fn run(&self, ctx: &AgentContext, payload: Value) -> anyhow::Result<Value> {
        let Payload {
            article_id,
            content_revision,
        } = serde_json::from_value(payload)?;
        let pool = ctx.db.pool();

        let Some(article) = articles::find_by_id(pool, article_id).await? else {
            return Ok(json!({ "skipped": "article gone" }));
        };
        let content_revision = content_revision.unwrap_or(article.content_revision);
        if article.content_revision != content_revision {
            return Ok(json!({ "skipped": "stale content revision" }));
        }
        let Some(feed) = feeds::find_by_id(pool, article.feed_id).await? else {
            return Ok(json!({ "skipped": "feed gone" }));
        };
        let Some(lifetime) =
            subscriptions::requested_article_page_lifetime(pool, article.feed_id).await?
        else {
            return Ok(json!({ "skipped": "no subscriber requests public-page content" }));
        };
        let page_is_current = article.article_page_content.is_some()
            && article.article_page_processing_version >= ARTICLE_PAGE_PROCESSING_VERSION
            && article
                .article_page_expires_at
                .is_none_or(|expires_at| expires_at > time::OffsetDateTime::now_utc());
        if page_is_current {
            return Ok(json!({ "skipped": "public-page content is current" }));
        }
        let content_settings = FeedContentSettings::from_json(feed.extraction_settings.as_ref());
        let canonical_url = ctx.tracking_url_policy.clean(&article.url);
        tracing::info!(
            article_id = %article_id,
            feed = %feed.name,
            url = %canonical_url,
            "extract_content: fetching publisher page"
        );
        let acquisition_fetch = ctx.fetch_for_feed(article.feed_id).await?;

        match self
            .service
            .extract_with_tracking_policy(
                &ctx.db,
                &acquisition_fetch,
                &canonical_url,
                content_settings.selector.as_deref(),
                feed.respect_robots_txt,
                feed.extraction_delay_ms.map(|delay| delay.max(0) as u64),
                &ctx.tracking_pixel_policy,
            )
            .await
        {
            ExtractOutcome::Extracted(e) => {
                let rewritten = apply_rewrite_rules(&e.content, &content_settings.rewrite_rules)
                    .map_err(|error| anyhow::anyhow!("content rewrite failed: {error}"))?;
                let report = sanitize_article_html_with_policies(
                    &rewritten,
                    &canonical_url,
                    &ctx.tracking_url_policy,
                    &ctx.tracking_pixel_policy,
                );
                let blocked_tracking_pixels = e
                    .blocked_tracking_pixels
                    .saturating_add(report.blocked_tracking_pixels);
                let updated = articles::set_extracted_content_with_privacy(
                    pool,
                    article_id,
                    content_revision,
                    &report.html,
                    e.excerpt.as_deref(),
                    lifetime,
                    blocked_tracking_pixels,
                )
                .await?;
                let Some(content_revision) = updated else {
                    return Ok(json!({ "skipped": "stale content revision" }));
                };
                // Embed using the freshly-extracted content.
                if ctx.embed_on_ingest {
                    crate::worker::enqueue_embed(&ctx.db, article_id, content_revision).await?;
                }
                if ctx.auto_summarize {
                    crate::worker::enqueue_summary(&ctx.db, article_id, content_revision).await?;
                }
                Ok(json!({ "status": "success", "extractor": e.extractor }))
            }
            // Soft failure: keep the RSS content, don't retry the job.
            ExtractOutcome::Disallowed => {
                let _ =
                    store_failure(ctx, article_id, content_revision, "robots_disallowed").await?;
                Ok(json!({ "status": "disallowed" }))
            }
            ExtractOutcome::RobotsUnavailable => {
                store_failure(ctx, article_id, content_revision, "robots_unavailable").await
            }
            ExtractOutcome::HttpStatus(status) => {
                let failure_class = format!("http_{status}");
                store_failure(ctx, article_id, content_revision, &failure_class).await
            }
            ExtractOutcome::ReaderFailed => {
                store_failure(ctx, article_id, content_revision, "reader_failed").await
            }
            ExtractOutcome::FetchFailed(failure) => {
                store_failure(ctx, article_id, content_revision, failure.as_str()).await
            }
            ExtractOutcome::RateLimited { retry_after } => {
                // Hard signal: fail the job so it backs off and retries later.
                anyhow::bail!("rate limited; retry after {}s", retry_after.as_secs())
            }
        }
    }
}
