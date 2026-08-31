//! Indexed keyword, semantic, and hybrid search over a user's subscriptions.

use askama::Template;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Response};
use axum_extra::extract::cookie::CookieJar;
use rustss_core::config::{SearchMode, SearchTextConfig};
use rustss_db::{embeddings, search as article_search};
use serde::Deserialize;

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
    effective_mode: String,
    effective_text_config: String,
    semantic_weight_percent: u8,
    fallback_notice: Option<String>,
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
    let mode = SearchMode::parse(&requested_mode).unwrap_or(preferred);
    let text_config = text_config(&preferences, &state.settings);
    let semantic_weight_percent = preferences
        .search_semantic_weight_percent
        .unwrap_or(state.settings.search_semantic_weight_percent);
    let semantic_weight = f64::from(semantic_weight_percent) / 100.0;
    let mut fallback_notice = None;

    let (searched, hits) = if trimmed.is_empty() {
        (false, Vec::new())
    } else {
        let results = match (mode, semantic_weight_percent) {
            (SearchMode::Keyword, _) | (SearchMode::Hybrid, 0) => {
                article_search::keyword_for_user(state.db.pool(), user.id, trimmed, text_config, 30)
                    .await?
            }
            (SearchMode::Hybrid | SearchMode::Semantic, _) => {
                match state.embedder.embed_one(trimmed).await {
                    Ok(vector) if mode == SearchMode::Hybrid && semantic_weight_percent < 100 => {
                        article_search::hybrid_for_user(
                            state.db.pool(),
                            &article_search::HybridSearch {
                                user_id: user.id,
                                query: trimmed,
                                query_vector: &vector,
                                model: state.embedder.model(),
                                text_config,
                                semantic_weight,
                                limit: 30,
                            },
                        )
                        .await?
                    }
                    Ok(vector) => embeddings::search_for_user(
                        state.db.pool(),
                        user.id,
                        &vector,
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
                    .collect(),
                    Err(error) => {
                        tracing::warn!(error = %error, "semantic query embedding unavailable; using indexed keyword search");
                        fallback_notice = Some(
                            "Meaning search is temporarily unavailable. Showing exact PostgreSQL matches instead."
                                .to_string(),
                        );
                        article_search::keyword_for_user(
                            state.db.pool(),
                            user.id,
                            trimmed,
                            text_config,
                            30,
                        )
                        .await?
                    }
                }
            }
        };
        (true, results.into_iter().map(ranked_hit).collect())
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
        effective_mode: mode.as_str().to_string(),
        effective_text_config: text_config.as_str().to_string(),
        semantic_weight_percent,
        fallback_notice,
        searched,
        hits,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    Ok((jar, body).into_response())
}
