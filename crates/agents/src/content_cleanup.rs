//! ContentCleanupAgent: enforce finite public-page lifetimes while preserving
//! the RSS body stored with every shared article.

use async_trait::async_trait;
use serde_json::json;

use rustss_db::{api_audit, articles, media_cache};

use crate::{Agent, AgentContext};

pub struct ContentCleanupAgent {
    retention_days: u32,
    age_basis: rustss_core::config::RetentionAgeBasis,
}

impl ContentCleanupAgent {
    pub fn new(retention_days: u32, age_basis: rustss_core::config::RetentionAgeBasis) -> Self {
        Self {
            retention_days,
            age_basis,
        }
    }
}

impl Default for ContentCleanupAgent {
    fn default() -> Self {
        Self::new(0, rustss_core::config::RetentionAgeBasis::Published)
    }
}

#[async_trait]
impl Agent for ContentCleanupAgent {
    fn name(&self) -> &'static str {
        "content_cleanup"
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        _payload: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let expired = articles::clear_expired_article_page_content(ctx.db.pool()).await?;
        let expired_media = media_cache::delete_expired(ctx.db.pool()).await?;
        let expired_api_audit = api_audit::delete_older_than(ctx.db.pool(), 90).await?;
        let expired_articles = articles::delete_expired_unbookmarked(
            ctx.db.pool(),
            self.retention_days,
            self.age_basis,
        )
        .await?;
        Ok(json!({
            "expired_public_pages": expired,
            "expired_media_cache_entries": expired_media,
            "expired_api_audit_events": expired_api_audit,
            "expired_unbookmarked_articles": expired_articles
        }))
    }
}
