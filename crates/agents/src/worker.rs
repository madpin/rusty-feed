//! The AGENTS runtime: a periodic scheduler plus a pool of durable-queue
//! consumers with retry/backoff and per-run records.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use time::Duration as TimeDuration;
use tokio::sync::watch;
use tokio::time::Duration;

use rustss_db::{agent_runs, jobs, Db, Job};
use rustss_embed::Embedder;
use rustss_feeds::SafeFetchClient;

use crate::{
    Agent, AgentContext, EmbedArticleAgent, ExtractContentAgent, FeedRefreshAgent, FetchFeedAgent,
};

/// Maps job `kind` → agent.
pub struct Registry {
    agents: HashMap<String, Arc<dyn Agent>>,
}

impl Registry {
    /// All built-in agents.
    pub fn with_defaults() -> Self {
        let mut registry = Self {
            agents: HashMap::new(),
        };
        let list: [Arc<dyn Agent>; 4] = [
            Arc::new(FeedRefreshAgent),
            Arc::new(FetchFeedAgent),
            Arc::new(ExtractContentAgent::default()),
            Arc::new(EmbedArticleAgent::default()),
        ];
        for agent in list {
            registry.register(agent.name().to_string(), agent);
        }
        registry
    }

    /// Register (or override) an agent for a job kind.
    pub fn register(&mut self, kind: impl Into<String>, agent: Arc<dyn Agent>) {
        self.agents.insert(kind.into(), agent);
    }

    pub fn get(&self, kind: &str) -> Option<Arc<dyn Agent>> {
        self.agents.get(kind).cloned()
    }

    pub fn kinds(&self) -> Vec<String> {
        self.agents.keys().cloned().collect()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Runtime tuning.
#[derive(Clone)]
pub struct WorkerConfig {
    pub worker_id: String,
    pub concurrency: usize,
    pub poll_interval: Duration,
    pub scheduler_interval: Duration,
    /// Enqueue embedding jobs for new/extracted articles.
    pub embed_on_ingest: bool,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: format!("worker-{}", uuid::Uuid::now_v7()),
            concurrency: 4,
            poll_interval: Duration::from_secs(2),
            scheduler_interval: Duration::from_secs(60),
            embed_on_ingest: false,
        }
    }
}

/// The background worker: a scheduler + N queue consumers.
pub struct Worker {
    ctx: Arc<AgentContext>,
    registry: Arc<Registry>,
    config: WorkerConfig,
}

impl Worker {
    pub fn new(
        db: Db,
        fetch: SafeFetchClient,
        embedder: Arc<dyn Embedder>,
        config: WorkerConfig,
    ) -> Self {
        let mut registry = Registry::with_defaults();
        registry.register("embed_article", Arc::new(EmbedArticleAgent::new(embedder)));
        let ctx = AgentContext::new(db, fetch).with_embed_on_ingest(config.embed_on_ingest);
        Self {
            ctx: Arc::new(ctx),
            registry: Arc::new(registry),
            config,
        }
    }

    pub fn context(&self) -> &AgentContext {
        &self.ctx
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Run until `shutdown` becomes `true`. Spawns the scheduler and
    /// `concurrency` consumers, then awaits them.
    pub async fn run(&self, shutdown: watch::Receiver<bool>) {
        let kinds = self.registry.kinds();
        let mut handles = Vec::new();

        // Reclaim jobs a previous (crashed) worker left `running`.
        match jobs::reclaim_stale(self.ctx.db.pool(), TimeDuration::minutes(15)).await {
            Ok(n) if n > 0 => tracing::info!(reclaimed = n, "reclaimed stale jobs"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "reclaim_stale failed"),
        }

        // Scheduler: periodically enqueue a deduped feed_refresh job.
        {
            let ctx = self.ctx.clone();
            let interval = self.config.scheduler_interval;
            let mut sd = shutdown.clone();
            handles.push(tokio::spawn(async move {
                loop {
                    if let Err(e) = enqueue_feed_refresh(&ctx.db).await {
                        tracing::warn!(error = %e, "scheduler: could not enqueue feed_refresh");
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(interval) => {}
                        _ = sd.changed() => {}
                    }
                    if *sd.borrow() {
                        break;
                    }
                }
            }));
        }

        // Consumers.
        for i in 0..self.config.concurrency {
            let ctx = self.ctx.clone();
            let registry = self.registry.clone();
            let worker_id = format!("{}#{i}", self.config.worker_id);
            let poll = self.config.poll_interval;
            let kinds = kinds.clone();
            let mut sd = shutdown.clone();
            handles.push(tokio::spawn(async move {
                loop {
                    if *sd.borrow() {
                        break;
                    }
                    match jobs::claim_next(ctx.db.pool(), &worker_id, &kinds).await {
                        Ok(Some(job)) => process(&ctx, &registry, job).await,
                        Ok(None) => {
                            tokio::select! {
                                _ = tokio::time::sleep(poll) => {}
                                _ = sd.changed() => {}
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "claim_next failed");
                            tokio::select! {
                                _ = tokio::time::sleep(poll) => {}
                                _ = sd.changed() => {}
                            }
                        }
                    }
                }
            }));
        }

        for handle in handles {
            let _ = handle.await;
        }
    }
}

/// Enqueue a single (deduped) `embed_article` job.
pub(crate) async fn enqueue_embed(db: &Db, article_id: uuid::Uuid) -> anyhow::Result<()> {
    let dedup = format!("embed:{article_id}");
    jobs::enqueue(
        db.pool(),
        "embed_article",
        serde_json::json!({ "article_id": article_id }),
        Some(&dedup),
        None,
        3,
    )
    .await?;
    Ok(())
}

/// Enqueue a single (deduped) `feed_refresh` job. Public so a freshly-started
/// worker can kick one immediately.
pub async fn enqueue_feed_refresh(db: &Db) -> anyhow::Result<()> {
    jobs::enqueue(
        db.pool(),
        "feed_refresh",
        serde_json::json!({}),
        Some("feed_refresh"),
        None,
        3,
    )
    .await?;
    Ok(())
}

/// Run one job through its agent and record the outcome.
pub async fn process(ctx: &AgentContext, registry: &Registry, job: Job) {
    let pool = ctx.db.pool();

    let Some(agent) = registry.get(&job.kind) else {
        tracing::warn!(kind = %job.kind, "no agent registered for kind; dropping job");
        let _ = jobs::complete(pool, job.id).await;
        return;
    };

    let run_id = agent_runs::start(pool, &job.kind, "scheduler").await.ok();
    let started = Instant::now();
    let result = agent.run(ctx, job.payload.clone()).await;
    let duration_ms = started.elapsed().as_millis() as i64;

    match result {
        Ok(stats) => {
            let _ = jobs::complete(pool, job.id).await;
            if let Some(rid) = run_id {
                let _ =
                    agent_runs::finish(pool, rid, "success", duration_ms, Some(stats), None).await;
            }
        }
        Err(e) => {
            let msg = e.to_string();
            let outcome = jobs::fail(pool, &job, &msg, backoff_for(job.attempts)).await;
            tracing::warn!(kind = %job.kind, attempts = job.attempts, ?outcome, error = %msg, "job failed");
            if let Some(rid) = run_id {
                let _ =
                    agent_runs::finish(pool, rid, "failed", duration_ms, None, Some(&msg)).await;
            }
        }
    }
}

/// Exponential backoff: 60s, 120s, 240s, … capped at 1h.
fn backoff_for(attempts: i32) -> TimeDuration {
    let exp = attempts.saturating_sub(1).clamp(0, 16) as u32;
    let secs = 60u64.saturating_mul(2u64.saturating_pow(exp)).min(3600);
    TimeDuration::seconds(secs as i64)
}
