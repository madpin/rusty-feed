//! Saved-search management + results, and the notifications page (M11).

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use rusty_feed_db::{jobs, notifications, saved_search_matches, saved_searches};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

struct SearchRow {
    id: String,
    name: String,
    query: String,
    notify: bool,
}

#[derive(Template)]
#[template(path = "searches.html")]
struct SearchesTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    searches: Vec<SearchRow>,
    error: Option<String>,
}

struct ResultRow {
    id: String,
    title: String,
    excerpt: Option<String>,
    feed_name: String,
    published: Option<String>,
    score: i64,
}

#[derive(Template)]
#[template(path = "search_results.html")]
struct ResultsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    id: String,
    name: String,
    query: String,
    notify: bool,
    results: Vec<ResultRow>,
    previous_url: Option<String>,
    next_url: Option<String>,
}

#[derive(Template)]
#[template(path = "notifications.html")]
struct NotificationsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    notifications: Vec<NotificationRow>,
}

struct NotificationRow {
    title: String,
    message: String,
    created: String,
    read: bool,
}

#[derive(Deserialize)]
pub struct CreateForm {
    csrf_token: String,
    name: String,
    query: String,
    #[serde(default)]
    notify: Option<String>,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct UpdateForm {
    csrf_token: String,
    name: String,
    query: String,
    #[serde(default)]
    notify: Option<String>,
}

#[derive(Default, Deserialize)]
pub struct ResultsQuery {
    #[serde(default = "first_page")]
    page: u32,
}

fn first_page() -> u32 {
    1
}

fn render<T: Template>(tpl: &T) -> Result<Html<String>, AppError> {
    tpl.render()
        .map(Html)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))
}

async fn searches_page(
    state: &AppState,
    user_id: Uuid,
    user_email: String,
    is_admin: bool,
    jar: CookieJar,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user_id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let searches = saved_searches::list_for_user(state.db.pool(), user_id)
        .await?
        .into_iter()
        .map(|s| SearchRow {
            id: s.id.to_string(),
            name: s.name,
            query: s.query,
            notify: s.notify_on_match,
        })
        .collect();
    let body = render(&SearchesTemplate {
        theme: ui.theme.clone(),
        user_email,
        csrf_token,
        ui,
        is_admin,
        searches,
        error,
    })?;
    Ok((jar, body).into_response())
}

/// `GET /searches`.
pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let is_admin = user.role == "admin";
    searches_page(&state, user.id, user.email, is_admin, jar, None).await
}

/// `POST /searches` — create a saved search and kick off an immediate match.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<CreateForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    crate::quota::assert_within(
        &state,
        user.id,
        rusty_feed_core::quotas::QuotaKind::SavedSearches,
    )
    .await?;
    // Demo accounts get a much tighter saved-search cap (§7.4).
    if user.is_demo {
        let count = rusty_feed_db::saved_searches::count_for_user(state.db.pool(), user.id).await?;
        if count >= 3 {
            return Err(AppError::QuotaExceeded(
                "Demo accounts are limited to 3 saved searches.".to_string(),
            ));
        }
    }
    let is_admin = user.role == "admin";
    let name = form.name.trim();
    let query = form.query.trim();
    if name.is_empty() || query.is_empty() {
        return searches_page(
            &state,
            user.id,
            user.email,
            is_admin,
            jar,
            Some("Name and query are required.".into()),
        )
        .await;
    }

    let ss = saved_searches::create(
        state.db.pool(),
        user.id,
        name,
        query,
        0.6,
        form.notify.is_some(),
        0.85,
    )
    .await?;

    // Match now (deduped) so results appear promptly.
    let dedup = format!("saved_search_match:{}", ss.id);
    jobs::enqueue(
        state.db.pool(),
        "saved_search_match",
        json!({ "saved_search_id": ss.id }),
        Some(&dedup),
        None,
        3,
    )
    .await?;

    Ok((jar, Redirect::to("/searches")).into_response())
}

/// `POST /searches/{id}/delete`.
pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    saved_searches::delete(state.db.pool(), id, user.id).await?;
    Ok((jar, Redirect::to("/searches")).into_response())
}

/// `POST /searches/{id}/run` — enqueue an immediate match pass.
pub async fn run(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if saved_searches::get_for_user(state.db.pool(), id, user.id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound);
    }
    let dedup = format!("saved_search_match:{id}");
    jobs::enqueue(
        state.db.pool(),
        "saved_search_match",
        json!({ "saved_search_id": id }),
        Some(&dedup),
        None,
        3,
    )
    .await?;
    Ok((jar, Redirect::to(&format!("/searches/{id}"))).into_response())
}

/// `POST /searches/{id}` — edit an owned saved search and enqueue a full
/// rematch so results reflect the new expression immediately.
pub async fn update(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<UpdateForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let name = form.name.trim();
    let query = form.query.trim();
    if name.is_empty() || query.is_empty() {
        return Err(AppError::Validation("Name and query are required.".into()));
    }
    if !saved_searches::update(
        state.db.pool(),
        id,
        user.id,
        name,
        query,
        form.notify.is_some(),
    )
    .await?
    {
        return Err(AppError::NotFound);
    }
    saved_search_matches::clear_for_search(state.db.pool(), id).await?;
    enqueue_match(&state, id).await?;
    Ok((jar, Redirect::to(&format!("/searches/{id}"))).into_response())
}

/// `POST /searches/{id}/rematch` — clear stale matches and run from scratch.
pub async fn rematch(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if saved_searches::get_for_user(state.db.pool(), id, user.id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound);
    }
    saved_search_matches::clear_for_search(state.db.pool(), id).await?;
    enqueue_match(&state, id).await?;
    Ok((jar, Redirect::to(&format!("/searches/{id}"))).into_response())
}

async fn enqueue_match(state: &AppState, id: Uuid) -> Result<(), AppError> {
    let dedup = format!("saved_search_match:{id}");
    jobs::enqueue(
        state.db.pool(),
        "saved_search_match",
        json!({ "saved_search_id": id }),
        Some(&dedup),
        None,
        3,
    )
    .await?;
    Ok(())
}

/// `GET /searches/{id}` — match results.
pub async fn results(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Query(params): Query<ResultsQuery>,
) -> Result<Response, AppError> {
    let ss = saved_searches::get_for_user(state.db.pool(), id, user.id)
        .await?
        .ok_or(AppError::NotFound)?;
    const PAGE_SIZE: i64 = 25;
    let page = params.page.clamp(1, 10_000);
    let offset = i64::from(page - 1) * PAGE_SIZE;
    let mut matches =
        saved_search_matches::list_for_search(state.db.pool(), id, PAGE_SIZE + 1, offset).await?;
    let has_next = matches.len() > PAGE_SIZE as usize;
    matches.truncate(PAGE_SIZE as usize);
    let results = matches
        .into_iter()
        .map(|m| ResultRow {
            id: m.id.to_string(),
            title: m.title,
            excerpt: m.excerpt,
            feed_name: m.feed_name,
            published: m.published_at.map(|d| d.date().to_string()),
            score: (m.relevance_score.clamp(0.0, 1.0) * 100.0).round() as i64,
        })
        .collect();
    let ui = prefs::load(&state.db, user.id).await;
    let is_admin = user.role == "admin";
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = render(&ResultsTemplate {
        theme: ui.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui,
        is_admin,
        id: id.to_string(),
        name: ss.name,
        query: ss.query,
        notify: ss.notify_on_match,
        results,
        previous_url: (page > 1).then(|| format!("/searches/{id}?page={}", page - 1)),
        next_url: has_next.then(|| format!("/searches/{id}?page={}", page + 1)),
    })?;
    Ok((jar, body).into_response())
}

/// `GET /notifications`.
pub async fn notifications_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let items = notifications::list(state.db.pool(), user.id, 50)
        .await?
        .into_iter()
        .map(|n| NotificationRow {
            title: n.title,
            message: n.message,
            created: n.created_at.date().to_string(),
            read: n.read,
        })
        .collect();
    let ui = prefs::load(&state.db, user.id).await;
    let is_admin = user.role == "admin";
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = render(&NotificationsTemplate {
        theme: ui.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui,
        is_admin,
        notifications: items,
    })?;
    Ok((jar, body).into_response())
}

/// Small polling fragment used by the application shell. It carries no
/// notification content and remains useful when HTMX is unavailable.
pub async fn notification_count(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html<String>, AppError> {
    let count = notifications::unread_count(state.db.pool(), user.id).await?;
    let label = if count == 0 {
        "No unread alerts".to_string()
    } else if count == 1 {
        "1 unread alert".to_string()
    } else {
        format!("{count} unread alerts")
    };
    let value = if count == 0 {
        String::new()
    } else if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    };
    Ok(Html(format!(
        "<span class=\"notification-badge\" aria-label=\"{label}\">{value}</span>"
    )))
}

/// `POST /notifications/read` — mark all read.
pub async fn mark_read(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    notifications::mark_all_read(state.db.pool(), user.id).await?;
    Ok((jar, Redirect::to("/notifications")).into_response())
}
