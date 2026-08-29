//! ExtractContentAgent: enrich a shared article with full text (once per
//! article), via `rustss-extract` and the SSRF-safe client.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use rustss_core::feed_content::FeedContentSettings;
use rustss_db::{articles, feeds};
use rustss_extract::{ExtractOutcome, ExtractionService};

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
        // Once per shared article.
        if article.extraction_status != "none" {
            return Ok(json!({ "skipped": "already processed" }));
        }

        let Some(feed) = feeds::find_by_id(pool, article.feed_id).await? else {
            return Ok(json!({ "skipped": "feed gone" }));
        };
        let content_settings = FeedContentSettings::from_json(feed.extraction_settings.as_ref());
        if !content_settings.mode.fetches_article_pages() {
            return Ok(json!({ "skipped": "feed uses RSS content" }));
        }

        match self
            .service
            .extract(
                &ctx.db,
                &ctx.fetch,
                &article.url,
                content_settings.selector.as_deref(),
                feed.respect_robots_txt,
                feed.extraction_delay_ms.map(|delay| delay.max(0) as u64),
            )
            .await
        {
            ExtractOutcome::Extracted(e) => {
                let updated = articles::set_extracted_content(
                    pool,
                    article_id,
                    content_revision,
                    &e.content,
                    e.excerpt.as_deref(),
                )
                .await?;
                let Some(content_revision) = updated else {
                    return Ok(json!({ "skipped": "stale content revision" }));
                };
                // Embed using the freshly-extracted content.
                if ctx.embed_on_ingest {
                    crate::worker::enqueue_embed(&ctx.db, article_id, content_revision).await?;
                }
                Ok(json!({ "status": "success", "extractor": e.extractor }))
            }
            ExtractOutcome::Disallowed => {
                let revision = articles::set_extraction_failed(
                    pool,
                    article_id,
                    content_revision,
                    "robots disallow",
                )
                .await?;
                if ctx.embed_on_ingest {
                    if let Some(revision) = revision {
                        crate::worker::enqueue_embed(&ctx.db, article_id, revision).await?;
                    }
                }
                Ok(json!({ "status": "disallowed" }))
            }
            ExtractOutcome::Failed(msg) => {
                // Soft failure: keep the RSS content, don't retry the job.
                let revision =
                    articles::set_extraction_failed(pool, article_id, content_revision, &msg)
                        .await?;
                if ctx.embed_on_ingest {
                    if let Some(revision) = revision {
                        crate::worker::enqueue_embed(&ctx.db, article_id, revision).await?;
                    }
                }
                Ok(json!({ "status": "failed", "reason": msg }))
            }
            ExtractOutcome::RateLimited { retry_after } => {
                // Hard signal: fail the job so it backs off and retries later.
                anyhow::bail!("rate limited; retry after {}s", retry_after.as_secs())
            }
        }
    }
}
