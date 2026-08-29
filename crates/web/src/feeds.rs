//! Feed management: subscribe, inspect shared source policy, queue refreshes,
//! and unsubscribe. Content acquisition stays shared per feed, regardless of
//! subscriber count.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use rustss_core::feed_content::{FeedContentMode, FeedContentSettings};
use rustss_db::{articles, feeds, subscriptions, Feed};
use rustss_feeds::{enqueue_fetch_feed, normalize_url};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

struct FeedRow {
    id: String,
    name: String,
    url: String,
    health: String,
    content_mode: String,
    content_mode_label: String,
    fetch_interval_minutes: i32,
    can_configure: bool,
}

struct FeedSettingsView {
    id: String,
    name: String,
    url: String,
    content_mode: String,
    selector: String,
    respect_robots_txt: bool,
    extraction_delay_ms: Option<i32>,
    fetch_interval_minutes: i32,
}

#[derive(Template)]
#[template(path = "feeds.html")]
struct FeedsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: crate::prefs::Preferences,
    feeds: Vec<FeedRow>,
    message: Option<String>,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "feed_settings.html")]
struct FeedSettingsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: crate::prefs::Preferences,
    feed: FeedSettingsView,
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

#[derive(Deserialize)]
pub struct ContentSettingsForm {
    csrf_token: String,
    content_mode: String,
    #[serde(default)]
    selector: String,
    #[serde(default)]
    respect_robots_txt: Option<String>,
    #[serde(default)]
    extraction_delay_ms: String,
    fetch_interval_minutes: String,
}

fn content_settings(feed: &Feed) -> FeedContentSettings {
    FeedContentSettings::from_json(feed.extraction_settings.as_ref())
}

fn feed_row(feed: Feed, can_configure: bool) -> FeedRow {
    let settings = content_settings(&feed);
    FeedRow {
        id: feed.id.to_string(),
        name: feed.name,
        url: feed.url,
        health: feed.health_status,
        content_mode: settings.mode.as_str().to_string(),
        content_mode_label: settings.mode.label().to_string(),
        fetch_interval_minutes: feed.fetch_interval_minutes,
        can_configure,
    }
}

fn feed_settings_view(feed: &Feed) -> FeedSettingsView {
    let settings = content_settings(feed);
    FeedSettingsView {
        id: feed.id.to_string(),
        name: feed.name.clone(),
        url: feed.url.clone(),
        content_mode: settings.mode.as_str().to_string(),
        selector: settings.selector.unwrap_or_default(),
        respect_robots_txt: feed.respect_robots_txt,
        extraction_delay_ms: feed.extraction_delay_ms,
        fetch_interval_minutes: feed.fetch_interval_minutes,
    }
}

async fn render_page(
    state: &AppState,
    user_id: Uuid,
    user_email: String,
    is_admin: bool,
    jar: CookieJar,
    message: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = crate::prefs::load(&state.db, user_id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let feeds = feeds::list_for_user(state.db.pool(), user_id)
        .await?
        .into_iter()
        .map(|feed| feed_row(feed, is_admin))
        .collect();
    let body = FeedsTemplate {
        theme: ui.theme.clone(),
        user_email,
        csrf_token,
        ui,
        feeds,
        message,
        error,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, body).into_response())
}

async fn render_settings_page(
    state: &AppState,
    user_id: Uuid,
    user_email: String,
    jar: CookieJar,
    feed: &Feed,
    message: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = crate::prefs::load(&state.db, user_id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = FeedSettingsTemplate {
        theme: ui.theme.clone(),
        user_email,
        csrf_token,
        ui,
        feed: feed_settings_view(feed),
        message,
        error,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, body).into_response())
}

fn parse_interval(value: &str) -> Result<i32, String> {
    value
        .trim()
        .parse::<i32>()
        .map(|minutes| minutes.clamp(5, 1440))
        .map_err(|_| "Refresh interval must be a whole number of minutes.".to_string())
}

fn parse_delay(value: &str) -> Result<Option<i32>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<i32>()
        .map(|delay| Some(delay.clamp(250, 120_000)))
        .map_err(|_| "Extraction delay must be a whole number of milliseconds.".to_string())
}

/// `GET /feeds`.
pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let is_admin = user.role == "admin";
    render_page(&state, user.id, user.email, is_admin, jar, None, None).await
}

/// `POST /feeds` — subscribe to a feed and queue the shared refresh path.
pub async fn add(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<AddForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let is_admin = user.role == "admin";
    let normalized = match normalize_url(&form.url) {
        Ok(url) => url,
        Err(error) => {
            return render_page(
                &state,
                user.id,
                user.email,
                is_admin,
                jar,
                None,
                Some(format!("Invalid feed URL: {error}")),
            )
            .await;
        }
    };
    if let Err(error) = state.fetch.validate_url(&normalized) {
        return render_page(
            &state,
            user.id,
            user.email,
            is_admin,
            jar,
            None,
            Some(format!("That URL cannot be used: {error}")),
        )
        .await;
    }

    let host = url::Url::parse(&normalized)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "Feed".to_string());
    let (feed, created) = feeds::upsert_by_url(state.db.pool(), &normalized, &host, None).await?;
    subscriptions::subscribe(state.db.pool(), user.id, feed.id, None).await?;
    let queued = enqueue_fetch_feed(&state.db, feed.id).await?.is_some();
    let refresh_message = if queued {
        "Refresh queued."
    } else {
        "A refresh is already queued."
    };
    let action = if created {
        "Subscribed to"
    } else {
        "Subscribed to existing"
    };
    render_page(
        &state,
        user.id,
        user.email,
        is_admin,
        jar,
        Some(format!("{action} “{}”. {refresh_message}", feed.name)),
        None,
    )
    .await
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
    let is_admin = user.role == "admin";
    let queued = enqueue_fetch_feed(&state.db, id).await?.is_some();
    let message = if queued {
        "Refresh queued.".to_string()
    } else {
        "A refresh is already queued.".to_string()
    };
    render_page(
        &state,
        user.id,
        user.email,
        is_admin,
        jar,
        Some(message),
        None,
    )
    .await
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
    let feeds = feeds::list_for_user(state.db.pool(), user.id).await?;
    let mut queued = 0u64;
    for feed in &feeds {
        if enqueue_fetch_feed(&state.db, feed.id).await?.is_some() {
            queued += 1;
        }
    }
    let is_admin = user.role == "admin";
    render_page(
        &state,
        user.id,
        user.email,
        is_admin,
        jar,
        Some(format!(
            "Queued {queued} of {} feed refreshes.",
            feeds.len()
        )),
        None,
    )
    .await
}

/// `GET /feeds/{id}/settings`.
pub async fn settings_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    if !subscriptions::is_subscribed(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    let feed = feeds::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    if user.role != "admin" {
        return render_page(
            &state,
            user.id,
            user.email,
            false,
            jar,
            None,
            Some("Only an administrator can change this shared feed policy.".to_string()),
        )
        .await;
    }
    render_settings_page(&state, user.id, user.email, jar, &feed, None, None).await
}

/// `POST /feeds/{id}/settings`.
pub async fn update_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ContentSettingsForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if user.role != "admin" {
        return Err(AppError::Forbidden);
    }
    if !subscriptions::is_subscribed(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    let feed = feeds::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let mode = match FeedContentMode::parse(form.content_mode.trim()) {
        Some(mode) => mode,
        None => {
            return render_settings_page(
                &state,
                user.id,
                user.email,
                jar,
                &feed,
                None,
                Some("Choose a valid content source.".to_string()),
            )
            .await;
        }
    };
    let fetch_interval_minutes = match parse_interval(&form.fetch_interval_minutes) {
        Ok(value) => value,
        Err(error) => {
            return render_settings_page(
                &state,
                user.id,
                user.email,
                jar,
                &feed,
                None,
                Some(error),
            )
            .await;
        }
    };
    let extraction_delay_ms = match parse_delay(&form.extraction_delay_ms) {
        Ok(value) => value,
        Err(error) => {
            return render_settings_page(
                &state,
                user.id,
                user.email,
                jar,
                &feed,
                None,
                Some(error),
            )
            .await;
        }
    };
    let selector = (!form.selector.trim().is_empty()).then(|| form.selector.trim().to_string());
    let settings = FeedContentSettings {
        mode,
        selector: if mode.fetches_article_pages() {
            selector
        } else {
            None
        },
    };
    feeds::update_content_settings(
        state.db.pool(),
        id,
        &settings.to_json(),
        form.respect_robots_txt.is_some(),
        if mode.fetches_article_pages() {
            extraction_delay_ms
        } else {
            None
        },
        fetch_interval_minutes,
    )
    .await?;

    let reset = if mode.fetches_article_pages() {
        articles::reset_extraction_for_feed(state.db.pool(), id).await?
    } else {
        articles::restore_feed_content(state.db.pool(), id).await?
    };
    let queued = enqueue_fetch_feed(&state.db, id).await?.is_some();
    let updated = feeds::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let policy_message = if mode.fetches_article_pages() {
        format!("Shared source policy saved. Queued public-page processing; reset {reset} failed fallback(s).")
    } else {
        format!("Shared source policy saved. Restored RSS fallback for {reset} article(s).")
    };
    let message = if queued {
        policy_message
    } else {
        format!("{policy_message} A refresh is already queued.")
    };
    render_settings_page(
        &state,
        user.id,
        user.email,
        jar,
        &updated,
        Some(message),
        None,
    )
    .await
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
