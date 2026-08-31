//! SummarizeArticleAgent: generate a shared article's summary once (via the LLM
//! client), recording cost. No-op when no LLM is configured.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use rustss_db::{articles, integrations, jobs};
use rustss_llm::{LlmClient, ProviderCascade};

use crate::{Agent, AgentContext};

#[derive(Deserialize)]
struct Payload {
    article_id: Uuid,
    #[serde(default)]
    content_revision: i64,
    #[serde(default)]
    user_id: Option<Uuid>,
}

pub struct SummarizeArticleAgent {
    llm: Option<Arc<dyn LlmClient>>,
    cascade: Option<Arc<ProviderCascade>>,
}

impl SummarizeArticleAgent {
    pub fn new(llm: Option<Arc<dyn LlmClient>>) -> Self {
        Self { llm, cascade: None }
    }

    pub fn with_cascade(mut self, cascade: Option<Arc<ProviderCascade>>) -> Self {
        self.cascade = cascade;
        self
    }
}

impl Default for SummarizeArticleAgent {
    fn default() -> Self {
        Self::new(None)
    }
}

async fn notify_summary_ready(
    ctx: &AgentContext,
    user_id: Option<Uuid>,
    article_id: Uuid,
    written: bool,
) -> anyhow::Result<u64> {
    let Some(user_id) = user_id.filter(|_| written) else {
        return Ok(0);
    };
    let mut enqueued = 0;
    for profile in integrations::profiles_for_event(ctx.db.pool(), user_id, "summary.ready").await?
    {
        let delivery = integrations::queue_delivery(
            ctx.db.pool(),
            &profile,
            Some(article_id),
            "summary.ready",
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
    Ok(enqueued)
}

#[async_trait]
impl Agent for SummarizeArticleAgent {
    fn name(&self) -> &'static str {
        "summarize_article"
    }

    async fn run(&self, ctx: &AgentContext, payload: Value) -> anyhow::Result<Value> {
        if self.llm.is_none() && self.cascade.is_none() {
            return Ok(json!({ "skipped": "no LLM configured" }));
        }
        let Payload {
            article_id,
            content_revision,
            user_id,
        } = serde_json::from_value(payload)?;

        let Some(article) = articles::find_by_id(ctx.db.pool(), article_id).await? else {
            return Ok(json!({ "skipped": "article gone" }));
        };
        if article.content_revision != content_revision {
            return Ok(json!({ "skipped": "stale content revision" }));
        }

        if let Some(cascade) = &self.cascade {
            let (written, model) =
                rustss_llm::summarize_article_with_cascade(cascade, user_id, &article).await?;
            let integration_jobs = notify_summary_ready(ctx, user_id, article_id, written).await?;
            return Ok(
                json!({ "written": written, "model": model, "integration_jobs": integration_jobs }),
            );
        }
        let Some(llm) = self.llm.as_ref() else {
            return Ok(json!({ "skipped": "no LLM configured" }));
        };
        let written =
            rustss_llm::summarize_article(&ctx.db, llm.as_ref(), user_id, &article).await?;
        let integration_jobs = notify_summary_ready(ctx, user_id, article_id, written).await?;
        Ok(
            json!({ "written": written, "model": llm.model(), "integration_jobs": integration_jobs }),
        )
    }
}
