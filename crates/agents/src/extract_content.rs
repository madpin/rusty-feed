//! ExtractContentAgent: enrich a shared article with full text (once per
//! article), via `rustss-extract` and the SSRF-safe client.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use rustss_db::{articles, feeds};
use rustss_extract::{ExtractOutcome, ExtractionService};

use crate::{Agent, AgentContext};

#[derive(Deserialize)]
struct Payload {
    article_id: Uuid,
}

/// Whether a feed opts into full-text extraction, and its optional CSS selector.
pub(crate) fn feed_extraction(settings: &Option<Value>) -> (bool, Option<String>) {
    match settings {
        Some(v) => {
            let enabled = v.get("enabled").and_then(Value::as_bool).unwrap_or(false);
            let selector = v
                .get("selector")
                .and_then(Value::as_str)
                .map(str::to_string);
            (enabled, selector)
        }
        None => (false, None),
    }
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
        let Payload { article_id } = serde_json::from_value(payload)?;
        let pool = ctx.db.pool();

        let Some(article) = articles::find_by_id(pool, article_id).await? else {
            return Ok(json!({ "skipped": "article gone" }));
        };
        // Once per shared article.
        if article.extraction_status != "none" {
            return Ok(json!({ "skipped": "already processed" }));
        }

        let feed = feeds::find_by_id(pool, article.feed_id).await?;
        let respect_robots = feed.as_ref().map(|f| f.respect_robots_txt).unwrap_or(true);
        let (_enabled, selector) =
            feed_extraction(&feed.as_ref().and_then(|f| f.extraction_settings.clone()));

        match self
            .service
            .extract(
                &ctx.db,
                &ctx.fetch,
                &article.url,
                selector.as_deref(),
                respect_robots,
            )
            .await
        {
            ExtractOutcome::Extracted(e) => {
                articles::set_extracted_content(
                    pool,
                    article_id,
                    &e.content,
                    e.excerpt.as_deref(),
                    "success",
                    None,
                )
                .await?;
                // Embed using the freshly-extracted content.
                if ctx.embed_on_ingest {
                    crate::worker::enqueue_embed(&ctx.db, article_id).await?;
                }
                Ok(json!({ "status": "success", "extractor": e.extractor }))
            }
            ExtractOutcome::Disallowed => {
                articles::set_extraction_failed(pool, article_id, "robots disallow").await?;
                Ok(json!({ "status": "disallowed" }))
            }
            ExtractOutcome::Failed(msg) => {
                // Soft failure: keep the RSS content, don't retry the job.
                articles::set_extraction_failed(pool, article_id, &msg).await?;
                Ok(json!({ "status": "failed", "reason": msg }))
            }
            ExtractOutcome::RateLimited { retry_after } => {
                // Hard signal: fail the job so it backs off and retries later.
                anyhow::bail!("rate limited; retry after {}s", retry_after.as_secs())
            }
        }
    }
}
