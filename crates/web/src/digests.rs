//! Personal daily briefings assembled from shared stories.

use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;

use rusty_feed_core::reading::BriefingStory;
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
            "Briefings are disabled in the demo.".into(),
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
        format!("Updated today's briefing from {grouped} shared stories in this pass.")
    } else if grouped == 0 {
        "No second source matched a recent article. Refresh feeds, then try again after embeddings finish.".to_string()
    } else {
        "Stories were grouped, but none of them are in your subscriptions yet.".to_string()
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
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let digests = digests::list_for_user(state.db.pool(), user.id, 14)
        .await?
        .into_iter()
        .map(|digest| DigestRow {
            when: ui.format_date(digest.created_at),
            title: digest.title,
            body: digest.body,
        })
        .collect();
    let live_stories = stories::briefing_stories(state.db.pool(), user.id).await?;
    let body = DigestsTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui,
        is_admin: user.role == "admin" || user.role == "admin_readonly",
        digests,
        stories: live_stories,
        notice,
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
