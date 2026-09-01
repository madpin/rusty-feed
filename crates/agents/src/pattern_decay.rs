//! PatternDecayAgent: scheduled maintenance of learned patterns — decay old
//! weights, prune tiny ones, and cap each user at the top 100 (ARCH §11.2).

use async_trait::async_trait;
use serde_json::{json, Value};

use rusty_feed_db::patterns;

use crate::{Agent, AgentContext};

/// Prune threshold and per-user cap (per spec).
const PRUNE_BELOW: f64 = 0.1;
const TOP_N: i64 = 100;

pub struct PatternDecayAgent;

#[async_trait]
impl Agent for PatternDecayAgent {
    fn name(&self) -> &'static str {
        "pattern_decay"
    }

    async fn run(&self, ctx: &AgentContext, _payload: Value) -> anyhow::Result<Value> {
        let pool = ctx.db.pool();
        let decayed = patterns::decay_all(pool).await?;
        let pruned = patterns::prune_small(pool, PRUNE_BELOW).await?;
        let trimmed = patterns::enforce_top_n(pool, TOP_N).await?;
        Ok(json!({ "decayed": decayed, "pruned": pruned, "trimmed": trimmed }))
    }
}
