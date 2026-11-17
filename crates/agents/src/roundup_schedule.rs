//! RoundupScheduleAgent scans user-local schedules, enqueues the same deduped
//! shared RoundupAgent work as HTTP, reconciles personal history, and delivers
//! opted-in email without ever generating content per subscriber.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use rusty_feed_core::reading::absolutize_app_paths;
use rusty_feed_core::roundup::{
    select_candidates, RoundupKey, ROUNDUP_LLM_CALL_WEIGHT, ROUNDUP_MIN_ARTICLES,
};
use rusty_feed_db::{agent_locks, jobs, roundup_runs, roundup_schedules, roundups};
use rusty_feed_llm::ProviderCascade;

use crate::{Agent, AgentContext};

const DUE_SCAN_LIMIT: i64 = 2_000;
const RECONCILE_LIMIT: i64 = 500;
const EMAIL_LIMIT: usize = 50;
const TOO_FEW: &str = "Fewer than 3 articles were available in this window.";
const NO_PROVIDER: &str = "No AI provider is configured for this Roundup.";
const OVER_BUDGET: &str = "Today's AI budget was used up; no generation was started.";
const NOT_GENERATED: &str =
    "The Roundup did not produce a result. The next scheduled run can try again.";
const FAILED: &str = "The Roundup could not be generated. The next scheduled run can try again.";

pub struct RoundupScheduleAgent {
    cascade: Option<Arc<ProviderCascade>>,
    smtp_url: Option<String>,
    smtp_from: Option<String>,
    public_base_url: String,
}

impl RoundupScheduleAgent {
    pub fn new(
        cascade: Option<Arc<ProviderCascade>>,
        smtp_url: Option<String>,
        smtp_from: Option<String>,
        public_base_url: String,
    ) -> Self {
        Self {
            cascade,
            smtp_url,
            smtp_from,
            public_base_url,
        }
    }
}

impl Default for RoundupScheduleAgent {
    fn default() -> Self {
        Self::new(None, None, None, "http://localhost:8080".to_string())
    }
}

#[derive(Default)]
struct Stats {
    considered: u64,
    enqueued: u64,
    ready: u64,
    skipped: u64,
    failed: u64,
    emailed: u64,
}

#[async_trait]
impl Agent for RoundupScheduleAgent {
    fn name(&self) -> &'static str {
        "roundup_schedule"
    }

    async fn run(&self, ctx: &AgentContext, _payload: Value) -> anyhow::Result<Value> {
        let pool = ctx.db.pool();
        let holder = Uuid::now_v7().to_string();
        if !agent_locks::acquire(pool, "roundup_schedule", &holder, Duration::minutes(30)).await? {
            return Ok(json!({ "skipped": "another instance holds the lease" }));
        }
        let outcome = async {
            let mut stats = Stats::default();
            self.reconcile(ctx, &mut stats).await?;
            self.process_due(ctx, &mut stats).await?;
            self.deliver(ctx, &mut stats).await?;
            anyhow::Ok(json!({
                "considered": stats.considered,
                "enqueued": stats.enqueued,
                "ready": stats.ready,
                "skipped": stats.skipped,
                "failed": stats.failed,
                "emailed": stats.emailed,
            }))
        }
        .await;
        agent_locks::release(pool, "roundup_schedule", &holder).await?;
        outcome
    }
}

impl RoundupScheduleAgent {
    async fn reconcile(&self, ctx: &AgentContext, stats: &mut Stats) -> anyhow::Result<()> {
        let now = OffsetDateTime::now_utc();
        for run in roundup_runs::unfinished(ctx.db.pool(), RECONCILE_LIMIT).await? {
            let Some(key) = run.key() else {
                if now - run.created_at >= Duration::minutes(15) {
                    finish(ctx, &run, "failed", None, None, Some(FAILED), stats).await?;
                }
                continue;
            };
            if let Some(stored) = roundups::find_by_key(ctx.db.pool(), &key).await? {
                stats.ready +=
                    roundup_runs::mark_ready_for_key(ctx.db.pool(), run.user_id, &key, stored.id)
                        .await?;
                continue;
            }
            let job = jobs::latest_by_dedup(ctx.db.pool(), &key.dedup_key()).await?;
            match job {
                Some(job) if job.status == "dead" || job.status == "failed" => {
                    finish(ctx, &run, "failed", Some(&key), None, Some(FAILED), stats).await?;
                }
                Some(job) if job.status == "done" => {
                    finish(
                        ctx,
                        &run,
                        "skipped",
                        Some(&key),
                        None,
                        Some(NOT_GENERATED),
                        stats,
                    )
                    .await?;
                }
                None if now - run.created_at >= Duration::minutes(15) => {
                    finish(ctx, &run, "failed", Some(&key), None, Some(FAILED), stats).await?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn process_due(&self, ctx: &AgentContext, stats: &mut Stats) -> anyhow::Result<()> {
        let now = OffsetDateTime::now_utc();
        let candidates =
            roundup_schedules::due_candidates(ctx.db.pool(), now, DUE_SCAN_LIMIT).await?;
        for schedule in candidates {
            let Some(scheduled_for) = schedule.next_run() else {
                continue;
            };
            if scheduled_for > now {
                continue;
            }
            let Some(run) =
                roundup_runs::claim_window(ctx.db.pool(), &schedule, scheduled_for, now).await?
            else {
                continue;
            };
            stats.considered += 1;
            let (Some(scope), Some(period)) = (schedule.scope(), schedule.roundup_period()) else {
                finish(ctx, &run, "failed", None, None, Some(FAILED), stats).await?;
                continue;
            };
            let selected = select_candidates(
                roundups::candidates_for_user(ctx.db.pool(), schedule.user_id, scope, period)
                    .await?,
                period,
                now,
            );
            if selected.len() < ROUNDUP_MIN_ARTICLES {
                finish(ctx, &run, "skipped", None, None, Some(TOO_FEW), stats).await?;
                continue;
            }
            let model_version = match &self.cascade {
                Some(cascade) => cascade.selected_model(Some(schedule.user_id)).await?,
                None => None,
            };
            let Some(model_version) = model_version else {
                finish(ctx, &run, "skipped", None, None, Some(NO_PROVIDER), stats).await?;
                continue;
            };
            let ids: Vec<_> = selected.iter().map(|candidate| candidate.id).collect();
            let key = RoundupKey::new(&ids, period, &model_version);
            if let Some(stored) = roundups::find_by_key(ctx.db.pool(), &key).await? {
                finish(ctx, &run, "ready", Some(&key), Some(stored.id), None, stats).await?;
                continue;
            }
            let active = jobs::latest_by_dedup(ctx.db.pool(), &key.dedup_key())
                .await?
                .is_some_and(|job| matches!(job.status.as_str(), "pending" | "running"));
            if !active
                && !rusty_feed_llm::llm_budget_allows(
                    &ctx.db,
                    &ctx.ai_budget,
                    schedule.user_id,
                    ROUNDUP_LLM_CALL_WEIGHT,
                )
                .await?
            {
                finish(
                    ctx,
                    &run,
                    "skipped",
                    Some(&key),
                    None,
                    Some(OVER_BUDGET),
                    stats,
                )
                .await?;
                continue;
            }
            jobs::enqueue_in_lane(
                ctx.db.pool(),
                jobs::QueueLane::Interactive,
                "roundup",
                json!({
                    "input_hash": key.input_hash,
                    "period": key.period.as_str(),
                    "model_version": key.model_version,
                    "article_ids": ids,
                    "user_id": schedule.user_id,
                }),
                Some(&key.dedup_key()),
                None,
                3,
            )
            .await?;
            finish(ctx, &run, "queued", Some(&key), None, None, stats).await?;
            stats.enqueued += 1;
        }
        Ok(())
    }

    async fn deliver(&self, ctx: &AgentContext, stats: &mut Stats) -> anyhow::Result<()> {
        if self.smtp_url.is_none() || self.smtp_from.is_none() {
            return Ok(());
        }
        for _ in 0..EMAIL_LIMIT {
            let Some(pending) = roundup_runs::claim_pending_email(ctx.db.pool()).await? else {
                break;
            };
            let (Some(key), Some(scope)) = (pending.key(), pending.scope()) else {
                continue;
            };
            let link = public_roundup_url(&self.public_base_url, scope, &key);
            let manage = format!("{}/roundups", self.public_base_url.trim_end_matches('/'));
            let body = absolutize_app_paths(
                &format!(
                    "{}\n\n{}\n\nOpen the Roundup:\n{link}\n\nStop or change these emails:\n{manage}\n",
                    pending.headline,
                    readable_markdown(&pending.body_markdown),
                ),
                &self.public_base_url,
            );
            let subject = format!("Roundup: {}", pending.headline);
            if let Err(error) = crate::email::send_plain_text(
                self.smtp_url.as_deref(),
                self.smtp_from.as_deref(),
                &pending.email,
                &subject,
                &body,
            )
            .await
            {
                tracing::warn!(error = %error, run_id = %pending.id, "Roundup email failed");
                roundup_runs::release_email_claim(ctx.db.pool(), pending.id).await?;
                break;
            }
            stats.emailed += 1;
        }
        Ok(())
    }
}

async fn finish(
    ctx: &AgentContext,
    run: &roundup_runs::RoundupRun,
    status: &str,
    key: Option<&RoundupKey>,
    roundup_id: Option<Uuid>,
    reason: Option<&'static str>,
    stats: &mut Stats,
) -> anyhow::Result<()> {
    if roundup_runs::finish(
        ctx.db.pool(),
        run.user_id,
        run.id,
        status,
        key,
        roundup_id,
        reason,
    )
    .await?
    {
        match status {
            "ready" => stats.ready += 1,
            "skipped" => stats.skipped += 1,
            "failed" => stats.failed += 1,
            _ => {}
        }
    }
    Ok(())
}

fn public_roundup_url(
    public_base_url: &str,
    scope: roundups::RoundupScope,
    key: &RoundupKey,
) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("period", key.period.as_str())
        .append_pair("input_hash", &key.input_hash)
        .append_pair("model_version", &key.model_version)
        .finish();
    format!(
        "{}/roundups/{}/{}?{query}",
        public_base_url.trim_end_matches('/'),
        scope.as_str(),
        scope.id(),
    )
}

/// Stored model text escaped Markdown punctuation. Keep headings and citation
/// links, but remove those escapes for a readable plain-text email.
fn readable_markdown(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\'
            && characters
                .peek()
                .is_some_and(|next| next.is_ascii_punctuation())
        {
            continue;
        }
        output.push(character);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_text_keeps_structure_but_drops_model_escapes() {
        assert_eq!(
            readable_markdown("Intro\\.\n\n## Theme\\!\n\nBody [1](/articles/id)"),
            "Intro.\n\n## Theme!\n\nBody [1](/articles/id)"
        );
    }
}
