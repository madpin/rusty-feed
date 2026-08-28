//! Feed management: subscribe (add), list, refresh, and unsubscribe. Adding a
//! feed validates + fetches it synchronously (background refresh is M7).

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use rustss_db::{feeds, subscriptions};
use rustss_feeds::{normalize_url, refresh_feed};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

struct FeedRow {
    id: String,
    name: String,
    url: String,
    health: String,
}

#[derive(Template)]
#[template(path = "feeds.html")]
struct FeedsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    feeds: Vec<FeedRow>,
    message: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
pub struct AddForm {
    csrf_token: String,
    url: String,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

async fn render_page(
    state: &AppState,
    user_id: Uuid,
    user_email: String,
    jar: CookieJar,
    message: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let theme = crate::prefs::load(&state.db, user_id).await.theme;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let feeds = feeds::list_for_user(state.db.pool(), user_id)
        .await?
        .into_iter()
        .map(|f| FeedRow {
            id: f.id.to_string(),
            name: f.name,
            url: f.url,
            health: f.health_status,
        })
        .collect();
    let body = FeedsTemplate {
        theme,
        user_email,
        csrf_token,
        feeds,
        message,
        error,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, body).into_response())
}

/// `GET /feeds`.
pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render_page(&state, user.id, user.email, jar, None, None).await
}

/// `POST /feeds` — subscribe to a feed by URL (validated + fetched now).
pub async fn add(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<AddForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }

    // Validate + normalize.
    let normalized = match normalize_url(&form.url) {
        Ok(u) => u,
        Err(e) => {
            return render_page(
                &state,
                user.id,
                user.email,
                jar,
                None,
                Some(format!("Invalid feed URL: {e}")),
            )
            .await;
        }
    };
    if let Err(e) = state.fetch.validate_url(&normalized) {
        return render_page(
            &state,
            user.id,
            user.email,
            jar,
            None,
            Some(format!("That URL can't be used: {e}")),
        )
        .await;
    }

    // Get-or-create the shared feed, subscribe, then fetch/ingest.
    let host = url::Url::parse(&normalized)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "Feed".to_string());
    let (feed, _created) = feeds::upsert_by_url(state.db.pool(), &normalized, &host, None).await?;
    subscriptions::subscribe(state.db.pool(), user.id, feed.id, None).await?;

    match refresh_feed(&state.db, &state.fetch, &feed).await {
        Ok(stats) => {
            let msg = format!(
                "Added “{}” — {} new article(s).",
                feed_display_name(&state, feed.id).await,
                stats.created
            );
            render_page(&state, user.id, user.email, jar, Some(msg), None).await
        }
        Err(e) => {
            let msg =
                format!("Subscribed, but fetching failed: {e}. It will be retried on refresh.");
            render_page(&state, user.id, user.email, jar, None, Some(msg)).await
        }
    }
}

async fn feed_display_name(state: &AppState, feed_id: Uuid) -> String {
    feeds::find_by_id(state.db.pool(), feed_id)
        .await
        .ok()
        .flatten()
        .map(|f| f.name)
        .unwrap_or_else(|| "feed".to_string())
}

/// `POST /feeds/{id}/refresh`.
pub async fn refresh(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !subscriptions::is_subscribed(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    let (message, error) = match feeds::find_by_id(state.db.pool(), id).await? {
        Some(feed) => match refresh_feed(&state.db, &state.fetch, &feed).await {
            Ok(s) => (
                Some(format!("Refreshed — {} new article(s).", s.created)),
                None,
            ),
            Err(e) => (None, Some(format!("Refresh failed: {e}"))),
        },
        None => (None, Some("Feed not found.".to_string())),
    };
    render_page(&state, user.id, user.email, jar, message, error).await
}

/// `POST /feeds/refresh-all`.
pub async fn refresh_all(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let feeds_list = feeds::list_for_user(state.db.pool(), user.id).await?;
    let mut created = 0u64;
    for feed in &feeds_list {
        if let Ok(s) = refresh_feed(&state.db, &state.fetch, feed).await {
            created += s.created;
        }
    }
    let msg = format!(
        "Refreshed {} feed(s) — {} new article(s).",
        feeds_list.len(),
        created
    );
    render_page(&state, user.id, user.email, jar, Some(msg), None).await
}

/// `POST /feeds/{id}/unsubscribe`.
pub async fn unsubscribe(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    subscriptions::unsubscribe(state.db.pool(), user.id, id).await?;
    Ok((jar, Redirect::to("/feeds")).into_response())
}
