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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractFailure {
    InvalidUrl,
    Network,
    ResponseTooLarge,
}

impl ExtractFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidUrl => "invalid_url",
            Self::Network => "network",
            Self::ResponseTooLarge => "response_too_large",
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
    RobotsUnavailable,
    HttpStatus(u16),
    ReaderFailed,
    FetchFailed(ExtractFailure),
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
        delay_override_ms: Option<u64>,
    ) -> ExtractOutcome {
        let parsed = match url::Url::parse(url) {
            Ok(u) => u,
            Err(_) => return ExtractOutcome::FetchFailed(ExtractFailure::InvalidUrl),
        };
        let Some(domain) = parsed.host_str().map(str::to_string) else {
            return ExtractOutcome::FetchFailed(ExtractFailure::InvalidUrl);
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
        self.apply_politeness(db, &domain, robots_delay_ms, delay_override_ms)
            .await;

        match self.fetch_html(fetch.client(), fetch, url).await {
            Ok(FetchResult::Html {
                body,
                final_url,
                verified_html,
            }) => match self.run_extractors(&body, selector, &final_url) {
                Some(mut extracted) => {
                    extracted.verified_html = verified_html;
                    let cd = robots_delay_ms.map(|ms| ms as i32);
                    let _ = domain_state::record_success(db.pool(), &domain, cd).await;
                    ExtractOutcome::Extracted(extracted)
                }
                None => {
                    let _ = domain_state::record_failure(db.pool(), &domain, "reader_failed").await;
                    ExtractOutcome::ReaderFailed
                }
            },
            Ok(FetchResult::RateLimited(retry_after)) => {
                let _ = domain_state::record_rate_limit(db.pool(), &domain).await;
                ExtractOutcome::RateLimited { retry_after }
            }
            Ok(FetchResult::Status(code)) => {
                let _ = domain_state::record_failure(db.pool(), &domain, "http_status").await;
                ExtractOutcome::HttpStatus(code)
            }
            Err(error) => {
                let failure = match error {
                    ExtractError::Url(_) => ExtractFailure::InvalidUrl,
                    ExtractError::Http(_) => ExtractFailure::Network,
                    ExtractError::TooLarge => ExtractFailure::ResponseTooLarge,
                    ExtractError::Status(_) => ExtractFailure::Network,
                };
                let _ = domain_state::record_failure(db.pool(), &domain, failure.as_str()).await;
                ExtractOutcome::FetchFailed(failure)
            }
        }
    }

    pub async fn probe_public_strict(
        &self,
        db: &Db,
        fetch: &SafeFetchClient,
        url: &str,
    ) -> ExtractOutcome {
        let parsed = match url::Url::parse(url) {
            Ok(url) => url,
            Err(_) => return ExtractOutcome::FetchFailed(ExtractFailure::InvalidUrl),
        };
        let Some(domain) = parsed.host_str().map(str::to_string) else {
            return ExtractOutcome::FetchFailed(ExtractFailure::InvalidUrl);
        };
        let Some(robots) = self.strict_robots_for(fetch, &parsed).await else {
            return ExtractOutcome::RobotsUnavailable;
        };
        if !robots.is_allowed(parsed.path()) {
            return ExtractOutcome::Disallowed;
        }
        self.apply_politeness(db, &domain, robots.crawl_delay_ms, None)
            .await;
        let outcome = match self
            .fetch_html(fetch.client_without_redirects(), fetch, url)
            .await
        {
            Ok(FetchResult::Html {
                body,
                final_url,
                verified_html,
            }) => match self.run_extractors(&body, None, &final_url) {
                Some(mut extracted) => {
                    extracted.verified_html = verified_html;
                    ExtractOutcome::Extracted(extracted)
                }
                None => ExtractOutcome::ReaderFailed,
            },
            Ok(FetchResult::RateLimited(retry_after)) => {
                ExtractOutcome::RateLimited { retry_after }
            }
            Ok(FetchResult::Status(code)) => ExtractOutcome::HttpStatus(code),
            Err(error) => match error {
                ExtractError::Url(_) => ExtractOutcome::FetchFailed(ExtractFailure::InvalidUrl),
                ExtractError::Http(_) | ExtractError::Status(_) => {
                    ExtractOutcome::FetchFailed(ExtractFailure::Network)
                }
                ExtractError::TooLarge => {
                    ExtractOutcome::FetchFailed(ExtractFailure::ResponseTooLarge)
                }
            },
        };
        match &outcome {
            ExtractOutcome::Extracted(_) => {
                let _ = domain_state::record_success(
                    db.pool(),
                    &domain,
                    robots.crawl_delay_ms.map(|delay| delay as i32),
                )
                .await;
            }
            ExtractOutcome::RateLimited { .. } => {
                let _ = domain_state::record_rate_limit(db.pool(), &domain).await;
            }
            ExtractOutcome::HttpStatus(_) => {
                let _ = domain_state::record_failure(db.pool(), &domain, "http_status").await;
            }
            ExtractOutcome::ReaderFailed => {
                let _ = domain_state::record_failure(db.pool(), &domain, "reader_failed").await;
            }
            ExtractOutcome::FetchFailed(failure) => {
                let _ = domain_state::record_failure(db.pool(), &domain, failure.as_str()).await;
            }
            ExtractOutcome::Disallowed | ExtractOutcome::RobotsUnavailable => {}
        }
        outcome
    }

    fn run_extractors(
        &self,
        html: &str,
        selector: Option<&str>,
        base_url: &str,
    ) -> Option<Extracted> {
        self.extractors
            .iter()
            .find_map(|ex| ex.extract(html, selector, base_url))
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

    async fn strict_robots_for(
        &self,
        fetch: &SafeFetchClient,
        url: &url::Url,
    ) -> Option<Arc<RobotsInfo>> {
        let host = url.host_str()?;
        let port = url.port_or_known_default()?;
        let cache_key = format!("strict:{}://{host}:{port}", url.scheme());
        if let Some(info) = self.robots_cache.get(&cache_key).await {
            return Some(info);
        }
        let host_for_url = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        let robots_url = format!("{}://{host_for_url}:{port}/robots.txt", url.scheme());
        if fetch.validate_url(&robots_url).is_err() {
            return None;
        }
        let response = fetch
            .client_without_redirects()
            .get(robots_url)
            .send()
            .await
            .ok()?;
        let info = if response.status() == reqwest::StatusCode::NOT_FOUND {
            RobotsInfo::allow_all()
        } else if response.status().is_success() {
            let body = read_capped(response, 512 * 1024).await.ok()?;
            RobotsInfo::parse(&String::from_utf8_lossy(&body), &self.config.user_agent)
        } else {
            return None;
        };
        let info = Arc::new(info);
        self.robots_cache.insert(cache_key, info.clone()).await;
        Some(info)
    }

    async fn apply_politeness(
        &self,
        db: &Db,
        domain: &str,
        robots_delay_ms: Option<u64>,
        delay_override_ms: Option<u64>,
    ) {
        let base = delay_override_ms.unwrap_or(self.config.base_delay.as_millis() as u64);
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

    async fn fetch_html(
        &self,
        client: &reqwest::Client,
        fetch: &SafeFetchClient,
        url: &str,
    ) -> Result<FetchResult> {
        fetch
            .validate_url(url)
            .map_err(|e| ExtractError::Url(e.to_string()))?;
        let resp = client
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
        let final_url = resp.url().to_string();
        let verified_html = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                let value = value.to_ascii_lowercase();
                value.starts_with("text/html") || value.starts_with("application/xhtml+xml")
            });
        let bytes = read_capped(resp, self.config.max_bytes).await?;
        Ok(FetchResult::Html {
            body: String::from_utf8_lossy(&bytes).into_owned(),
            final_url,
            verified_html,
        })
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
    Html {
        body: String,
        final_url: String,
        verified_html: bool,
    },
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
