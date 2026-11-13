//! Personal daily briefings assembled from shared stories. The user-facing
//! word is "Briefing" (`/briefing`); the `digest` names below are internal.

use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;

use rusty_feed_core::reading::{BriefingStory, STORY_CLUSTER_MIN_SIMILARITY};
use rusty_feed_db::{digests, jobs, stories};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

#[derive(Template)]
#[template(path = "digests.html")]
struct DigestsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    digests: Vec<DigestRow>,
    stories: Vec<BriefingStory>,
    notice: Option<String>,
    daily_briefing_on: bool,
    similarity: String,
    recent_articles: i64,
    embedded_articles: i64,
}

struct DigestRow {
    title: String,
    body: String,
    when: String,
}

#[derive(Deserialize)]
pub(crate) struct CsrfForm {
    csrf_token: String,
}

/// Permanent redirect from the former `/digests` address.
pub async fn moved_page() -> Redirect {
    Redirect::permanent("/briefing")
}

/// Permanent redirect (method-preserving) from the former `/digests/refresh`.
pub async fn moved_refresh() -> Redirect {
    Redirect::permanent("/briefing/refresh")
}

pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render(&state, &user, jar, None).await
}

pub async fn refresh(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if user.is_demo {
        return Err(AppError::Validation(
            "The briefing is disabled in the demo.".into(),
        ));
    }
    let stats = stories::cluster_recent(state.db.pool()).await?;
    let written = digests::write_today(state.db.pool(), user.id).await?;
    jobs::enqueue(
        state.db.pool(),
        "cluster_stories",
        serde_json::json!({}),
        Some("cluster_stories"),
        None,
        3,
    )
    .await?;
    jobs::enqueue(
        state.db.pool(),
        "daily_digest",
        serde_json::json!({}),
        Some("daily_digest"),
        None,
        3,
    )
    .await?;
    let grouped = stats.joined + stats.created;
    let notice = if written.is_some() {
        format!("Updated today's briefing from {grouped} stories found in this pass.")
    } else if grouped == 0 {
        "No story found yet: no two of your feeds covered the same event. Refresh your feeds, then try again once the search index has caught up.".to_string()
    } else {
        "Stories were found, but none of them involve your feeds yet.".to_string()
    };
    render(&state, &user, jar, Some(notice)).await
}

async fn render(
    state: &AppState,
    user: &rusty_feed_db::User,
    jar: CookieJar,
    notice: Option<String>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let daily_briefing_on = ui.daily_digest;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let digests: Vec<DigestRow> = digests::list_for_user(state.db.pool(), user.id, 14)
        .await?
        .into_iter()
        .map(|digest| DigestRow {
            when: ui.format_date(digest.created_at),
            title: digest.title,
            body: digest.body,
        })
        .collect();
    let live_stories = stories::briefing_stories(state.db.pool(), user.id).await?;
    let coverage = if live_stories.is_empty() && digests.is_empty() {
        stories::embedding_coverage_for_user(state.db.pool(), user.id).await?
    } else {
        stories::EmbeddingCoverage {
            recent: 0,
            embedded: 0,
        }
    };
    let body = DigestsTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui,
        is_admin: user.role == "admin" || user.role == "admin_readonly",
        digests,
        stories: live_stories,
        notice,
        daily_briefing_on,
        similarity: format!("{STORY_CLUSTER_MIN_SIMILARITY:.2}"),
        recent_articles: coverage.recent,
        embedded_articles: coverage.embedded,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    let mut response = (jar, body).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}
