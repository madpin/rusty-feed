//! The reading experience: article list, per-article actions (read/bookmark),
//! and article reading (standalone page + inline fragment).

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use rustss_db::{articles, bookmarks, feeds, read_articles};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::extract::Ajax;
use crate::prefs;
use crate::state::AppState;

struct ArticleView {
    id: String,
    title: String,
    excerpt: Option<String>,
    feed_name: String,
    published: Option<String>,
    is_read: bool,
    is_bookmarked: bool,
}

#[derive(Template)]
#[template(path = "articles.html")]
struct ArticlesTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    articles: Vec<ArticleView>,
    unread_only: bool,
    reading_mode: String,
    back_url: String,
    has_prev: bool,
    has_next: bool,
    prev_url: String,
    next_url: String,
}

#[derive(Template)]
#[template(path = "article.html")]
struct ArticleTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    id: String,
    title: String,
    feed_name: String,
    url: String,
    published: Option<String>,
    content: String, // sanitized HTML
    is_read: bool,
    is_bookmarked: bool,
}

#[derive(Deserialize)]
pub struct ArticlesQuery {
    #[serde(default)]
    unread: Option<String>,
    #[serde(default)]
    offset: Option<i64>,
}

#[derive(Deserialize)]
pub struct ActionForm {
    csrf_token: String,
    #[serde(default)]
    next: Option<String>,
}

/// Only allow same-origin relative redirects (avoid open-redirects).
fn safe_next(next: Option<String>) -> String {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") => n,
        _ => "/articles".to_string(),
    }
}

/// `GET /articles` — the user's reading list.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Query(q): Query<ArticlesQuery>,
) -> Result<Response, AppError> {
    let prefs = prefs::load(&state.db, user.id).await;
    let per_page = prefs.articles_per_page.clamp(5, 100) as i64;
    let unread_param = q.unread.as_deref() == Some("1");
    let unread_only = unread_param || !prefs.show_read_articles;
    let offset = q.offset.unwrap_or(0).max(0);

    let mut items =
        articles::list_for_user(state.db.pool(), user.id, unread_only, per_page + 1, offset)
            .await?;
    let has_next = items.len() as i64 > per_page;
    items.truncate(per_page as usize);

    let article_views = items
        .into_iter()
        .map(|a| ArticleView {
            id: a.id.to_string(),
            title: a.title,
            excerpt: a.excerpt,
            feed_name: a.feed_name,
            published: a.published_at.map(|d| d.date().to_string()),
            is_read: a.is_read,
            is_bookmarked: a.is_bookmarked,
        })
        .collect();

    let suffix = if unread_param { "&unread=1" } else { "" };
    let back_url = format!("/articles?offset={offset}{suffix}");
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);

    let body = ArticlesTemplate {
        theme: prefs.theme,
        user_email: user.email,
        csrf_token,
        articles: article_views,
        unread_only,
        reading_mode: prefs.reading_mode,
        back_url,
        has_prev: offset > 0,
        has_next,
        prev_url: format!("/articles?offset={}{suffix}", (offset - per_page).max(0)),
        next_url: format!("/articles?offset={}{suffix}", offset + per_page),
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;

    Ok((jar, body).into_response())
}

/// `GET /articles/{id}` — standalone reading view.
pub async fn read(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    if !articles::user_can_access(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;

    let prefs = prefs::load(&state.db, user.id).await;
    if prefs.auto_mark_read {
        read_articles::mark_read(state.db.pool(), user.id, id).await?;
    }

    let feed_name = feeds::find_by_id(state.db.pool(), article.feed_id)
        .await?
        .map(|f| f.name)
        .unwrap_or_default();
    let is_bookmarked = bookmarks::is_bookmarked(state.db.pool(), user.id, id).await?;

    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = ArticleTemplate {
        theme: prefs.theme,
        user_email: user.email,
        csrf_token,
        id: id.to_string(),
        title: article.title,
        feed_name,
        url: article.url,
        published: article.published_at.map(|d| d.date().to_string()),
        content: ammonia::clean(&article.content),
        is_read: prefs.auto_mark_read
            || read_articles::is_read(state.db.pool(), user.id, id).await?,
        is_bookmarked,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, body).into_response())
}

/// `GET /articles/{id}/content` — sanitized content fragment (for inline mode).
pub async fn content_fragment(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    if !articles::user_can_access(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let prefs = prefs::load(&state.db, user.id).await;
    if prefs.auto_mark_read {
        read_articles::mark_read(state.db.pool(), user.id, id).await?;
    }
    Ok(Html(ammonia::clean(&article.content)).into_response())
}

async fn authorize_action(state: &AppState, user_id: Uuid, id: Uuid) -> Result<(), AppError> {
    if articles::user_can_access(state.db.pool(), user_id, id).await? {
        Ok(())
    } else {
        Err(AppError::NotFound)
    }
}

/// Actions return `204` to our fetch client (which updates the UI in place) and
/// a redirect for plain form posts (no-JS).
fn action_response(is_ajax: bool, next: Option<String>) -> Response {
    if is_ajax {
        axum::http::StatusCode::NO_CONTENT.into_response()
    } else {
        Redirect::to(&safe_next(next)).into_response()
    }
}

/// `POST /articles/{id}/read`.
pub async fn mark_read(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ActionForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    authorize_action(&state, user.id, id).await?;
    read_articles::mark_read(state.db.pool(), user.id, id).await?;
    Ok(action_response(is_ajax, form.next))
}

/// `POST /articles/{id}/unread`.
pub async fn mark_unread(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ActionForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    authorize_action(&state, user.id, id).await?;
    read_articles::mark_unread(state.db.pool(), user.id, id).await?;
    Ok(action_response(is_ajax, form.next))
}

/// `POST /articles/{id}/bookmark`.
pub async fn bookmark(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ActionForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    authorize_action(&state, user.id, id).await?;
    bookmarks::add(state.db.pool(), user.id, id).await?;
    Ok(action_response(is_ajax, form.next))
}

/// `POST /articles/{id}/unbookmark`.
pub async fn unbookmark(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ActionForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    authorize_action(&state, user.id, id).await?;
    bookmarks::remove(state.db.pool(), user.id, id).await?;
    Ok(action_response(is_ajax, form.next))
}

/// `POST /articles/read-all`.
pub async fn mark_all_read(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Form(form): Form<ActionForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    read_articles::mark_all_read(state.db.pool(), user.id).await?;
    Ok(action_response(is_ajax, form.next))
}
