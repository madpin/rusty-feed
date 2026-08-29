//! # rustss-llm
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

use rustss_core::config::LlmProvider;
use rustss_db::{articles, cost_ledger, Article, Db};

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
) -> anyhow::Result<bool> {
    // Skip the (paid) call if it's already summarized.
    if articles::is_summarized(db.pool(), article.id).await? {
        return Ok(false);
    }

    let (summary, completion) = summarize(client, &article.title, &article.content).await?;
    let written = articles::set_summary(
        db.pool(),
        article.id,
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
