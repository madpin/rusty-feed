//! EmbedArticleAgent: compute and store an article's embedding once, using the
//! instance's single configured embedder.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use rustss_db::{articles, cost_ledger, embeddings};
use rustss_embed::{prepare_text, Embedder, HashEmbedder};

use crate::{Agent, AgentContext};

#[derive(Deserialize)]
struct Payload {
    article_id: Uuid,
    #[serde(default)]
    content_revision: Option<i64>,
}

pub struct EmbedArticleAgent {
    embedder: Arc<dyn Embedder>,
}

impl EmbedArticleAgent {
    pub fn new(embedder: Arc<dyn Embedder>) -> Self {
        Self { embedder }
    }
}

impl Default for EmbedArticleAgent {
    fn default() -> Self {
        Self::new(Arc::new(HashEmbedder::default()))
    }
}

#[async_trait]
impl Agent for EmbedArticleAgent {
    fn name(&self) -> &'static str {
        "embed_article"
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
        if content_revision != article.content_revision {
            return Ok(json!({ "skipped": "stale content revision" }));
        }
        if embeddings::exists(pool, article_id, self.embedder.model(), content_revision).await? {
            return Ok(json!({ "skipped": "already embedded" }));
        }

        let text = prepare_text(&article.title, article.excerpt.as_deref(), &article.content);
        let vector = self.embedder.embed_one(&text).await?;
        embeddings::upsert(
            pool,
            article_id,
            &vector,
            self.embedder.model(),
            content_revision,
        )
        .await?;

        // Meter shared article embeddings (user_id = NULL): free of API cost for
        // the local provider but not of CPU, so budgets/dashboards still see them
        // (FND-187). A per-user cap is applied to interactive query embeddings.
        let units = text.split_whitespace().count() as i64;
        if let Err(error) = cost_ledger::record(
            pool,
            None,
            self.embedder.provider(),
            self.embedder.model(),
            "embedding",
            units,
            0,
            0,
            false,
        )
        .await
        {
            tracing::warn!(%article_id, error = %error, "failed to record embedding usage");
        }

        Ok(json!({ "status": "embedded", "model": self.embedder.model() }))
    }
}
