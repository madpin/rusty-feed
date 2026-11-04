//! ClusterStoriesAgent groups recent embedded articles into shared events.
//! DailyDigestAgent assembles a personal briefing from those events and, when
//! SMTP is configured, sends it to readers who asked for email.

use std::sync::Arc;

use async_trait::async_trait;
use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde_json::{json, Value};

use rusty_feed_core::ai_budget::AiBudget;
use rusty_feed_core::reading::fallback_story_summary;
use rusty_feed_db::{cost_ledger, digests, stories};
use rusty_feed_llm::{cost_micro_usd, LlmClient};

use crate::{Agent, AgentContext};

const SUMMARY_BATCH: i64 = 8;

pub struct ClusterStoriesAgent {
    llm: Option<Arc<dyn LlmClient>>,
}

impl ClusterStoriesAgent {
    pub fn new(llm: Option<Arc<dyn LlmClient>>) -> Self {
        Self { llm }
    }
}

#[async_trait]
impl Agent for ClusterStoriesAgent {
    fn name(&self) -> &'static str {
        "cluster_stories"
    }

    async fn run(&self, ctx: &AgentContext, _payload: Value) -> anyhow::Result<Value> {
        let stats = stories::cluster_recent(ctx.db.pool()).await?;
        let mut summarized = 0u64;
        let mut fallbacks = 0u64;
        let pending = stories::clusters_needing_summary(ctx.db.pool(), SUMMARY_BATCH).await?;
        let budget_open = instance_budget_open(ctx).await?;
        for (cluster_id, _title) in pending {
            let sources = stories::summary_sources(ctx.db.pool(), cluster_id).await?;
            let titles: Vec<String> = sources.iter().map(|(title, _)| title.clone()).collect();
            if titles.len() < 2 {
                continue;
            }
            let wrote = if budget_open {
                if let Some(llm) = &self.llm {
                    match summarize_story(llm.as_ref(), &sources).await {
                        Ok((text, completion)) => {
                            let stored =
                                stories::set_summary(ctx.db.pool(), cluster_id, &text).await?;
                            if stored {
                                let cost = cost_micro_usd(
                                    llm.model(),
                                    completion.input_tokens,
                                    completion.output_tokens,
                                );
                                cost_ledger::record(
                                    ctx.db.pool(),
                                    None,
                                    llm.provider(),
                                    llm.model(),
                                    "digest",
                                    completion.input_tokens as i64,
                                    completion.output_tokens as i64,
                                    cost,
                                    false,
                                )
                                .await?;
                                summarized += 1;
                            }
                            true
                        }
                        Err(error) => {
                            tracing::warn!(%error, %cluster_id, "story summary failed; leaving it for the next run");
                            false
                        }
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if !wrote && self.llm.is_none() {
                let text = fallback_story_summary(&titles);
                if stories::set_summary(ctx.db.pool(), cluster_id, &text).await? {
                    fallbacks += 1;
                }
            }
        }
        Ok(json!({
            "considered": stats.considered,
            "joined": stats.joined,
            "created": stats.created,
            "summarized": summarized,
            "fallbacks": fallbacks,
        }))
    }
}

pub struct DailyDigestAgent {
    smtp_url: Option<String>,
    smtp_from: Option<String>,
    public_base_url: String,
}

impl DailyDigestAgent {
    pub fn new(
        smtp_url: Option<String>,
        smtp_from: Option<String>,
        public_base_url: String,
    ) -> Self {
        Self {
            smtp_url,
            smtp_from,
            public_base_url,
        }
    }
}

#[async_trait]
impl Agent for DailyDigestAgent {
    fn name(&self) -> &'static str {
        "daily_digest"
    }

    async fn run(&self, ctx: &AgentContext, _payload: Value) -> anyhow::Result<Value> {
        let users = digests::users_wanting_daily(ctx.db.pool(), 2_000).await?;
        let mut written = 0u64;
        for user_id in users {
            if digests::write_today(ctx.db.pool(), user_id)
                .await?
                .is_some()
            {
                written += 1;
            }
        }
        let mut emailed = 0u64;
        if self.smtp_url.is_some() && self.smtp_from.is_some() {
            for _ in 0..50 {
                let Some(pending) = digests::claim_pending_email(ctx.db.pool()).await? else {
                    break;
                };
                let link = format!("{}/digests", self.public_base_url.trim_end_matches('/'));
                let body = format!(
                    "{}\n\n{}\n\nOpen the briefing:\n{link}\n",
                    pending.title, pending.body
                );
                if let Err(error) = send_briefing(
                    self.smtp_url.as_deref(),
                    self.smtp_from.as_deref(),
                    &pending.email,
                    &pending.title,
                    &body,
                )
                .await
                {
                    tracing::warn!(error = %error, digest_id = %pending.id, "briefing email failed");
                    digests::release_email_claim(ctx.db.pool(), pending.id).await?;
                    break;
                }
                emailed += 1;
            }
        }
        Ok(json!({ "written": written, "emailed": emailed }))
    }
}

async fn instance_budget_open(ctx: &AgentContext) -> anyhow::Result<bool> {
    if !ctx.ai_budget.has_global_llm_cap() {
        return Ok(true);
    }
    let spent =
        cost_ledger::instance_spend_micro_usd_since(ctx.db.pool(), AiBudget::today_start()).await?;
    Ok(spent < ctx.ai_budget.llm_global_daily_micro_usd)
}

async fn summarize_story(
    llm: &dyn LlmClient,
    sources: &[(String, String)],
) -> anyhow::Result<(String, rusty_feed_llm::Completion)> {
    let mut prompt = String::from("Sources:\n");
    for (title, excerpt) in sources.iter().take(6) {
        let excerpt: String = excerpt.chars().take(240).collect();
        prompt.push_str("- ");
        prompt.push_str(title.trim());
        if !excerpt.trim().is_empty() {
            prompt.push_str(": ");
            prompt.push_str(excerpt.trim());
        }
        prompt.push('\n');
    }
    let completion = llm
        .complete(
            "You write a neutral news briefing from several headlines about one event. \
             Reply with 2 or 3 plain sentences and no markdown, list, or JSON.",
            &prompt,
            320,
        )
        .await?;
    let text: String = completion.text.chars().take(1_200).collect();
    Ok((text.trim().to_string(), completion))
}

async fn send_briefing(
    smtp_url: Option<&str>,
    smtp_from: Option<&str>,
    to: &str,
    subject: &str,
    body: &str,
) -> anyhow::Result<()> {
    let (Some(url), Some(from)) = (smtp_url, smtp_from) else {
        return Ok(());
    };
    let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)?.build();
    let from: Mailbox = from
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid SMTP from address: {error}"))?;
    let message = Message::builder()
        .from(from)
        .to(to
            .parse()
            .map_err(|error| anyhow::anyhow!("invalid briefing recipient: {error}"))?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())?;
    transport.send(message).await?;
    Ok(())
}
