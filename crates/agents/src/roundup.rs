//! RoundupAgent: generate one shared feed/folder Roundup from frozen article
//! IDs on the interactive lane. Provider work never runs in the web handler.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use rusty_feed_core::roundup::{RoundupKey, RoundupPeriod, ROUNDUP_MIN_ARTICLES};
use rusty_feed_db::roundups;
use rusty_feed_llm::{LlmClient, ProviderCascade};

use crate::{Agent, AgentContext};

#[derive(Deserialize)]
struct Payload {
    input_hash: String,
    period: String,
    model_version: String,
    article_ids: Vec<Uuid>,
    user_id: Uuid,
}

pub struct RoundupAgent {
    llm: Option<Arc<dyn LlmClient>>,
    cascade: Option<Arc<ProviderCascade>>,
}

impl RoundupAgent {
    pub fn new(llm: Option<Arc<dyn LlmClient>>) -> Self {
        Self { llm, cascade: None }
    }

    pub fn with_cascade(mut self, cascade: Option<Arc<ProviderCascade>>) -> Self {
        self.cascade = cascade;
        self
    }
}

impl Default for RoundupAgent {
    fn default() -> Self {
        Self::new(None)
    }
}

#[async_trait]
impl Agent for RoundupAgent {
    fn name(&self) -> &'static str {
        "roundup"
    }

    async fn run(&self, ctx: &AgentContext, payload: Value) -> anyhow::Result<Value> {
        if self.llm.is_none() && self.cascade.is_none() {
            return Ok(json!({ "skipped": "no LLM configured" }));
        }
        let payload: Payload = serde_json::from_value(payload)?;
        let Some(period) = RoundupPeriod::parse(&payload.period) else {
            anyhow::bail!("invalid Roundup period");
        };
        let key = RoundupKey {
            input_hash: payload.input_hash,
            period,
            model_version: payload.model_version,
        };
        if roundups::find_by_key(ctx.db.pool(), &key).await?.is_some() {
            return Ok(json!({ "written": false, "shared": true }));
        }

        let loaded = roundups::candidates_by_ids(ctx.db.pool(), &payload.article_ids).await?;
        let by_id: HashMap<_, _> = loaded
            .into_iter()
            .map(|candidate| (candidate.id, candidate))
            .collect();
        let inputs: Vec<_> = payload
            .article_ids
            .iter()
            .filter_map(|id| by_id.get(id).cloned())
            .collect();
        if inputs.len() != payload.article_ids.len() || inputs.len() < ROUNDUP_MIN_ARTICLES {
            return Ok(json!({ "skipped": "input articles changed or unavailable" }));
        }

        if let Some(cascade) = &self.cascade {
            let (written, model) = match rusty_feed_llm::generate_roundup_with_cascade(
                cascade,
                &ctx.ai_budget,
                Some(payload.user_id),
                &key,
                &inputs,
            )
            .await
            {
                Ok(result) => result,
                Err(rusty_feed_llm::RoundupError::Budget(reason)) => {
                    tracing::info!(reason, "Roundup skipped: over AI budget");
                    return Ok(json!({ "skipped": "over AI budget", "reason": reason }));
                }
                Err(rusty_feed_llm::RoundupError::NoProvider) => {
                    return Ok(json!({ "skipped": "no LLM configured" }));
                }
                Err(other) => return Err(anyhow::Error::new(other)),
            };
            return Ok(json!({ "written": written, "model": model }));
        }

        let Some(llm) = self.llm.as_ref() else {
            return Ok(json!({ "skipped": "no LLM configured" }));
        };
        let written = rusty_feed_llm::generate_roundup(
            &ctx.db,
            llm.as_ref(),
            Some(payload.user_id),
            &key,
            &inputs,
            false,
        )
        .await?;
        Ok(json!({ "written": written, "model": llm.model() }))
    }
}
