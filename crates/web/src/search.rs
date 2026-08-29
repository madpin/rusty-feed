//! Semantic search over the user's subscribed articles (M9): embed the query
//! with the instance model, then KNN against `article_embeddings`.

use askama::Template;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Response};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;

use rustss_db::embeddings;

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

struct Hit {
    id: String,
    title: String,
    excerpt: Option<String>,
    feed_name: String,
    published: Option<String>,
    similarity: i64, // percent, for display
}

#[derive(Template)]
#[template(path = "search.html")]
struct SearchTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    query: String,
    searched: bool,
    hits: Vec<Hit>,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    q: Option<String>,
}

/// `GET /search?q=...`.
pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Query(query): Query<SearchQuery>,
) -> Result<Response, AppError> {
    let prefs = prefs::load(&state.db, user.id).await;
    let q = query.q.unwrap_or_default();
    let trimmed = q.trim();

    let (searched, hits) = if trimmed.is_empty() {
        (false, Vec::new())
    } else {
        let vector = state
            .embedder
            .embed_one(trimmed)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!("embedding failed: {e}")))?;
        let results = embeddings::search_for_user(
            state.db.pool(),
            user.id,
            &vector,
            state.embedder.model(),
            30,
        )
        .await?;
        let hits = results
            .into_iter()
            .map(|r| Hit {
                id: r.id.to_string(),
                title: r.title,
                excerpt: r.excerpt,
                feed_name: r.feed_name,
                published: r.published_at.map(|d| d.date().to_string()),
                similarity: (r.similarity.clamp(0.0, 1.0) * 100.0).round() as i64,
            })
            .collect();
        (true, hits)
    };

    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = SearchTemplate {
        theme: prefs.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui: prefs,
        query: q,
        searched,
        hits,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, body).into_response())
}
