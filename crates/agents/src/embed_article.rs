//! EmbedArticleAgent: compute and store an article's embedding once, using the
//! instance's single configured embedder.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use rustss_db::{articles, embeddings};
use rustss_embed::{prepare_text, Embedder, HashEmbedder};

use crate::{Agent, AgentContext};

#[derive(Deserialize)]
struct Payload {
    article_id: Uuid,
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
        let Payload { article_id } = serde_json::from_value(payload)?;
        let pool = ctx.db.pool();

        let Some(article) = articles::find_by_id(pool, article_id).await? else {
            return Ok(json!({ "skipped": "article gone" }));
        };
        // Once per shared article.
        if embeddings::exists(pool, article_id).await? {
            return Ok(json!({ "skipped": "already embedded" }));
        }

        let text = prepare_text(&article.title, article.excerpt.as_deref(), &article.content);
        let vector = self.embedder.embed_one(&text).await?;
        embeddings::upsert(pool, article_id, &vector, self.embedder.model()).await?;

        Ok(json!({ "status": "embedded", "model": self.embedder.model() }))
    }
}
