//! The reading experience: the article list. (M5 start — reading modes, HTMX
//! infinite scroll, and per-article actions follow.)

use askama::Template;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Response};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;

use rustss_db::articles;

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

struct ArticleView {
    title: String,
    url: String,
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
    has_prev: bool,
    has_next: bool,
    prev_url: String,
    next_url: String,
}

#[derive(Deserialize)]
pub struct ArticlesQuery {
    #[serde(default)]
    unread: Option<String>,
    #[serde(default)]
    offset: Option<i64>,
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

    // Fetch one extra row to know whether there's a next page.
    let mut items =
        articles::list_for_user(state.db.pool(), user.id, unread_only, per_page + 1, offset)
            .await?;
    let has_next = items.len() as i64 > per_page;
    items.truncate(per_page as usize);

    let article_views = items
        .into_iter()
        .map(|a| ArticleView {
            title: a.title,
            url: a.url,
            excerpt: a.excerpt,
            feed_name: a.feed_name,
            published: a.published_at.map(|d| d.date().to_string()),
            is_read: a.is_read,
            is_bookmarked: a.is_bookmarked,
        })
        .collect();

    let suffix = if unread_param { "&unread=1" } else { "" };
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);

    let body = ArticlesTemplate {
        theme: prefs.theme,
        user_email: user.email,
        csrf_token,
        articles: article_views,
        unread_only,
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
