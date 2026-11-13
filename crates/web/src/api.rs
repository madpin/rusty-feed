//! Versioned native JSON resources authenticated by revocable API tokens.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use rusty_feed_db::{
    api_audit, api_tokens, articles, bookmarks, categories, feeds, jobs, read_articles,
    saved_searches, subscriptions, User,
};
use rusty_feed_feeds::{enqueue_fetch_feed, normalize_url};

use crate::auth::token;
use crate::error::AppError;
use crate::state::AppState;

async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    write: bool,
    operation: &'static str,
) -> Result<User, AppError> {
    let raw = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .or_else(|| {
            headers
                .get("x-auth-token")
                .and_then(|value| value.to_str().ok())
        })
        .filter(|value| !value.is_empty())
        .ok_or(AppError::Unauthorized)?;
    let api_token = api_tokens::find_active_by_hash(state.db.pool(), &token::hash(raw))
        .await?
        .ok_or(AppError::Unauthorized)?;
    if !state.api_limiter.check(&api_token.id.to_string()).await {
        return Err(AppError::RateLimited);
    }
    let allowed = api_token.scopes.is_empty()
        || api_token
            .scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "api" | "write") || (!write && scope == "read"));
    if !allowed {
        return Err(AppError::Forbidden);
    }
    api_tokens::touch(state.db.pool(), api_token.id).await?;
    let user = rusty_feed_db::users::find_by_id(state.db.pool(), api_token.user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    if write {
        api_audit::record(state.db.pool(), user.id, api_token.id, operation).await?;
    }
    Ok(user)
}

pub async fn openapi() -> Json<Value> {
    Json(json!({
        "openapi": "3.1.0",
        "info": {"title": "Rusty Feed native API", "version": "v1"},
        "servers": [{"url": "/api/v1"}],
        "components": {"securitySchemes": {
            "bearer": {"type": "http", "scheme": "bearer"},
            "xAuthToken": {"type": "apiKey", "in": "header", "name": "X-Auth-Token"}
        }},
        "security": [{"bearer": []}, {"xAuthToken": []}],
        "paths": {
            "/user": {"get": {"summary": "Current user"}},
            "/feeds": {"get": {"summary": "List subscriptions"}, "post": {"summary": "Subscribe"}},
            "/feeds/{id}": {"delete": {"summary": "Unsubscribe"}},
            "/feeds/{id}/refresh": {"post": {"summary": "Queue a shared refresh"}},
            "/articles": {"get": {"summary": "Paginated personal library"}},
            "/articles/{id}": {"get": {"summary": "Article detail"}},
            "/articles/{id}/state": {"post": {"summary": "Update read/bookmark state"}},
            "/categories": {"get": {"summary": "List folders"}, "post": {"summary": "Create a folder"}},
            "/saved-searches": {"get": {"summary": "List searches"}, "post": {"summary": "Create a search"}},
            "/saved-searches/{id}": {"delete": {"summary": "Delete a search"}}
        }
    }))
}

pub async fn version() -> Json<Value> {
    Json(json!({"data": {"version": env!("CARGO_PKG_VERSION"), "api": "v1"}}))
}

pub async fn user(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    let user = authenticate(&state, &headers, false, "user.read").await?;
    Ok(Json(json!({
        "data": { "id": user.id, "email": user.email, "name": user.name, "role": user.role }
    })))
}

pub async fn feeds_list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    let user = authenticate(&state, &headers, false, "feeds.list").await?;
    let data: Vec<_> = feeds::list_for_user(state.db.pool(), user.id)
        .await?
        .into_iter()
        .map(|feed| {
            json!({
                "id": feed.id,
                "url": feed.url,
                "name": feed.name,
                "site_url": feed.site_url,
                "description": feed.description,
                "health": feed.health_status,
                "last_successful_fetch_at": feed.last_successful_fetch_at,
            })
        })
        .collect();
    Ok(Json(json!({ "data": data })))
}

#[derive(Deserialize)]
pub struct CreateFeed {
    url: String,
    name: Option<String>,
}

pub async fn feed_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CreateFeed>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let user = authenticate(&state, &headers, true, "feeds.subscribe").await?;
    crate::verify::require_verified(&state, &user)?;
    crate::quota::assert_within(
        &state,
        user.id,
        rusty_feed_core::quotas::QuotaKind::Subscriptions,
    )
    .await?;
    let url = normalize_url(input.url.trim())
        .map_err(|_| AppError::Validation("feed URL is invalid".into()))?;
    state
        .fetch
        .validate_url(&url)
        .map_err(|_| AppError::Validation("feed URL is not allowed".into()))?;
    crate::moderation::assert_feed_allowed(&state, &url).await?;
    let host = url::Url::parse(&url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .ok_or_else(|| AppError::Validation("feed URL has no host".into()))?;
    let name = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&host);
    let (feed, _) = feeds::upsert_by_url(state.db.pool(), &url, name, None).await?;
    subscriptions::subscribe(state.db.pool(), user.id, feed.id, None).await?;
    enqueue_fetch_feed(&state.db, feed.id).await?;
    Ok((StatusCode::CREATED, Json(json!({"data": {"id": feed.id}}))))
}

pub async fn feed_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let user = authenticate(&state, &headers, true, "feeds.unsubscribe").await?;
    if !subscriptions::unsubscribe(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn feed_refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let user = authenticate(&state, &headers, true, "feeds.refresh").await?;
    if !subscriptions::is_subscribed(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    enqueue_fetch_feed(&state.db, id).await?;
    Ok(StatusCode::ACCEPTED)
}

pub async fn categories_list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    let user = authenticate(&state, &headers, false, "categories.list").await?;
    let data = categories::list_with_feed_counts(state.db.pool(), user.id)
        .await?
        .into_iter()
        .map(|category| {
            json!({
                "id": category.id,
                "name": category.name,
                "parent_id": category.parent_id,
                "feed_count": category.feed_count,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({"data": data})))
}

#[derive(Deserialize)]
pub struct CreateCategory {
    name: String,
    parent_id: Option<Uuid>,
}

pub async fn category_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CreateCategory>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let user = authenticate(&state, &headers, true, "categories.create").await?;
    crate::quota::assert_within(
        &state,
        user.id,
        rusty_feed_core::quotas::QuotaKind::Categories,
    )
    .await?;
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(AppError::Validation(
            "folder name must contain 1–80 characters".into(),
        ));
    }
    let category = categories::create(state.db.pool(), user.id, name, input.parent_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"data": {"id": category.id}})),
    ))
}

#[derive(Default, Deserialize)]
pub struct ArticleQuery {
    feed_id: Option<Uuid>,
    #[serde(default)]
    unread: bool,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 {
    50
}

pub async fn articles_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ArticleQuery>,
) -> Result<Json<Value>, AppError> {
    let user = authenticate(&state, &headers, false, "articles.list").await?;
    let data: Vec<_> = articles::list_for_user(
        state.db.pool(),
        user.id,
        articles::LibraryFilter {
            feed_id: query.feed_id,
            category_id: None,
            unfiled: false,
            tag: None,
            unseen_only: false,
        },
        query.unread,
        false,
        query.limit.clamp(1, 100),
        query.offset.clamp(0, 1_000_000),
    )
    .await?
    .into_iter()
    .map(|article| {
        let clean_url = state.settings.tracking_url_policy.clean(&article.url);
        json!({
            "id": article.id,
            "feed_id": article.feed_id,
            "title": article.title,
            "url": clean_url,
            "excerpt": article.excerpt,
            "image_url": article.image_url,
            "published_at": article.published_at,
            "feed_name": article.feed_name,
            "read": article.is_read,
            "bookmarked": article.is_bookmarked,
            "has_summary": article.has_summary,
        })
    })
    .collect();
    Ok(Json(
        json!({ "data": data, "meta": { "limit": query.limit.clamp(1, 100), "offset": query.offset.max(0) } }),
    ))
}

async fn owned_article(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
) -> Result<rusty_feed_db::Article, AppError> {
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !subscriptions::is_subscribed(state.db.pool(), user_id, article.feed_id).await? {
        return Err(AppError::NotFound);
    }
    Ok(article)
}

pub async fn article(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let user = authenticate(&state, &headers, false, "articles.read").await?;
    let article = owned_article(&state, user.id, id).await?;
    let read = read_articles::is_read(state.db.pool(), user.id, id).await?;
    let bookmarked = bookmarks::is_bookmarked(state.db.pool(), user.id, id).await?;
    let clean_url = state.settings.tracking_url_policy.clean(&article.url);
    Ok(Json(json!({ "data": {
        "id": article.id, "feed_id": article.feed_id, "title": article.title,
        "url": clean_url, "original_url": article.original_url,
        "publisher_url": article.original_url,
        "author": article.author, "content_html": article.content,
        "excerpt": article.excerpt, "published_at": article.published_at,
        "summary": article.summary, "key_points": article.key_points,
        "topics": article.topics, "read": read, "bookmarked": bookmarked,
    }})))
}

#[derive(Deserialize)]
pub struct StateChange {
    read: Option<bool>,
    bookmarked: Option<bool>,
}

pub async fn article_state(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(change): Json<StateChange>,
) -> Result<Json<Value>, AppError> {
    let user = authenticate(&state, &headers, true, "articles.state").await?;
    owned_article(&state, user.id, id).await?;
    if let Some(read) = change.read {
        if read {
            read_articles::mark_read(state.db.pool(), user.id, id).await?;
        } else {
            read_articles::mark_unread(state.db.pool(), user.id, id).await?;
        }
    }
    if let Some(bookmarked) = change.bookmarked {
        if bookmarked {
            bookmarks::add(state.db.pool(), user.id, id).await?;
            crate::integrations::dispatch_event(&state, user.id, id, "article.bookmarked").await?;
        } else {
            bookmarks::remove(state.db.pool(), user.id, id).await?;
        }
    }
    Ok(Json(json!({ "data": { "id": id, "updated": true } })))
}

pub async fn saved_searches_list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    let user = authenticate(&state, &headers, false, "saved_searches.list").await?;
    let data: Vec<_> = saved_searches::list_for_user(state.db.pool(), user.id)
        .await?
        .into_iter()
        .map(|search| {
            json!({
                "id": search.id, "name": search.name, "query": search.query,
                "threshold": search.threshold, "notify": search.notify_on_match,
            })
        })
        .collect();
    Ok(Json(json!({ "data": data })))
}

#[derive(Deserialize)]
pub struct CreateSearch {
    name: String,
    query: String,
    #[serde(default)]
    notify: bool,
}

pub async fn saved_search_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CreateSearch>,
) -> Result<Json<Value>, AppError> {
    let user = authenticate(&state, &headers, true, "saved_searches.create").await?;
    crate::quota::assert_within(
        &state,
        user.id,
        rusty_feed_core::quotas::QuotaKind::SavedSearches,
    )
    .await?;
    let name = input.name.trim();
    let query = input.query.trim();
    if name.is_empty() || query.is_empty() {
        return Err(AppError::Validation("name and query are required".into()));
    }
    let search = saved_searches::create(
        state.db.pool(),
        user.id,
        name,
        query,
        0.6,
        input.notify,
        0.85,
    )
    .await?;
    let dedup = format!("saved_search_match:{}", search.id);
    jobs::enqueue(
        state.db.pool(),
        "saved_search_match",
        json!({ "saved_search_id": search.id }),
        Some(&dedup),
        None,
        3,
    )
    .await?;
    Ok(Json(json!({ "data": { "id": search.id } })))
}

pub async fn saved_search_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let user = authenticate(&state, &headers, true, "saved_searches.delete").await?;
    if !saved_searches::delete(state.db.pool(), id, user.id).await? {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}
