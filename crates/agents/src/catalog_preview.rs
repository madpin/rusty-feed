use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use rustss_db::catalog::{self, ContentProbeCandidate};
use rustss_extract::{ExtractOutcome, Extracted, ExtractionService};

use crate::{Agent, AgentContext};

const MAX_SAMPLE_ARTICLES: i64 = 3;

#[derive(Deserialize)]
struct Payload {
    feed_id: Uuid,
}

pub struct CatalogPreviewAgent {
    service: Arc<ExtractionService>,
}

impl CatalogPreviewAgent {
    pub fn new(service: Arc<ExtractionService>) -> Self {
        Self { service }
    }
}

impl Default for CatalogPreviewAgent {
    fn default() -> Self {
        Self::new(Arc::new(ExtractionService::default()))
    }
}

fn extracted_sample(
    candidate: &ContentProbeCandidate,
    extracted: Extracted,
) -> catalog::ContentProbeWrite {
    let status = if extracted.extractor == "readability" {
        catalog::ContentProbeStatus::Full
    } else {
        catalog::ContentProbeStatus::Partial
    };
    catalog::ContentProbeWrite {
        article_id: candidate.id,
        rss_body_chars: candidate.rss_body_chars,
        processed_body_chars: i32::try_from(extracted.content.chars().count()).ok(),
        status,
    }
}

fn unavailable_sample(
    candidate: &ContentProbeCandidate,
    status: catalog::ContentProbeStatus,
) -> catalog::ContentProbeWrite {
    catalog::ContentProbeWrite {
        article_id: candidate.id,
        rss_body_chars: candidate.rss_body_chars,
        processed_body_chars: None,
        status,
    }
}

#[async_trait]
impl Agent for CatalogPreviewAgent {
    fn name(&self) -> &'static str {
        "catalog_content_probe"
    }

    async fn run(&self, ctx: &AgentContext, payload: Value) -> anyhow::Result<Value> {
        let Payload { feed_id } = serde_json::from_value(payload)?;
        if !catalog::is_published_feed(ctx.db.pool(), feed_id).await? {
            return Ok(json!({ "skipped": "catalog feed unavailable" }));
        }
        if !catalog::probe_needs_refresh(ctx.db.pool(), feed_id).await? {
            return Ok(json!({ "skipped": "current probe" }));
        }
        let candidates = catalog::content_probe_candidates(ctx.db.pool(), feed_id).await?;
        if candidates.is_empty() {
            return Ok(json!({ "skipped": "no recent articles" }));
        }
        let mut samples = Vec::with_capacity(candidates.len());
        for candidate in candidates.iter().take(MAX_SAMPLE_ARTICLES as usize) {
            let outcome = self
                .service
                .probe_public_strict(&ctx.db, &ctx.fetch, &candidate.url)
                .await;
            let rate_limited = matches!(&outcome, ExtractOutcome::RateLimited { .. });
            let sample = match outcome {
                ExtractOutcome::Extracted(extracted) => extracted_sample(candidate, extracted),
                ExtractOutcome::Disallowed => {
                    unavailable_sample(candidate, catalog::ContentProbeStatus::RobotsDisallowed)
                }
                ExtractOutcome::HttpStatus(401) => {
                    unavailable_sample(candidate, catalog::ContentProbeStatus::Http401)
                }
                ExtractOutcome::HttpStatus(402) => {
                    unavailable_sample(candidate, catalog::ContentProbeStatus::Http402)
                }
                ExtractOutcome::HttpStatus(403) => {
                    unavailable_sample(candidate, catalog::ContentProbeStatus::Http403)
                }
                ExtractOutcome::HttpStatus(451) => {
                    unavailable_sample(candidate, catalog::ContentProbeStatus::Http451)
                }
                ExtractOutcome::RateLimited { .. } => {
                    unavailable_sample(candidate, catalog::ContentProbeStatus::RateLimited)
                }
                ExtractOutcome::ReaderFailed => {
                    unavailable_sample(candidate, catalog::ContentProbeStatus::ReaderFailed)
                }
                ExtractOutcome::RobotsUnavailable
                | ExtractOutcome::HttpStatus(_)
                | ExtractOutcome::FetchFailed(_) => {
                    unavailable_sample(candidate, catalog::ContentProbeStatus::FetchFailed)
                }
            };
            samples.push(sample);
            if rate_limited {
                break;
            }
        }
        catalog::replace_content_probe(ctx.db.pool(), feed_id, &candidates, &samples).await?;
        let full = samples
            .iter()
            .filter(|sample| sample.status == catalog::ContentProbeStatus::Full)
            .count();
        let partial = samples
            .iter()
            .filter(|sample| sample.status == catalog::ContentProbeStatus::Partial)
            .count();
        let http_402 = samples
            .iter()
            .filter(|sample| sample.status == catalog::ContentProbeStatus::Http402)
            .count();
        let failed = samples
            .iter()
            .filter(|sample| sample.status == catalog::ContentProbeStatus::FetchFailed)
            .count();
        Ok(json!({
            "sampled": samples.len(),
            "full_content": full,
            "partial_content": partial,
            "http_402": http_402,
            "failed": failed,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_readability_output_as_full() {
        let candidate = ContentProbeCandidate {
            id: Uuid::now_v7(),
            url: "https://example.com/article".to_string(),
            rss_body_chars: Some(20),
        };
        let extracted = Extracted {
            content: format!("<p>{}</p>", "article ".repeat(100)),
            excerpt: None,
            title: None,
            extractor: "readability",
            verified_html: false,
        };
        assert_eq!(
            extracted_sample(&candidate, extracted).status,
            catalog::ContentProbeStatus::Full
        );
    }
}
