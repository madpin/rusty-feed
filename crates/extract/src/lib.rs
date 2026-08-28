//! # rustss-extract
//!
//! Full-text content extraction (M8): fetch an article page through the
//! SSRF-safe client, respect per-domain politeness (base delay, robots.txt
//! crawl-delay, adaptive backoff on 429/failures), run a Readability heuristic
//! (with a raw fallback), and sanitize before returning. See
//! `docs/ARCHITECTURE.md` §12. Headless-Chrome extraction is a future extractor
//! (FND-120).

pub mod error;
mod readability;
pub mod robots;

pub use error::{ExtractError, Result};
pub use readability::{sanitize, Extracted, Extractor, RawExtractor, ReadabilityExtractor};

use std::sync::Arc;
use std::time::Duration as StdDuration;

use futures_util::StreamExt;
use time::OffsetDateTime;

use rustss_db::{domain_state, Db};
use rustss_feeds::SafeFetchClient;

use crate::robots::RobotsInfo;

/// Tuning for extraction politeness and limits.
#[derive(Debug, Clone)]
pub struct ExtractConfig {
    pub base_delay: StdDuration,
    pub max_bytes: usize,
    pub robots_ttl: StdDuration,
    pub user_agent: String,
}

impl Default for ExtractConfig {
    fn default() -> Self {
        Self {
            base_delay: StdDuration::from_secs(3),
            max_bytes: 5 * 1024 * 1024,
            robots_ttl: StdDuration::from_secs(24 * 3600),
            user_agent: "RustSS/0.1".to_string(),
        }
    }
}

/// Result of an extraction attempt.
#[derive(Debug)]
pub enum ExtractOutcome {
    Extracted(Extracted),
    /// The domain rate-limited us; retry no sooner than `retry_after`.
    RateLimited {
        retry_after: StdDuration,
    },
    /// robots.txt disallows this path.
    Disallowed,
    /// A non-retryable/soft failure (keep the RSS content).
    Failed(String),
}

/// Extraction orchestrator: holds the extractor chain and an in-memory robots
/// cache. One per process (constructed in the agent).
pub struct ExtractionService {
    extractors: Vec<Box<dyn Extractor>>,
    robots_cache: moka::future::Cache<String, Arc<RobotsInfo>>,
    config: ExtractConfig,
}

impl ExtractionService {
    pub fn new(config: ExtractConfig) -> Self {
        let robots_cache = moka::future::Cache::builder()
            .time_to_live(config.robots_ttl)
            .max_capacity(10_000)
            .build();
        Self {
            extractors: vec![Box::new(ReadabilityExtractor), Box::new(RawExtractor)],
            robots_cache,
            config,
        }
    }

    /// Extract the article at `url`, applying per-domain politeness.
    pub async fn extract(
        &self,
        db: &Db,
        fetch: &SafeFetchClient,
        url: &str,
        selector: Option<&str>,
        respect_robots: bool,
    ) -> ExtractOutcome {
        let parsed = match url::Url::parse(url) {
            Ok(u) => u,
            Err(_) => return ExtractOutcome::Failed(format!("invalid URL: {url}")),
        };
        let Some(domain) = parsed.host_str().map(str::to_string) else {
            return ExtractOutcome::Failed("URL has no host".into());
        };

        // robots.txt (cached): crawl-delay + Disallow.
        let mut robots_delay_ms = None;
        if respect_robots {
            let info = self.robots_for(fetch, &parsed).await;
            if !info.is_allowed(parsed.path()) {
                let _ = domain_state::record_failure(db.pool(), &domain, "robots disallow").await;
                return ExtractOutcome::Disallowed;
            }
            robots_delay_ms = info.crawl_delay_ms;
        }

        // Per-domain spacing with adaptive backoff.
        self.apply_politeness(db, &domain, robots_delay_ms).await;

        match self.fetch_html(fetch, url).await {
            Ok(FetchResult::Html(html)) => match self.run_extractors(&html, selector) {
                Some(extracted) => {
                    let cd = robots_delay_ms.map(|ms| ms as i32);
                    let _ = domain_state::record_success(db.pool(), &domain, cd).await;
                    ExtractOutcome::Extracted(extracted)
                }
                None => {
                    let _ =
                        domain_state::record_failure(db.pool(), &domain, "no main content").await;
                    ExtractOutcome::Failed("no main content found".into())
                }
            },
            Ok(FetchResult::RateLimited(retry_after)) => {
                let _ = domain_state::record_rate_limit(db.pool(), &domain).await;
                ExtractOutcome::RateLimited { retry_after }
            }
            Ok(FetchResult::Status(code)) => {
                let _ =
                    domain_state::record_failure(db.pool(), &domain, &format!("HTTP {code}")).await;
                ExtractOutcome::Failed(format!("HTTP {code}"))
            }
            Err(e) => {
                let _ = domain_state::record_failure(db.pool(), &domain, &e.to_string()).await;
                ExtractOutcome::Failed(e.to_string())
            }
        }
    }

    fn run_extractors(&self, html: &str, selector: Option<&str>) -> Option<Extracted> {
        self.extractors
            .iter()
            .find_map(|ex| ex.extract(html, selector))
    }

    async fn robots_for(&self, fetch: &SafeFetchClient, url: &url::Url) -> Arc<RobotsInfo> {
        let host = url.host_str().unwrap_or_default().to_string();
        if let Some(info) = self.robots_cache.get(&host).await {
            return info;
        }
        let robots_url = format!(
            "{}://{}{}/robots.txt",
            url.scheme(),
            host,
            url.port().map(|p| format!(":{p}")).unwrap_or_default()
        );
        let info = match self.fetch_text(fetch, &robots_url).await {
            Ok(Some(body)) => RobotsInfo::parse(&body, &self.config.user_agent),
            _ => RobotsInfo::allow_all(),
        };
        let info = Arc::new(info);
        self.robots_cache.insert(host, info.clone()).await;
        info
    }

    async fn apply_politeness(&self, db: &Db, domain: &str, robots_delay_ms: Option<u64>) {
        let base = self.config.base_delay.as_millis() as u64;
        let mut delay = robots_delay_ms.unwrap_or(base).max(base);

        if let Ok(Some(state)) = domain_state::get(db.pool(), domain).await {
            // Adaptive backoff after prior rate-limits/failures.
            let strikes = (state.rate_limit_count + state.failure_count).clamp(0, 6) as u32;
            if strikes > 0 {
                delay = delay
                    .saturating_mul(2u64.saturating_pow(strikes))
                    .min(120_000);
            }
            let elapsed = (OffsetDateTime::now_utc() - state.last_extracted_at)
                .whole_milliseconds()
                .max(0) as u64;
            if elapsed < delay {
                let wait = (delay - elapsed).min(delay);
                tokio::time::sleep(StdDuration::from_millis(wait)).await;
            }
        }
    }

    async fn fetch_html(&self, fetch: &SafeFetchClient, url: &str) -> Result<FetchResult> {
        fetch
            .validate_url(url)
            .map_err(|e| ExtractError::Url(e.to_string()))?;
        let resp = fetch
            .client()
            .get(url)
            .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
            .send()
            .await
            .map_err(ExtractError::Http)?;

        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after =
                parse_retry_after(resp.headers()).unwrap_or(StdDuration::from_secs(60));
            return Ok(FetchResult::RateLimited(retry_after));
        }
        if !status.is_success() {
            return Ok(FetchResult::Status(status.as_u16()));
        }
        let bytes = read_capped(resp, self.config.max_bytes).await?;
        Ok(FetchResult::Html(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))
    }

    async fn fetch_text(&self, fetch: &SafeFetchClient, url: &str) -> Result<Option<String>> {
        if fetch.validate_url(url).is_err() {
            return Ok(None);
        }
        let resp = fetch
            .client()
            .get(url)
            .send()
            .await
            .map_err(ExtractError::Http)?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let bytes = read_capped(resp, 512 * 1024).await?;
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }
}

impl Default for ExtractionService {
    fn default() -> Self {
        Self::new(ExtractConfig::default())
    }
}

enum FetchResult {
    Html(String),
    RateLimited(StdDuration),
    Status(u16),
}

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<StdDuration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    value.trim().parse::<u64>().ok().map(StdDuration::from_secs)
}

async fn read_capped(resp: reqwest::Response, max: usize) -> Result<Vec<u8>> {
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(ExtractError::Http)?;
        if buf.len() + chunk.len() > max {
            return Err(ExtractError::TooLarge);
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}
