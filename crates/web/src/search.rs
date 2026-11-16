//! Indexed keyword, semantic, and hybrid search over a user's subscriptions.

use askama::Template;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Response};
use axum_extra::extract::cookie::CookieJar;
use rusty_feed_core::ai_budget::AiBudget;
use rusty_feed_core::config::{SearchMode, SearchTextConfig};
use rusty_feed_db::{cost_ledger, embeddings, search as article_search};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::prefs;
use crate::state::{AppSettings, AppState};

struct Hit {
    id: String,
    title: String,
    excerpt: Option<String>,
    feed_name: String,
    published: Option<String>,
    score: i64,
    semantic_score: Option<i64>,
    keyword_score: Option<i64>,
}

#[derive(Template)]
#[template(path = "search.html")]
struct SearchTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    query: String,
    requested_mode: String,
    embeddings_enabled: bool,
    effective_mode: String,
    effective_text_config: String,
    semantic_weight_percent: u8,
    fallback_notice: Option<String>,
    /// More keyword matches than the ranking cap: only the newest were ranked.
    results_limited: bool,
    searched: bool,
    hits: Vec<Hit>,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    mode: Option<String>,
}

fn preferred_mode(preferences: &prefs::Preferences, settings: &AppSettings) -> SearchMode {
    SearchMode::parse(&preferences.search_mode).unwrap_or(settings.search_mode)
}

/// Whether this user has hit today's interactive query-embedding cap (FND-187).
async fn embed_call_cap_reached(state: &AppState, user_id: Uuid) -> Result<bool, AppError> {
    let budget = &state.settings.ai_budget;
    if !budget.has_embed_call_cap() {
        return Ok(false);
    }
    let since = AiBudget::today_start();
    let calls =
        cost_ledger::user_call_count_since(state.db.pool(), user_id, "embedding", since).await?;
    Ok(calls >= i64::from(budget.embed_per_user_daily_calls))
}

/// Record one interactive query embedding (per-user; instance-funded compute).
async fn record_query_embedding(state: &AppState, user_id: Uuid) {
    if let Err(error) = cost_ledger::record(
        state.db.pool(),
        Some(user_id),
        state.embedder.provider(),
        state.embedder.model(),
        "embedding",
        0,
        0,
        0,
        false,
    )
    .await
    {
        tracing::warn!(error = %error, "failed to record query embedding usage");
    }
}

fn text_config(preferences: &prefs::Preferences, settings: &AppSettings) -> SearchTextConfig {
    SearchTextConfig::parse(&preferences.search_text_config).unwrap_or(settings.search_text_config)
}

fn percent(score: f64) -> i64 {
    (score.clamp(0.0, 1.0) * 100.0).round() as i64
}

fn ranked_hit(hit: article_search::SearchHit) -> Hit {
    Hit {
        id: hit.id.to_string(),
        title: hit.title,
        excerpt: hit.excerpt,
        feed_name: hit.feed_name,
        published: hit.published_at.map(|date| date.date().to_string()),
        score: percent(hit.final_score),
        semantic_score: hit.semantic_score.map(percent),
        keyword_score: hit.keyword_score.map(percent),
    }
}

/// `GET /search?q=...&mode=preferred|hybrid|keyword|semantic`.
pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Query(query): Query<SearchQuery>,
) -> Result<Response, AppError> {
    if !state.search_limiter.check(&user.id.to_string()).await {
        return Err(AppError::RateLimited);
    }
    let preferences = prefs::load(&state.db, user.id).await;
    let is_admin = user.role == "admin";
    let q = query.q.unwrap_or_default();
    let trimmed = q.trim();
    if trimmed.chars().count() > 512 {
        return Err(AppError::Validation(
            "Search queries must be 512 characters or fewer.".to_string(),
        ));
    }
    let preferred = preferred_mode(&preferences, &state.settings);
    let requested_mode = match query.mode.as_deref() {
        Some("hybrid" | "keyword" | "semantic") => query.mode.unwrap_or_default(),
        _ => "preferred".to_string(),
    };
    // EMBEDDINGS_ENABLED=false is authoritative: ignore both the request and
    // any stale personal preference saved before the instance disabled it,
    // rather than relying solely on the embedder erroring on every query.
    let mode = if state.settings.embeddings_enabled {
        SearchMode::parse(&requested_mode).unwrap_or(preferred)
    } else {
        SearchMode::Keyword
    };
    let text_config = text_config(&preferences, &state.settings);
    let semantic_weight_percent = preferences
        .search_semantic_weight_percent
        .unwrap_or(state.settings.search_semantic_weight_percent);
    let semantic_weight = f64::from(semantic_weight_percent) / 100.0;
    let mut fallback_notice = None;

    let want_semantic = !trimmed.is_empty()
        && (matches!(mode, SearchMode::Semantic)
            || (mode == SearchMode::Hybrid && semantic_weight_percent > 0));

    // Embed the query at most once: metered per user and capped per day. On cap
    // or embedder failure, degrade gracefully to indexed keyword search rather
    // than rejecting the request (FND-187 §4.1).
    let query_vector = if !want_semantic {
        None
    } else if embed_call_cap_reached(&state, user.id).await? {
        fallback_notice = Some(
            "You've reached today's meaning-search limit. Showing exact PostgreSQL matches instead."
                .to_string(),
        );
        None
    } else {
        match state.embedder.embed_one(trimmed).await {
            Ok(vector) => {
                record_query_embedding(&state, user.id).await;
                Some(vector)
            }
            Err(error) => {
                tracing::warn!(error = %error, "semantic query embedding unavailable; using indexed keyword search");
                fallback_notice = Some(
                    "Meaning search is temporarily unavailable. Showing exact PostgreSQL matches instead."
                        .to_string(),
                );
                None
            }
        }
    };

    let (searched, results_limited, hits) = if trimmed.is_empty() {
        (false, false, Vec::new())
    } else {
        let page = match (mode, semantic_weight_percent, query_vector.as_ref()) {
            (SearchMode::Hybrid, weight, Some(vector)) if weight > 0 && weight < 100 => {
                article_search::hybrid_page_for_user(
                    state.db.pool(),
                    &article_search::HybridSearch {
                        user_id: user.id,
                        query: trimmed,
                        query_vector: vector,
                        model: state.embedder.model(),
                        text_config,
                        semantic_weight,
                        limit: 30,
                    },
                )
                .await?
            }
            (SearchMode::Semantic, _, Some(vector)) | (SearchMode::Hybrid, 100, Some(vector)) => {
                let hits = embeddings::search_for_user(
                    state.db.pool(),
                    user.id,
                    vector,
                    state.embedder.model(),
                    30,
                )
                .await?
                .into_iter()
                .map(|hit| article_search::SearchHit {
                    id: hit.id,
                    title: hit.title,
                    url: hit.url,
                    excerpt: hit.excerpt,
                    image_url: hit.image_url,
                    published_at: hit.published_at,
                    feed_name: hit.feed_name,
                    semantic_score: Some(hit.similarity),
                    keyword_score: None,
                    final_score: hit.similarity,
                })
                .collect();
                article_search::SearchPage {
                    hits,
                    truncated: false,
                }
            }
            _ => {
                article_search::keyword_page_for_user(
                    state.db.pool(),
                    user.id,
                    trimmed,
                    text_config,
                    30,
                )
                .await?
            }
        };
        (
            true,
            page.truncated,
            page.hits.into_iter().map(ranked_hit).collect(),
        )
    };

    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = SearchTemplate {
        theme: preferences.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui: preferences,
        is_admin,
        query: q,
        requested_mode,
        embeddings_enabled: state.settings.embeddings_enabled,
        effective_mode: mode.as_str().to_string(),
        effective_text_config: text_config.as_str().to_string(),
        semantic_weight_percent,
        fallback_notice,
        results_limited,
        searched,
        hits,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    Ok((jar, body).into_response())
}
