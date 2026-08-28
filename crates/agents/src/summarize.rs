//! SummarizeArticleAgent: generate a shared article's summary once (via the LLM
//! client), recording cost. No-op when no LLM is configured.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use rustss_db::articles;
use rustss_llm::LlmClient;

use crate::{Agent, AgentContext};

#[derive(Deserialize)]
struct Payload {
    article_id: Uuid,
    #[serde(default)]
    user_id: Option<Uuid>,
}

pub struct SummarizeArticleAgent {
    llm: Option<Arc<dyn LlmClient>>,
}

impl SummarizeArticleAgent {
    pub fn new(llm: Option<Arc<dyn LlmClient>>) -> Self {
        Self { llm }
    }
}

impl Default for SummarizeArticleAgent {
    fn default() -> Self {
        Self::new(None)
    }
}

#[async_trait]
impl Agent for SummarizeArticleAgent {
    fn name(&self) -> &'static str {
        "summarize_article"
    }

    async fn run(&self, ctx: &AgentContext, payload: Value) -> anyhow::Result<Value> {
        let Some(llm) = &self.llm else {
            return Ok(json!({ "skipped": "no LLM configured" }));
        };
        let Payload {
            article_id,
            user_id,
        } = serde_json::from_value(payload)?;

        let Some(article) = articles::find_by_id(ctx.db.pool(), article_id).await? else {
            return Ok(json!({ "skipped": "article gone" }));
        };

        let written =
            rustss_llm::summarize_article(&ctx.db, llm.as_ref(), user_id, &article).await?;
        Ok(json!({ "written": written, "model": llm.model() }))
    }
}
