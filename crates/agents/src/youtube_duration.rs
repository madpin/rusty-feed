//! Optional shared YouTube duration enrichment through bounded public-page parsing.

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use rustss_db::youtube_metadata;

use crate::{Agent, AgentContext};

#[derive(Deserialize)]
struct Payload {
    video_id: String,
}

pub struct YouTubeDurationAgent;

fn duration_from_page(page: &str) -> Option<i32> {
    Regex::new(r#""lengthSeconds"\s*:\s*"(\d{1,8})""#)
        .ok()?
        .captures(page)
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse::<i32>().ok())
        .filter(|value| *value > 0)
}

#[async_trait]
impl Agent for YouTubeDurationAgent {
    fn name(&self) -> &'static str {
        "youtube_duration"
    }

    async fn run(&self, ctx: &AgentContext, payload: Value) -> anyhow::Result<Value> {
        let Payload { video_id } = serde_json::from_value(payload)?;
        if !ctx.youtube_duration_enabled {
            return Ok(json!({"skipped": "duration enrichment disabled"}));
        }
        if !rustss_core::youtube::valid_video_id(&video_id) {
            anyhow::bail!("invalid YouTube video identifier");
        }
        if youtube_metadata::attempts_today(ctx.db.pool()).await?
            >= i64::from(ctx.youtube_duration_daily_budget)
        {
            anyhow::bail!("YouTube duration daily budget reached");
        }
        let url = format!("https://www.youtube.com/watch?v={video_id}");
        let body = match ctx.fetch.get_bytes(&url, 2 * 1_024 * 1_024).await {
            Ok(body) => body,
            Err(error) => {
                youtube_metadata::mark_failed(ctx.db.pool(), &video_id, "publisher fetch failed")
                    .await?;
                return Err(error.into());
            }
        };
        let page = String::from_utf8_lossy(&body);
        let duration = duration_from_page(&page);
        let Some(duration) = duration else {
            youtube_metadata::mark_failed(ctx.db.pool(), &video_id, "duration unavailable").await?;
            anyhow::bail!("YouTube page did not expose a duration");
        };
        youtube_metadata::mark_ready(
            ctx.db.pool(),
            &video_id,
            duration,
            ctx.youtube_duration_cache_days,
        )
        .await?;
        Ok(json!({"video_id": video_id, "duration_secs": duration}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_bounded_positive_duration() {
        assert_eq!(
            duration_from_page(r#"{"videoDetails":{"lengthSeconds":"213"}}"#),
            Some(213)
        );
        assert_eq!(duration_from_page(r#"{"lengthSeconds":"0"}"#), None);
        assert_eq!(duration_from_page(r#"{"lengthSeconds":"999999999"}"#), None);
        assert_eq!(duration_from_page("not a YouTube page"), None);
    }
}
