//! The reading experience: article list, per-article actions (read/bookmark),
//! and article reading (side panel, standalone page, and inline fragment).

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use rustss_core::feed_content::{FeedContentMode, FeedContentSettings};
use rustss_core::html::sanitize_article_html;
use rustss_db::{articles, bookmarks, feedback, feeds, jobs, patterns, read_articles};
use rustss_search::personalize;
use rustss_search::{extract_keywords, relevance_score, ExplicitFeedback};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::extract::Ajax;
use crate::prefs;
use crate::state::AppState;

/// Candidate window scored when sorting by relevance.
const RELEVANCE_CANDIDATES: i64 = 120;
/// Default implicit bounce threshold (ARCH §11.2).
const BOUNCE_THRESHOLD: f64 = 0.25;

struct ArticleView {
    id: String,
    title: String,
    excerpt: Option<String>,
    image_url: Option<String>,
    feed_name: String,
    published: Option<String>,
    is_read: bool,
    is_bookmarked: bool,
    is_liked: bool,
    is_disliked: bool,
    score: Option<i64>,
}

struct FeedNav {
    id: String,
    name: String,
    active: bool,
}

#[derive(Template)]
#[template(path = "articles.html")]
struct ArticlesTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    articles: Vec<ArticleView>,
    feeds: Vec<FeedNav>,
    unread_only: bool,
    reading_mode: String,
    side_panel: bool,
    sort: String,
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
    ui: prefs::Preferences,
    id: String,
    title: String,
    feed_name: String,
    url: String,
    published: Option<String>,
    image_url: Option<String>,
    content: String, // sanitized HTML
    content_notice: Option<String>,
    is_read: bool,
    is_bookmarked: bool,
    is_liked: bool,
    is_disliked: bool,
    summary: Option<String>,
    key_points: Vec<String>,
    topics: Vec<String>,
    can_summarize: bool,
}

#[derive(Template)]
#[template(path = "article_panel.html")]
struct ArticlePanelTemplate {
    csrf_token: String,
    id: String,
    title: String,
    feed_name: String,
    url: String,
    published: Option<String>,
    image_url: Option<String>,
    content: String,
    content_notice: Option<String>,
    is_read: bool,
    is_bookmarked: bool,
    is_liked: bool,
    is_disliked: bool,
    summary: Option<String>,
    key_points: Vec<String>,
    topics: Vec<String>,
    can_summarize: bool,
}

struct ReaderView {
    id: String,
    title: String,
    feed_name: String,
    url: String,
    published: Option<String>,
    image_url: Option<String>,
    content: String,
    content_notice: Option<String>,
    is_read: bool,
    is_bookmarked: bool,
    is_liked: bool,
    is_disliked: bool,
    summary: Option<String>,
    key_points: Vec<String>,
    topics: Vec<String>,
    can_summarize: bool,
}

#[derive(Deserialize)]
pub struct ArticlesQuery {
    #[serde(default)]
    unread: Option<String>,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    feed: Option<Uuid>,
    /// `recent` (default) or `relevance`.
    #[serde(default)]
    sort: Option<String>,
}

#[derive(Deserialize)]
pub struct FeedbackForm {
    csrf_token: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    time_spent: Option<f64>,
    #[serde(default)]
    next: Option<String>,
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

fn safe_media_url(value: Option<&str>) -> Option<String> {
    let value = value?;
    let url = url::Url::parse(value).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
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
    let feed_id = q.feed;
    let sort = if q.sort.as_deref() == Some("relevance") {
        "relevance"
    } else {
        "recent"
    };

    let (article_views, has_next) = if sort == "relevance" {
        relevance_page(&state, user.id, feed_id, unread_only, per_page, offset).await?
    } else {
        recency_page(&state, user.id, feed_id, unread_only, per_page, offset).await?
    };

    let side_panel = prefs.reading_mode == "side-panel" && !article_views.is_empty();
    let feeds = feeds::list_for_user(state.db.pool(), user.id)
        .await?
        .into_iter()
        .map(|f| FeedNav {
            active: Some(f.id) == feed_id,
            id: f.id.to_string(),
            name: f.name,
        })
        .collect();

    // Preserve the active filters in pagination/back links.
    let mut suffix = String::new();
    if unread_param {
        suffix.push_str("&unread=1");
    }
    if let Some(fid) = feed_id {
        suffix.push_str(&format!("&feed={fid}"));
    }
    if sort == "relevance" {
        suffix.push_str("&sort=relevance");
    }
    let back_url = format!("/articles?offset={offset}{suffix}");
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);

    let body = ArticlesTemplate {
        theme: prefs.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui: prefs.clone(),
        articles: article_views,
        feeds,
        unread_only,
        reading_mode: prefs.reading_mode,
        side_panel,
        sort: sort.to_string(),
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

/// Recency-ordered page (newest first), fetching one extra row for `has_next`.
async fn recency_page(
    state: &AppState,
    user_id: Uuid,
    feed_id: Option<Uuid>,
    unread_only: bool,
    per_page: i64,
    offset: i64,
) -> Result<(Vec<ArticleView>, bool), AppError> {
    let mut items = articles::list_for_user(
        state.db.pool(),
        user_id,
        feed_id,
        unread_only,
        per_page + 1,
        offset,
    )
    .await?;
    let has_next = items.len() as i64 > per_page;
    items.truncate(per_page as usize);
    let views = items
        .into_iter()
        .map(|a| ArticleView {
            id: a.id.to_string(),
            title: a.title,
            excerpt: a.excerpt,
            image_url: safe_media_url(a.image_url.as_deref()),
            feed_name: a.feed_name,
            published: a.published_at.map(|d| d.date().to_string()),
            is_read: a.is_read,
            is_bookmarked: a.is_bookmarked,
            is_liked: a.feedback_value.is_some_and(|value| value > 0.0),
            is_disliked: a.feedback_value.is_some_and(|value| value < 0.0),
            score: None,
        })
        .collect();
    Ok((views, has_next))
}

/// Relevance-ranked page: score a recent candidate window against the user's
/// patterns (one batched weights lookup), sort by score, then paginate.
async fn relevance_page(
    state: &AppState,
    user_id: Uuid,
    feed_id: Option<Uuid>,
    unread_only: bool,
    per_page: i64,
    offset: i64,
) -> Result<(Vec<ArticleView>, bool), AppError> {
    let candidates = articles::list_for_user(
        state.db.pool(),
        user_id,
        feed_id,
        unread_only,
        RELEVANCE_CANDIDATES,
        0,
    )
    .await?;

    // Extract keywords per candidate (title + excerpt) and look up all weights
    // in a single query.
    let keywords: Vec<_> = candidates
        .iter()
        .map(|a| {
            let text = format!("{} {}", a.title, a.excerpt.as_deref().unwrap_or(""));
            extract_keywords(&text, 30)
        })
        .collect();
    let mut all_terms: Vec<String> = keywords
        .iter()
        .flat_map(|ks| ks.iter().map(|k| k.term.clone()))
        .collect();
    all_terms.sort();
    all_terms.dedup();
    let weights: std::collections::HashMap<String, f64> =
        patterns::weights_for(state.db.pool(), user_id, &all_terms)
            .await?
            .into_iter()
            .collect();

    let mut scored: Vec<(articles::ArticleListItem, f64)> = candidates
        .into_iter()
        .zip(keywords)
        .map(|(a, ks)| {
            let total: f64 = ks
                .iter()
                .filter_map(|k| weights.get(&k.term).map(|w| w * k.relevance))
                .sum();
            (a, relevance_score(total))
        })
        .collect();
    scored.sort_by(|x, y| {
        y.1.partial_cmp(&x.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| y.0.published_at.cmp(&x.0.published_at))
    });

    let start = offset as usize;
    let has_next = scored.len() > start + per_page as usize;
    let views = scored
        .into_iter()
        .skip(start)
        .take(per_page as usize)
        .map(|(a, score)| ArticleView {
            id: a.id.to_string(),
            title: a.title,
            excerpt: a.excerpt,
            image_url: safe_media_url(a.image_url.as_deref()),
            feed_name: a.feed_name,
            published: a.published_at.map(|d| d.date().to_string()),
            is_read: a.is_read,
            is_bookmarked: a.is_bookmarked,
            is_liked: a.feedback_value.is_some_and(|value| value > 0.0),
            is_disliked: a.feedback_value.is_some_and(|value| value < 0.0),
            score: Some((score.clamp(0.0, 1.0) * 100.0).round() as i64),
        })
        .collect();
    Ok((views, has_next))
}

/// `POST /articles/{id}/feedback` — explicit 👍/👎 or an implicit reading signal.
pub async fn feedback(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<FeedbackForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    authorize_action(&state, user.id, id).await?;
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;

    match form.value.as_deref() {
        Some("up") => {
            personalize::record_explicit(
                &state.db,
                &state.cache,
                user.id,
                &article,
                ExplicitFeedback::Up,
            )
            .await?;
        }
        Some("down") => {
            personalize::record_explicit(
                &state.db,
                &state.cache,
                user.id,
                &article,
                ExplicitFeedback::Down,
            )
            .await?;
        }
        _ => {
            if let Some(time_spent) = form.time_spent {
                let estimated = estimate_reading_seconds(&article.content);
                personalize::record_implicit(
                    &state.db,
                    &state.cache,
                    user.id,
                    &article,
                    time_spent,
                    estimated,
                    BOUNCE_THRESHOLD,
                )
                .await?;
            }
        }
    }
    Ok(action_response(is_ajax, form.next))
}

/// Estimate reading time (seconds) at ~200 wpm.
fn estimate_reading_seconds(content: &str) -> f64 {
    let words = content.split_whitespace().count().max(1) as f64;
    words / 200.0 * 60.0
}

/// `POST /articles/{id}/summarize` — enqueue a (deduped) summary job.
pub async fn summarize(
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
    let dedup = format!("summarize:{id}");
    jobs::enqueue(
        state.db.pool(),
        "summarize_article",
        serde_json::json!({ "article_id": id, "user_id": user.id }),
        Some(&dedup),
        None,
        3,
    )
    .await?;
    Ok(action_response(is_ajax, form.next))
}

async fn reader_view(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
    auto_mark_read: bool,
) -> Result<ReaderView, AppError> {
    if !articles::user_can_access(state.db.pool(), user_id, id).await? {
        return Err(AppError::NotFound);
    }
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    if auto_mark_read {
        read_articles::mark_read(state.db.pool(), user_id, id).await?;
    }

    let feed = feeds::find_by_id(state.db.pool(), article.feed_id).await?;
    let feed_name = feed
        .as_ref()
        .map(|feed| feed.name.clone())
        .unwrap_or_default();
    let content_mode = FeedContentSettings::from_json(
        feed.as_ref()
            .and_then(|feed| feed.extraction_settings.as_ref()),
    )
    .mode;
    let content_notice = match (content_mode, article.content_source.as_str(), article.extraction_status.as_str()) {
        (FeedContentMode::Paywalled, _, _) => Some(
            "This feed is configured for paywalled/RSS fallback. RustSS shows the feed-provided content and does not attempt to bypass publisher access controls.".to_string(),
        ),
        (FeedContentMode::ArticlePage, "feed", "none") => Some(
            "Full article content is queued for this source. Showing feed-provided content until extraction finishes.".to_string(),
        ),
        (FeedContentMode::ArticlePage, "feed", "failed") => Some(
            "Full article content could not be downloaded. Showing the feed-provided fallback.".to_string(),
        ),
        _ => None,
    };
    let stored_summary = articles::get_summary(state.db.pool(), id).await?;
    let can_summarize = state.settings.summarization_enabled && stored_summary.is_none();
    let (summary, key_points, topics) = match stored_summary {
        Some(s) => (Some(s.summary), s.key_points, s.topics),
        None => (None, Vec::new(), Vec::new()),
    };
    let feedback_value = feedback::get(state.db.pool(), user_id, id)
        .await?
        .map(|record| record.feedback_value);

    Ok(ReaderView {
        id: id.to_string(),
        title: article.title,
        feed_name,
        image_url: safe_media_url(article.image_url.as_deref()),
        url: article.url.clone(),
        published: article.published_at.map(|d| d.date().to_string()),
        content: sanitize_article_html(&article.content, &article.url),
        content_notice,
        is_read: read_articles::is_read(state.db.pool(), user_id, id).await?,
        is_bookmarked: bookmarks::is_bookmarked(state.db.pool(), user_id, id).await?,
        is_liked: feedback_value.is_some_and(|value| value > 0.0),
        is_disliked: feedback_value.is_some_and(|value| value < 0.0),
        summary,
        key_points,
        topics,
        can_summarize,
    })
}

/// `GET /articles/{id}` — standalone reading view.
pub async fn read(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let prefs = prefs::load(&state.db, user.id).await;
    let article = reader_view(&state, user.id, id, prefs.auto_mark_read).await?;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = ArticleTemplate {
        theme: prefs.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui: prefs,
        id: article.id,
        title: article.title,
        feed_name: article.feed_name,
        url: article.url,
        published: article.published,
        image_url: article.image_url,
        content: article.content,
        content_notice: article.content_notice,
        is_read: article.is_read,
        is_bookmarked: article.is_bookmarked,
        is_liked: article.is_liked,
        is_disliked: article.is_disliked,
        summary: article.summary,
        key_points: article.key_points,
        topics: article.topics,
        can_summarize: article.can_summarize,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, body).into_response())
}

pub async fn side_panel(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let prefs = prefs::load(&state.db, user.id).await;
    let article = reader_view(&state, user.id, id, prefs.auto_mark_read).await?;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = ArticlePanelTemplate {
        csrf_token,
        id: article.id,
        title: article.title,
        feed_name: article.feed_name,
        url: article.url,
        published: article.published,
        image_url: article.image_url,
        content: article.content,
        content_notice: article.content_notice,
        is_read: article.is_read,
        is_bookmarked: article.is_bookmarked,
        is_liked: article.is_liked,
        is_disliked: article.is_disliked,
        summary: article.summary,
        key_points: article.key_points,
        topics: article.topics,
        can_summarize: article.can_summarize,
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
    Ok(Html(sanitize_article_html(&article.content, &article.url)).into_response())
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
