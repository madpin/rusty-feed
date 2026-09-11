//! The AGENTS runtime: a periodic scheduler plus a pool of durable-queue
//! consumers with retry/backoff and per-run records.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use time::Duration as TimeDuration;
use tokio::sync::watch;
use tokio::time::Duration;

use rusty_feed_core::{
    cron::CronSchedule, tracking_pixel::TrackingPixelPolicy, tracking_url::TrackingUrlPolicy,
};
use rusty_feed_db::{agent_runs, jobs, outbox, Db, Job};
use rusty_feed_embed::Embedder;
use rusty_feed_extract::ExtractionService;
use rusty_feed_feeds::SafeFetchClient;
use rusty_feed_llm::LlmClient;

use crate::{
    Agent, AgentContext, CatalogPreviewAgent, ContentCleanupAgent, DemoResetAgent,
    EmbedArticleAgent, ExtractContentAgent, FeedRefreshAgent, FetchFeedAgent,
    IntegrationDeliveryAgent, PatternDecayAgent, SavedSearchMatchAgent, SavedSearchScanAgent,
    SummarizeArticleAgent, SummaryBackfillAgent, YouTubeDurationAgent,
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
        let list: [Arc<dyn Agent>; 13] = [
            Arc::new(FeedRefreshAgent),
            Arc::new(FetchFeedAgent),
            Arc::new(IntegrationDeliveryAgent),
            Arc::new(CatalogPreviewAgent::default()),
            Arc::new(ContentCleanupAgent::default()),
            Arc::new(ExtractContentAgent::default()),
            Arc::new(EmbedArticleAgent::default()),
            Arc::new(PatternDecayAgent),
            Arc::new(SavedSearchScanAgent),
            Arc::new(SavedSearchMatchAgent::default()),
            Arc::new(SummarizeArticleAgent::default()),
            Arc::new(SummaryBackfillAgent),
            Arc::new(YouTubeDurationAgent),
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
    /// Reserved slots for user-triggered, latency-sensitive work.
    pub interactive_concurrency: usize,
    pub poll_interval: Duration,
    pub interactive_poll_interval: Duration,
    pub scheduler_interval: Duration,
    pub scheduler_enabled: bool,
    /// Cron expression controlling the shared feed due-scan.
    pub feed_refresh_schedule: CronSchedule,
    /// Cron expression controlling shared content cleanup.
    pub cleanup_schedule: CronSchedule,
    /// Shared extraction policy, including optional bounded Chromium fallback.
    pub extraction_config: rusty_feed_extract::ExtractConfig,
    /// Enqueue embedding jobs for new/extracted articles.
    pub embed_on_ingest: bool,
    /// Master switch for every embedding use case (`EMBEDDINGS_ENABLED`).
    /// `false` keeps `saved_search_match` keyword-only and never calls the
    /// (disabled) embedder.
    pub embeddings_enabled: bool,
    pub auto_summarize: bool,
    pub auto_summarize_min_chars: u32,
    /// Proactively backfill public-page extraction on feed ingest. When false,
    /// pages are extracted lazily on first reader access via the interactive lane.
    pub eager_extraction: bool,
    /// Shared URL privacy policy used by feed and content processing agents.
    pub tracking_url_policy: TrackingUrlPolicy,
    /// Shared tracking-image privacy policy.
    pub tracking_pixel_policy: TrackingPixelPolicy,
    /// Zero disables article-row cleanup; public-page lifetimes remain active.
    pub article_retention_days: u32,
    pub retention_age_basis: rusty_feed_core::config::RetentionAgeBasis,
    pub integration_credentials: rusty_feed_core::crypto::Cryptor,
    pub acquisition_credentials: rusty_feed_core::crypto::Cryptor,
    pub youtube_duration_enabled: bool,
    pub youtube_duration_daily_budget: u32,
    pub youtube_duration_cache_days: i64,
    /// Immutable AI budget enforced before background LLM spend (FND-187).
    pub ai_budget: rusty_feed_core::ai_budget::AiBudget,
    /// Cap on articles ingested per feed refresh; `0` == unlimited (FND-189).
    pub feed_max_items_per_refresh: usize,
    /// Whether the demo sandbox is enabled (drives the daily reset, §7).
    pub demo_mode: bool,
    /// UTC cron for the daily demo wipe.
    pub demo_reset_schedule: CronSchedule,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: format!("worker-{}", uuid::Uuid::now_v7()),
            concurrency: 4,
            interactive_concurrency: 1,
            poll_interval: Duration::from_secs(2),
            interactive_poll_interval: Duration::from_millis(150),
            scheduler_interval: Duration::from_secs(15),
            scheduler_enabled: true,
            feed_refresh_schedule: CronSchedule::default(),
            cleanup_schedule: CronSchedule::parse("0 3 * * *")
                .unwrap_or_else(|_| unreachable!("static cron is valid")),
            extraction_config: rusty_feed_extract::ExtractConfig::default(),
            embed_on_ingest: false,
            embeddings_enabled: true,
            auto_summarize: false,
            auto_summarize_min_chars: rusty_feed_db::summary_policy::DEFAULT_MIN_CONTENT_CHARS,
            eager_extraction: true,
            tracking_url_policy: TrackingUrlPolicy::default(),
            tracking_pixel_policy: TrackingPixelPolicy::default(),
            article_retention_days: 0,
            retention_age_basis: rusty_feed_core::config::RetentionAgeBasis::Published,
            integration_credentials: rusty_feed_core::crypto::Cryptor::new(
                b"rusty-feed-worker-default-test-key-material",
            ),
            acquisition_credentials: rusty_feed_core::crypto::Cryptor::new_with_context(
                b"rusty-feed-worker-default-acquisition-key",
                b"rusty-feed/acquisition-profile-secrets/v1",
            ),
            youtube_duration_enabled: false,
            youtube_duration_daily_budget: 100,
            youtube_duration_cache_days: 30,
            ai_budget: rusty_feed_core::ai_budget::AiBudget::default(),
            feed_max_items_per_refresh: 0,
            demo_mode: false,
            demo_reset_schedule: CronSchedule::parse("0 4 * * *")
                .unwrap_or_else(|_| unreachable!("static cron is valid")),
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
        llm: Option<Arc<dyn LlmClient>>,
        llm_cascade: Option<Arc<rusty_feed_llm::ProviderCascade>>,
        config: WorkerConfig,
    ) -> Self {
        let summarization_available = llm.is_some() || llm_cascade.is_some();
        let mut registry = Registry::with_defaults();
        registry.register(
            "embed_article",
            Arc::new(EmbedArticleAgent::new(embedder.clone())),
        );
        registry.register(
            "saved_search_match",
            Arc::new(SavedSearchMatchAgent::new(
                embedder,
                config.embeddings_enabled,
            )),
        );
        registry.register(
            "summarize_article",
            Arc::new(SummarizeArticleAgent::new(llm).with_cascade(llm_cascade)),
        );
        registry.register(
            "extract_content",
            Arc::new(ExtractContentAgent::new(Arc::new(ExtractionService::new(
                config.extraction_config.clone(),
            )))),
        );
        registry.register(
            "content_cleanup",
            Arc::new(ContentCleanupAgent::new(
                config.article_retention_days,
                config.retention_age_basis,
            )),
        );
        registry.register(
            "demo_reset",
            Arc::new(DemoResetAgent::new(config.demo_mode)),
        );
        let ctx = AgentContext::new(db, fetch)
            .with_embed_on_ingest(config.embed_on_ingest)
            .with_auto_summarize(config.auto_summarize && summarization_available)
            .with_auto_summarize_min_chars(config.auto_summarize_min_chars)
            .with_eager_extraction(config.eager_extraction)
            .with_tracking_url_policy(config.tracking_url_policy.clone())
            .with_tracking_pixel_policy(config.tracking_pixel_policy.clone());
        let ctx = ctx.with_integration_credentials(config.integration_credentials.clone());
        let ctx = ctx.with_acquisition_credentials(config.acquisition_credentials.clone());
        let ctx = ctx.with_youtube_duration_policy(
            config.youtube_duration_enabled,
            config.youtube_duration_daily_budget,
            config.youtube_duration_cache_days,
        );
        let ctx = ctx
            .with_ai_budget(config.ai_budget)
            .with_feed_max_items_per_refresh(config.feed_max_items_per_refresh);
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

        if let Err(error) = jobs::register_worker(
            self.ctx.db.pool(),
            &self.config.worker_id,
            self.config.concurrency,
            self.config.interactive_concurrency,
            self.config.scheduler_enabled,
        )
        .await
        {
            tracing::warn!(error = %error, "could not register worker heartbeat");
        }

        let ctx = self.ctx.clone();
        let worker_id = self.config.worker_id.clone();
        let mut heartbeat_shutdown = shutdown.clone();
        handles.push(tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {
                        if let Err(error) = jobs::heartbeat_worker(ctx.db.pool(), &worker_id).await {
                            tracing::warn!(error = %error, "worker heartbeat failed");
                        }
                    }
                    _ = heartbeat_shutdown.changed() => {}
                }
                if *heartbeat_shutdown.borrow() {
                    break;
                }
            }
        }));

        // Reclaim jobs a previous (crashed) worker left `running`.
        match jobs::reclaim_stale(self.ctx.db.pool(), TimeDuration::minutes(15)).await {
            Ok(n) if n > 0 => tracing::info!(reclaimed = n, "reclaimed stale jobs"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "reclaim_stale failed"),
        }

        // Outbox dispatch is independent of cron scheduling: manually-created
        // transactional events must still flow when scheduled jobs are off.
        {
            let ctx = self.ctx.clone();
            let allowed_kinds = kinds.clone();
            let mut sd = shutdown.clone();
            handles.push(tokio::spawn(async move {
                loop {
                    match outbox::dispatch(ctx.db.pool(), &allowed_kinds, 250).await {
                        Ok(count) if count > 0 => {
                            tracing::debug!(count, "dispatched transactional outbox events")
                        }
                        Ok(_) => {}
                        Err(error) => tracing::warn!(%error, "outbox dispatch failed"),
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                        _ = sd.changed() => {}
                    }
                    if *sd.borrow() {
                        break;
                    }
                }
            }));
        }

        // Scheduler: evaluate validated UTC cron expressions once per minute.
        if self.config.scheduler_enabled {
            let ctx = self.ctx.clone();
            let interval = self.config.scheduler_interval;
            let feed_refresh_schedule = self.config.feed_refresh_schedule.clone();
            let cleanup_schedule = self.config.cleanup_schedule.clone();
            let demo_reset_schedule = self.config.demo_reset_schedule.clone();
            let demo_mode = self.config.demo_mode;
            let mut sd = shutdown.clone();
            handles.push(tokio::spawn(async move {
                let mut evaluated_minute = None;
                loop {
                    let now = time::OffsetDateTime::now_utc();
                    let minute = now.unix_timestamp() / 60;
                    if evaluated_minute != Some(minute) {
                        evaluated_minute = Some(minute);
                        if feed_refresh_schedule.matches(now) {
                            if let Err(e) = enqueue_feed_refresh(&ctx.db).await {
                                tracing::warn!(error = %e, "scheduler: could not enqueue feed_refresh");
                            }
                            if let Err(e) = enqueue_singleton(&ctx.db, "saved_search_scan").await {
                                tracing::warn!(error = %e, "scheduler: could not enqueue saved_search_scan");
                            }
                        }
                        if cleanup_schedule.matches(now) {
                            if let Err(e) = enqueue_singleton(&ctx.db, "pattern_decay").await {
                                tracing::warn!(error = %e, "scheduler: could not enqueue pattern_decay");
                            }
                            if let Err(e) = enqueue_singleton(&ctx.db, "content_cleanup").await {
                                tracing::warn!(error = %e, "scheduler: could not enqueue content_cleanup");
                            }
                        }
                        if demo_mode && demo_reset_schedule.matches(now) {
                            if let Err(e) = enqueue_singleton(&ctx.db, "demo_reset").await {
                                tracing::warn!(error = %e, "scheduler: could not enqueue demo_reset");
                            }
                        }
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

        // Background consumers. They never claim interactive work, preserving
        // capacity for actions where a user is actively waiting.
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
                    match jobs::claim_next_in_lane(
                        ctx.db.pool(),
                        &worker_id,
                        jobs::QueueLane::Background,
                        &kinds,
                    )
                    .await
                    {
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

        // Interactive consumers are an isolated low-latency lane. Feed
        // downloads and bulk jobs cannot occupy these slots.
        for i in 0..self.config.interactive_concurrency {
            let ctx = self.ctx.clone();
            let registry = self.registry.clone();
            let worker_id = format!("{}#interactive-{i}", self.config.worker_id);
            let poll = self.config.interactive_poll_interval;
            let kinds = kinds.clone();
            let mut sd = shutdown.clone();
            handles.push(tokio::spawn(async move {
                loop {
                    if *sd.borrow() {
                        break;
                    }
                    match jobs::claim_next_in_lane(
                        ctx.db.pool(),
                        &worker_id,
                        jobs::QueueLane::Interactive,
                        &kinds,
                    )
                    .await
                    {
                        Ok(Some(job)) => process(&ctx, &registry, job).await,
                        Ok(None) => {
                            tokio::select! {
                                _ = tokio::time::sleep(poll) => {}
                                _ = sd.changed() => {}
                            }
                        }
                        Err(error) => {
                            tracing::warn!(error = %error, "interactive claim failed");
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
        if let Err(error) =
            jobs::unregister_worker(self.ctx.db.pool(), &self.config.worker_id).await
        {
            tracing::warn!(error = %error, "could not remove worker heartbeat");
        }
    }
}

/// Enqueue a single (deduped) `embed_article` job.
pub(crate) async fn enqueue_embed(
    db: &Db,
    article_id: uuid::Uuid,
    content_revision: i64,
) -> anyhow::Result<bool> {
    let dedup = format!("embed:{article_id}:{content_revision}");
    Ok(jobs::enqueue(
        db.pool(),
        "embed_article",
        serde_json::json!({ "article_id": article_id, "content_revision": content_revision }),
        Some(&dedup),
        None,
        3,
    )
    .await?
    .is_some())
}

/// Queue automatic shared summarization in the background lane. On-demand
/// requests use the same revision key in the reserved interactive lane.
pub(crate) async fn enqueue_summary(
    db: &Db,
    article_id: uuid::Uuid,
    content_revision: i64,
) -> anyhow::Result<bool> {
    let dedup = format!("summarize:{article_id}:{content_revision}");
    Ok(jobs::enqueue(
        db.pool(),
        "summarize_article",
        serde_json::json!({
            "article_id": article_id,
            "content_revision": content_revision,
            "user_id": null,
        }),
        Some(&dedup),
        None,
        3,
    )
    .await?
    .is_some())
}

pub(crate) async fn enqueue_automatic_summary(
    ctx: &AgentContext,
    article_id: uuid::Uuid,
    content_revision: i64,
) -> anyhow::Result<bool> {
    let policy = ctx.summary_policy().await?;
    if !policy.enabled {
        return Ok(false);
    }
    let Some(article) = rusty_feed_db::articles::find_by_id(ctx.db.pool(), article_id).await?
    else {
        return Ok(false);
    };
    if article.content_revision != content_revision
        || !rusty_feed_db::articles::meets_summary_minimum(
            &article.content,
            policy.min_content_chars,
        )
    {
        return Ok(false);
    }
    enqueue_summary(&ctx.db, article_id, content_revision).await
}

/// Queue a bounded automatic-summary scan for one feed or the whole instance.
pub async fn enqueue_summary_backfill(
    db: &Db,
    feed_id: Option<uuid::Uuid>,
    ready_only: bool,
    after: Option<uuid::Uuid>,
) -> anyhow::Result<bool> {
    let scope = feed_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "all".to_string());
    let cursor = after
        .map(|id| id.to_string())
        .unwrap_or_else(|| "start".to_string());
    let dedup = format!("summary-backfill:{scope}:{ready_only}:{cursor}");
    Ok(jobs::enqueue(
        db.pool(),
        "summary_backfill",
        serde_json::json!({
            "feed_id": feed_id,
            "ready_only": ready_only,
            "after": after,
        }),
        Some(&dedup),
        None,
        3,
    )
    .await?
    .is_some())
}

/// Enqueue a single (deduped) `feed_refresh` job. Public so a freshly-started
/// worker can kick one immediately.
pub async fn enqueue_feed_refresh(db: &Db) -> anyhow::Result<()> {
    enqueue_singleton(db, "feed_refresh").await
}

/// Enqueue a deduped, payload-less singleton job of the given kind.
async fn enqueue_singleton(db: &Db, kind: &str) -> anyhow::Result<()> {
    jobs::enqueue(db.pool(), kind, serde_json::json!({}), Some(kind), None, 3).await?;
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

    let trigger = if job.queue_name == jobs::QueueLane::Interactive.as_str() {
        "interactive"
    } else {
        "scheduler"
    };
    let run_id = agent_runs::start(pool, &job.kind, trigger).await.ok();
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
        Err(error) => {
            let message = safe_error_message(&error);
            let outcome = jobs::fail(
                pool,
                &job,
                &message,
                backoff_for(job.attempts, &job.queue_name),
            )
            .await;
            tracing::warn!(
                kind = %job.kind,
                lane = %job.queue_name,
                attempts = job.attempts,
                error = %error,
                ?outcome,
                "job failed"
            );
            if let Some(rid) = run_id {
                let _ = agent_runs::finish(pool, rid, "failed", duration_ms, None, Some(&message))
                    .await;
            }
        }
    }
}

/// Exponential backoff: 60s, 120s, 240s, … capped at 1h.
fn backoff_for(attempts: i32, queue_name: &str) -> TimeDuration {
    let exp = attempts.saturating_sub(1).clamp(0, 16) as u32;
    let (base, cap) = if queue_name == jobs::QueueLane::Interactive.as_str() {
        (2, 30)
    } else {
        (60, 3600)
    };
    let secs = base * 2u64.saturating_pow(exp).min(cap / base);
    TimeDuration::seconds(secs as i64)
}

/// Produce a bounded operational error suitable for privileged admin display.
/// URL credentials, query strings, fragments, and control characters are
/// removed while preserving useful HTTP/provider context.
fn safe_error_message(error: &anyhow::Error) -> String {
    let raw = format!("{error:#}");
    let normalized: String = raw
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let mut output = String::with_capacity(normalized.len().min(800));
    for (index, token) in normalized.split_whitespace().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        output.push_str(&redact_url(token));
        if output.chars().count() >= 800 {
            break;
        }
    }
    let mut bounded: String = output.chars().take(800).collect();
    if output.chars().count() > 800 {
        bounded.push('…');
    }
    if bounded.is_empty() {
        "Agent execution failed".to_string()
    } else {
        bounded
    }
}

fn redact_url(token: &str) -> String {
    let trailing = token
        .chars()
        .rev()
        .take_while(|character| matches!(character, ',' | '.' | ')' | ']' | '}'))
        .count();
    let split_at = token.len().saturating_sub(trailing);
    let (candidate, punctuation) = token.split_at(split_at);
    let Ok(mut url) = url::Url::parse(candidate) else {
        return token.to_string();
    };
    if !matches!(url.scheme(), "http" | "https") {
        return token.to_string();
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    format!("{url}{punctuation}")
}

#[cfg(test)]
mod tests {
    use super::safe_error_message;

    #[test]
    fn admin_error_removes_url_secrets_and_is_bounded() {
        let error = anyhow::anyhow!(
            "provider returned 401 from https://user:pass@example.com/v1/chat?api_key=secret#token"
        );
        let message = safe_error_message(&error);
        assert!(message.contains("provider returned 401"));
        assert!(message.contains("https://example.com/v1/chat"));
        assert!(!message.contains("pass"));
        assert!(!message.contains("secret"));
        assert!(message.chars().count() <= 801);
    }
}
