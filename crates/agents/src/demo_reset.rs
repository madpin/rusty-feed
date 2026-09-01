//! DemoResetAgent: wipe every ephemeral demo account on a schedule
//! (docs/PUBLIC_LAUNCH_PLAN.md §7.2). Deletion is scoped strictly to
//! `is_demo = true`, so real users and shared seed content are never touched.
//! A no-op unless demo mode is enabled.

use async_trait::async_trait;
use serde_json::{json, Value};

use rusty_feed_db::users;

use crate::{Agent, AgentContext};

pub struct DemoResetAgent {
    demo_mode: bool,
}

impl DemoResetAgent {
    pub fn new(demo_mode: bool) -> Self {
        Self { demo_mode }
    }
}

impl Default for DemoResetAgent {
    fn default() -> Self {
        Self::new(false)
    }
}

#[async_trait]
impl Agent for DemoResetAgent {
    fn name(&self) -> &'static str {
        "demo_reset"
    }

    async fn run(&self, ctx: &AgentContext, _payload: Value) -> anyhow::Result<Value> {
        if !self.demo_mode {
            return Ok(json!({ "skipped": "demo mode disabled" }));
        }
        let deleted = users::delete_all_demo(ctx.db.pool()).await?;
        tracing::info!(deleted_demo_users = deleted, "demo reset complete");
        Ok(json!({ "deleted_demo_users": deleted }))
    }
}
