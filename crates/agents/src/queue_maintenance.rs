//! QueueMaintenanceAgent: bounded retention purge of finished queue jobs,
//! dispatched outbox events, and finished agent run records (FND-238).
//! Dead-letter jobs, unprocessed events, and running rows are never deleted.

use async_trait::async_trait;
use serde_json::json;
use time::Duration;
use uuid::Uuid;

use rusty_feed_db::{agent_locks, agent_runs, jobs, outbox};

use crate::{Agent, AgentContext};

/// Default retention for finished jobs and dispatched outbox events.
pub const DEFAULT_JOB_RETENTION_DAYS: u32 = 7;
/// Default retention for finished agent run records.
pub const DEFAULT_AGENT_RUN_RETENTION_DAYS: u32 = 30;

pub struct QueueMaintenanceAgent {
    job_retention_days: u32,
    agent_run_retention_days: u32,
}

impl QueueMaintenanceAgent {
    /// Zero disables the purge of the matching table(s).
    pub fn new(job_retention_days: u32, agent_run_retention_days: u32) -> Self {
        Self {
            job_retention_days,
            agent_run_retention_days,
        }
    }
}

impl Default for QueueMaintenanceAgent {
    fn default() -> Self {
        Self::new(DEFAULT_JOB_RETENTION_DAYS, DEFAULT_AGENT_RUN_RETENTION_DAYS)
    }
}

#[async_trait]
impl Agent for QueueMaintenanceAgent {
    fn name(&self) -> &'static str {
        "queue_maintenance"
    }

    async fn run(
        &self,
        ctx: &AgentContext,
        _payload: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let pool = ctx.db.pool();
        let holder = Uuid::now_v7().to_string();
        if !agent_locks::acquire(pool, "queue_maintenance", &holder, Duration::minutes(30)).await? {
            return Ok(json!({ "skipped": "another instance holds the lease" }));
        }
        let purged = async {
            let jobs = jobs::purge_done(pool, self.job_retention_days).await?;
            let outbox = outbox::purge_processed(pool, self.job_retention_days).await?;
            let runs = agent_runs::purge_finished(pool, self.agent_run_retention_days).await?;
            anyhow::Ok(json!({
                "purged_done_jobs": jobs,
                "purged_processed_outbox_events": outbox,
                "purged_finished_agent_runs": runs
            }))
        }
        .await;
        agent_locks::release(pool, "queue_maintenance", &holder).await?;
        purged
    }
}
