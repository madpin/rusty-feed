//! # rusty-feed-llm
//!
//! LLM access for summaries/digests (M12): an OpenAI-compatible client that also
//! drives Ollama (via its `/v1/chat/completions` endpoint), robust
//! summary-JSON parsing, a pricing map, and the shared-article summarize
//! orchestration (first-writer-wins) with durable cost accounting.

mod pricing;

pub use pricing::cost_micro_usd;

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use uuid::Uuid;

use rusty_feed_core::ai_budget::AiBudget;
use rusty_feed_core::config::LlmProvider;
use rusty_feed_core::crypto::Cryptor;
use rusty_feed_core::html::article_html_to_markdown;
use rusty_feed_db::{articles, cost_ledger, llm_profiles, Article, Db};

/// Why a cascade summarize did not produce a summary.
#[derive(Debug, thiserror::Error)]
pub enum SummarizeError {
    /// Over an AI budget/quota. The message is safe to show the user.
    #[error("{0}")]
    Budget(String),
    /// No AI provider is configured for this instance/user.
    #[error("no AI summary provider is configured")]
    NoProvider,
    /// Every configured provider was attempted and failed.
    #[error("all configured AI summary providers failed ({0})")]
    AllFailed(String),
    /// An unexpected error (DB, decryption, …).
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// A chat completion result with token usage.
#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A minimal chat LLM.
#[async_trait]
pub trait LlmClient: Send + Sync {
    fn provider(&self) -> &str;
    fn model(&self) -> &str;
    async fn complete(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> anyhow::Result<Completion>;
}

/// OpenAI-compatible chat client (works for OpenAI and Ollama's `/v1`).
pub struct OpenAiCompatClient {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
    provider: String,
}

impl OpenAiCompatClient {
    pub fn new(provider: &str, base_url: String, api_key: Option<String>, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            api_key,
            model,
            provider: provider.to_string(),
        }
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}
#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}
#[derive(Deserialize)]
struct ChoiceMessage {
    #[serde(default)]
    content: String,
}
#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[async_trait]
impl LlmClient for OpenAiCompatClient {
    fn provider(&self) -> &str {
        &self.provider
    }
    fn model(&self) -> &str {
        &self.model
    }

    async fn complete(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> anyhow::Result<Completion> {
        let url = api_url(&self.base_url, "chat/completions");
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
            "max_tokens": max_tokens,
            "temperature": 0.3,
            "stream": false,
        });
        let mut req = self.client.post(url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await?.error_for_status()?;
        let parsed: ChatResponse = resp.json().await?;
        let text = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        let usage = parsed.usage.unwrap_or_default();
        Ok(Completion {
            text,
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
        })
    }
}

/// Configuration for [`build_client`].
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub provider: LlmProvider,
    pub model: String,
    pub openai_api_key: Option<String>,
    pub openai_base_url: Option<String>,
    pub ollama_base_url: String,
}

/// Build the configured client, or `None` when the provider isn't usable
/// (e.g. OpenAI selected without an API key).
pub fn build_client(config: LlmConfig) -> Option<Arc<dyn LlmClient>> {
    match config.provider {
        LlmProvider::Openai => {
            let key = config.openai_api_key?;
            let base = config
                .openai_base_url
                .unwrap_or_else(|| "https://api.openai.com".to_string());
            Some(Arc::new(OpenAiCompatClient::new(
                "openai",
                base,
                Some(key),
                config.model,
            )))
        }
        LlmProvider::Ollama => Some(Arc::new(OpenAiCompatClient::new(
            "ollama",
            config.ollama_base_url,
            None,
            config.model,
        ))),
    }
}

/// Resolves managed providers in user → administrator → environment order.
/// Failed providers are tried in sequence; credentials are decrypted only when
/// a summary job runs and are never logged or returned to the browser.
pub struct ProviderCascade {
    db: Db,
    cryptor: Cryptor,
    environment: Option<Arc<dyn LlmClient>>,
}

impl ProviderCascade {
    pub fn new(db: Db, encryption_secret: &[u8], environment: Option<Arc<dyn LlmClient>>) -> Self {
        Self {
            db,
            cryptor: Cryptor::new_with_context(
                encryption_secret,
                b"rusty-feed/llm-provider-credentials/v1",
            ),
            environment,
        }
    }

    pub fn has_environment(&self) -> bool {
        self.environment.is_some()
    }

    /// Resolved clients paired with whether each is **user-funded** (the user's
    /// own enabled key). User-funded spend is exempt from the instance budget.
    async fn clients(
        &self,
        user_id: Option<Uuid>,
    ) -> anyhow::Result<Vec<(Arc<dyn LlmClient>, bool)>> {
        let profiles = llm_profiles::resolve(self.db.pool(), user_id).await?;
        let mut clients: Vec<(Arc<dyn LlmClient>, bool)> = Vec::with_capacity(profiles.len() + 1);
        for profile in profiles {
            let user_funded = profile.scope == "user";
            let key = match profile.api_key_ciphertext.as_deref() {
                Some(ciphertext) => match self.cryptor.decrypt_str(ciphertext) {
                    Ok(key) => Some(key),
                    // A single corrupt/rotated credential must not break every
                    // provider — skip it and try the next (graceful degradation).
                    Err(_) => {
                        tracing::warn!(
                            profile = %profile.id,
                            scope = %profile.scope,
                            "skipping AI provider: credential could not be decrypted"
                        );
                        continue;
                    }
                },
                None => None,
            };
            clients.push((
                Arc::new(OpenAiCompatClient::new(
                    &profile.provider,
                    profile.base_url,
                    key,
                    profile.model,
                )),
                user_funded,
            ));
        }
        if let Some(environment) = &self.environment {
            // The environment key is instance-funded.
            clients.push((environment.clone(), false));
        }
        Ok(clients)
    }
}

/// Whether an *instance-funded* summary call is currently within budget for this
/// user. BYO-key spend is unaffected by these caps (`§4.1`, `§8.1`).
async fn instance_funding_allowed(
    db: &Db,
    budget: &AiBudget,
    user_id: Option<Uuid>,
) -> anyhow::Result<bool> {
    let since = AiBudget::today_start();
    if budget.has_global_llm_cap() {
        let spent = cost_ledger::instance_spend_micro_usd_since(db.pool(), since).await?;
        if spent >= budget.llm_global_daily_micro_usd {
            return Ok(false);
        }
    }
    if let Some(uid) = user_id {
        if budget.has_per_user_llm_usd_cap() {
            let spent = cost_ledger::user_spend_micro_usd_since(db.pool(), uid, since).await?;
            if spent >= budget.llm_per_user_daily_micro_usd {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Whether this user has hit today's LLM call cap (funding-agnostic: bounds
/// job/DB abuse even for BYO-key users).
async fn per_user_call_cap_reached(
    db: &Db,
    budget: &AiBudget,
    user_id: Option<Uuid>,
) -> anyhow::Result<bool> {
    let (Some(uid), true) = (user_id, budget.has_per_user_llm_call_cap()) else {
        return Ok(false);
    };
    let since = AiBudget::today_start();
    let calls = cost_ledger::user_call_count_since(db.pool(), uid, "summary", since).await?;
    Ok(calls >= i64::from(budget.llm_per_user_daily_calls))
}

/// Summarize through the configured cascade, stopping on the first successful
/// provider. A shared summary is still written at most once. Enforces the AI
/// budget: BYO-key clients are exempt from the instance USD budget, but the
/// per-user daily call cap applies to everyone (`docs/PUBLIC_LAUNCH_PLAN.md`
/// §4.1).
pub async fn summarize_article_with_cascade(
    cascade: &ProviderCascade,
    budget: &AiBudget,
    user_id: Option<Uuid>,
    article: &Article,
) -> Result<(bool, String), SummarizeError> {
    if per_user_call_cap_reached(&cascade.db, budget, user_id).await? {
        return Err(SummarizeError::Budget(
            "You've reached today's AI summary limit. Try again tomorrow, or add your own API key in Settings → AI.".to_string(),
        ));
    }
    let clients = cascade.clients(user_id).await?;
    if clients.is_empty() {
        return Err(SummarizeError::NoProvider);
    }
    let instance_allowed = instance_funding_allowed(&cascade.db, budget, user_id).await?;
    let mut failures = Vec::new();
    let mut skipped_over_budget = false;
    for (client, user_funded) in clients {
        if !user_funded && !instance_allowed {
            skipped_over_budget = true;
            continue;
        }
        match summarize_article(&cascade.db, client.as_ref(), user_id, article, user_funded).await {
            Ok(written) => return Ok((written, client.model().to_string())),
            Err(error) => {
                tracing::warn!(
                    provider = client.provider(),
                    model = client.model(),
                    error = %error,
                    "AI provider failed; trying the next configured provider"
                );
                failures.push(format!("{}/{}", client.provider(), client.model()));
            }
        }
    }
    if failures.is_empty() && skipped_over_budget {
        return Err(SummarizeError::Budget(
            "AI summaries are paused: today's shared budget is used up. Add your own API key in Settings → AI to keep summarizing.".to_string(),
        ));
    }
    Err(SummarizeError::AllFailed(failures.join(", ")))
}

/// A parsed article summary.
#[derive(Debug, Clone, Deserialize)]
pub struct Summary {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub key_points: Vec<String>,
    #[serde(default)]
    pub topics: Vec<String>,
}

const SUMMARY_SYSTEM: &str = "You summarize articles. Respond with ONLY a JSON object: \
{\"summary\": string (2-4 sentences), \"key_points\": array of 3-5 short strings, \
\"topics\": array of 1-5 short topic strings}. No prose outside the JSON.";

/// Ask the model to summarize an article; robustly parse the JSON (falling back
/// to treating the whole response as the summary).
pub async fn summarize(
    client: &dyn LlmClient,
    title: &str,
    content: &str,
) -> anyhow::Result<(Summary, Completion)> {
    let truncated: String = content.chars().take(6000).collect();
    let user = format!("Title: {title}\n\nContent:\n{truncated}");
    let completion = client.complete(SUMMARY_SYSTEM, &user, 600).await?;
    let summary = parse_summary(&completion.text);
    Ok((summary, completion))
}

/// Extract the JSON object from a model response, tolerating surrounding text or
/// code fences; fall back to the raw text as the summary.
pub fn parse_summary(text: &str) -> Summary {
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
        if end > start {
            if let Ok(s) = serde_json::from_str::<Summary>(&text[start..=end]) {
                if !s.summary.is_empty() || !s.key_points.is_empty() {
                    return s;
                }
            }
        }
    }
    Summary {
        summary: text.trim().to_string(),
        key_points: Vec::new(),
        topics: Vec::new(),
    }
}

/// Summarize a shared article once (first-writer-wins) and record the cost.
/// Returns whether this call wrote the summary.
pub async fn summarize_article(
    db: &Db,
    client: &dyn LlmClient,
    user_id: Option<Uuid>,
    article: &Article,
    billed_to_user: bool,
) -> anyhow::Result<bool> {
    // Skip the (paid) call if it's already summarized.
    if articles::is_summarized(db.pool(), article.id).await? {
        return Ok(false);
    }

    let markdown = article_html_to_markdown(&article.content, &article.url);
    let (summary, completion) = summarize(client, &article.title, &markdown).await?;
    let written = articles::set_summary(
        db.pool(),
        article.id,
        article.content_revision,
        &summary.summary,
        &summary.key_points,
        &summary.topics,
        client.model(),
    )
    .await?;

    if written {
        let cost = cost_micro_usd(
            client.model(),
            completion.input_tokens,
            completion.output_tokens,
        );
        cost_ledger::record(
            db.pool(),
            user_id,
            client.provider(),
            client.model(),
            "summary",
            completion.input_tokens as i64,
            completion.output_tokens as i64,
            cost,
            billed_to_user,
        )
        .await?;
    }
    Ok(written)
}

fn api_url(base_url: &str, endpoint: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let base = base.strip_suffix("/v1").unwrap_or(base);
    format!("{base}/v1/{endpoint}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_json() {
        let s = parse_summary(r#"{"summary":"A.","key_points":["x","y"],"topics":["t"]}"#);
        assert_eq!(s.summary, "A.");
        assert_eq!(s.key_points, vec!["x", "y"]);
        assert_eq!(s.topics, vec!["t"]);
    }

    #[test]
    fn parses_json_with_surrounding_text() {
        let s = parse_summary("Here you go:\n```json\n{\"summary\":\"B\",\"key_points\":[]}\n```");
        assert_eq!(s.summary, "B");
    }

    #[test]
    fn falls_back_to_raw_text() {
        let s = parse_summary("just a plain summary");
        assert_eq!(s.summary, "just a plain summary");
        assert!(s.key_points.is_empty());
    }

    #[test]
    fn accepts_openai_base_url_with_or_without_v1() {
        assert_eq!(
            api_url("https://api.openai.com", "chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            api_url("https://api.openai.com/v1/", "chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
    }
}
