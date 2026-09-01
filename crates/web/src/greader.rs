//! Google Reader-compatible protocol adapter backed by RustSS domain state.

use axum::extract::{Path, RawForm, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use time::OffsetDateTime;
use uuid::Uuid;

use rustss_db::{
    api_tokens, articles, bookmarks, categories, feeds, greader, read_articles, subscriptions,
    users, User,
};
use rustss_feeds::{enqueue_fetch_feed, normalize_url};

use crate::auth::{password, token};
use crate::extract::ClientIp;
use crate::state::AppState;

const READING_LIST: &str = "state/com.google/reading-list";
const READ: &str = "state/com.google/read";
const STARRED: &str = "state/com.google/starred";
const KEPT_UNREAD: &str = "state/com.google/kept-unread";
const MAX_ITEMS: i64 = 10_000;

#[derive(Debug, thiserror::Error)]
pub enum GReaderError {
    #[error("Unauthorized")]
    Unauthorized,
    #[error("{0}")]
    BadRequest(String),
    #[error("Not Found")]
    NotFound,
    #[error("too many requests")]
    RateLimited,
    #[error("internal server error")]
    Internal(#[from] anyhow::Error),
}

impl From<rustss_db::DbError> for GReaderError {
    fn from(error: rustss_db::DbError) -> Self {
        Self::Internal(anyhow::Error::new(error))
    }
}

impl IntoResponse for GReaderError {
    fn into_response(self) -> Response {
        if matches!(self, Self::Internal(_)) {
            tracing::error!(error = ?self, "GReader request failed");
        }
        let (status, body) = match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized".to_string()),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::NotFound => (StatusCode::NOT_FOUND, "Not Found".to_string()),
            Self::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "too many requests".to_string(),
            ),
            Self::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".to_string(),
            ),
        };
        let mut response = (status, body).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        if status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                "x-reader-google-bad-token",
                HeaderValue::from_static("true"),
            );
        }
        response
    }
}

type Result<T> = std::result::Result<T, GReaderError>;
type Pairs = Vec<(String, String)>;

fn decode(value: &[u8]) -> Pairs {
    url::form_urlencoded::parse(value)
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

fn query_pairs(raw: RawQuery) -> Pairs {
    raw.0
        .as_deref()
        .map(str::as_bytes)
        .map(decode)
        .unwrap_or_default()
}

fn merged(raw_query: RawQuery, raw_form: RawForm) -> Pairs {
    let mut pairs = query_pairs(raw_query);
    pairs.extend(decode(&raw_form.0));
    pairs
}

fn first<'a>(pairs: &'a Pairs, key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value.as_str())
}

fn all<'a>(pairs: &'a Pairs, key: &'a str) -> impl Iterator<Item = &'a str> {
    pairs
        .iter()
        .filter(move |(candidate, _)| candidate == key)
        .map(|(_, value)| value.as_str())
}

fn plain(body: impl Into<String>) -> Response {
    let mut response = body.into().into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

fn token_allowed(scopes: &[String], write: bool) -> bool {
    scopes.is_empty()
        || scopes.iter().any(|scope| {
            matches!(scope.as_str(), "api" | "greader" | "write") || (!write && scope == "read")
        })
}

async fn authenticate_raw(state: &AppState, raw: &str, write: bool) -> Result<User> {
    let api_token = api_tokens::find_active_by_hash(state.db.pool(), &token::hash(raw))
        .await?
        .ok_or(GReaderError::Unauthorized)?;
    if !token_allowed(&api_token.scopes, write) {
        return Err(GReaderError::Unauthorized);
    }
    if !state.api_limiter.check(&api_token.id.to_string()).await {
        return Err(GReaderError::RateLimited);
    }
    api_tokens::touch(state.db.pool(), api_token.id).await?;
    users::find_by_id(state.db.pool(), api_token.user_id)
        .await?
        .ok_or(GReaderError::Unauthorized)
}

async fn authenticate_get(state: &AppState, headers: &HeaderMap) -> Result<(User, String)> {
    let raw = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("GoogleLogin auth="))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(GReaderError::Unauthorized)?;
    Ok((authenticate_raw(state, raw, false).await?, raw.to_string()))
}

async fn authenticate_post(state: &AppState, pairs: &Pairs) -> Result<User> {
    let raw = first(pairs, "T").ok_or(GReaderError::Unauthorized)?;
    authenticate_raw(state, raw, true).await
}

/// Exchange local credentials or an app-specific token for GReader auth.
pub async fn client_login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    RawForm(raw): RawForm,
) -> Result<Response> {
    if !state.auth_limiter.check(&ip).await {
        return Err(GReaderError::RateLimited);
    }
    let pairs = decode(&raw);
    let email = first(&pairs, "Email").unwrap_or_default().trim();
    let secret = first(&pairs, "Passwd").unwrap_or_default();
    let user = users::find_by_email(state.db.pool(), email)
        .await?
        .ok_or(GReaderError::Unauthorized)?;
    let existing_token =
        api_tokens::find_active_by_hash(state.db.pool(), &token::hash(secret)).await?;
    let accepted_app_token = existing_token.as_ref().is_some_and(|candidate| {
        candidate.user_id == user.id && token_allowed(&candidate.scopes, true)
    });
    let accepted_password = user
        .password_hash
        .as_deref()
        .is_some_and(|hash| password::verify_password(secret, hash));
    if !accepted_app_token && !accepted_password {
        return Err(GReaderError::Unauthorized);
    }
    let raw_token = if accepted_app_token {
        secret.to_string()
    } else {
        // Password ClientLogin mints a fresh token each call; bound accumulation
        // with the same per-user API-token quota as the settings UI (FND-188).
        let quotas = &state.settings.quotas;
        let kind = rustss_core::quotas::QuotaKind::ApiTokens;
        if quotas.limit(kind) > 0 {
            let count = api_tokens::count_for_user(state.db.pool(), user.id).await?;
            if !quotas.allows(kind, count) {
                return Err(GReaderError::BadRequest(quotas.exceeded_message(kind)));
            }
        }
        let generated = token::generate();
        api_tokens::create(
            state.db.pool(),
            user.id,
            "Google Reader client",
            &generated.hash,
            &["greader".to_string()],
        )
        .await?;
        generated.raw
    };
    if first(&pairs, "output") == Some("json") {
        return Ok(Json(json!({
            "SID": raw_token,
            "LSID": raw_token,
            "Auth": raw_token,
        }))
        .into_response());
    }
    Ok(plain(format!(
        "SID={raw_token}\nLSID={raw_token}\nAuth={raw_token}\n"
    )))
}

pub async fn edit_token(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    let (_, raw) = authenticate_get(&state, &headers).await?;
    Ok(plain(raw))
}

pub async fn user_info(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>> {
    let (user, _) = authenticate_get(&state, &headers).await?;
    let id = user.id.to_string();
    Ok(Json(json!({
        "userId": id,
        "userName": user.name.unwrap_or_else(|| user.email.clone()),
        "userProfileId": id,
        "userEmail": user.email,
    })))
}

fn label_id(user_id: Uuid, label: &str) -> String {
    format!("user/{user_id}/label/{label}")
}

fn state_id(user_id: Uuid, state: &str) -> String {
    format!("user/{user_id}/{state}")
}

fn label_name(stream: &str) -> Option<&str> {
    stream
        .split_once("/label/")
        .map(|(_, label)| label)
        .filter(|v| !v.is_empty())
}

pub async fn tag_list(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>> {
    let (user, _) = authenticate_get(&state, &headers).await?;
    let mut tags = vec![json!({ "id": state_id(user.id, STARRED) })];
    tags.extend(
        categories::list_for_user(state.db.pool(), user.id)
            .await?
            .into_iter()
            .map(|category| {
                json!({
                    "id": label_id(user.id, &category.name),
                    "label": category.name,
                    "type": "folder",
                })
            }),
    );
    Ok(Json(json!({ "tags": tags })))
}

pub async fn subscription_list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    let (user, _) = authenticate_get(&state, &headers).await?;
    let rows = greader::subscriptions(state.db.pool(), user.id).await?;
    let subscriptions: Vec<_> = rows
        .into_iter()
        .map(|row| {
            let categories = row.category_name.map_or_else(Vec::new, |name| {
                vec![json!({ "id": label_id(user.id, &name), "label": name, "type": "folder" })]
            });
            json!({
                "id": format!("feed/{}", row.greader_id),
                "title": row.title,
                "categories": categories,
                "url": row.url,
                "htmlUrl": row.site_url.unwrap_or_default(),
                "iconUrl": row.image_url.unwrap_or_else(|| format!("{}/feeds/{}/icon", state.settings.base_url.trim_end_matches('/'), row.feed_id)),
            })
        })
        .collect();
    Ok(Json(json!({ "subscriptions": subscriptions })))
}

fn parse_feed_protocol_id(stream: &str) -> Result<i64> {
    stream
        .strip_prefix("feed/")
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| GReaderError::BadRequest("invalid feed stream".to_string()))
}

async fn category_for_label(state: &AppState, user_id: Uuid, stream: &str) -> Result<Uuid> {
    let name = label_name(stream)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| GReaderError::BadRequest("invalid label stream".to_string()))?;
    if name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(GReaderError::BadRequest("invalid label".to_string()));
    }
    if let Some(category) = categories::find_by_name(state.db.pool(), user_id, name).await? {
        return Ok(category.id);
    }
    // Creating a new label is a new category row — subject to the quota (FND-188).
    let quotas = &state.settings.quotas;
    let kind = rustss_core::quotas::QuotaKind::Categories;
    if quotas.limit(kind) > 0 {
        let count = categories::count_for_user(state.db.pool(), user_id).await?;
        if !quotas.allows(kind, count) {
            return Err(GReaderError::BadRequest(quotas.exceeded_message(kind)));
        }
    }
    Ok(categories::create(state.db.pool(), user_id, name, None)
        .await?
        .id)
}

pub async fn subscription_edit(
    State(state): State<AppState>,
    raw_query: RawQuery,
    raw_form: RawForm,
) -> Result<Response> {
    let pairs = merged(raw_query, raw_form);
    let user = authenticate_post(&state, &pairs).await?;
    let action = first(&pairs, "ac").unwrap_or("edit");
    let streams: Vec<_> = all(&pairs, "s").collect();
    let stream = streams
        .first()
        .copied()
        .ok_or_else(|| GReaderError::BadRequest("missing subscription stream".to_string()))?;
    match action {
        "subscribe" => {
            // Same gate as every other feed-add path: unverified local accounts
            // cannot add feeds via the GReader API either (FND-191).
            crate::verify::require_verified(&state, &user)
                .map_err(|error| GReaderError::BadRequest(error.to_string()))?;
            // Per-user subscription quota (FND-188).
            let quotas = &state.settings.quotas;
            let kind = rustss_core::quotas::QuotaKind::Subscriptions;
            if quotas.limit(kind) > 0 {
                let count = subscriptions::count_for_user(state.db.pool(), user.id).await?;
                if !quotas.allows(kind, count) {
                    return Err(GReaderError::BadRequest(quotas.exceeded_message(kind)));
                }
            }
            let source = stream
                .strip_prefix("feed/")
                .ok_or_else(|| GReaderError::BadRequest("invalid feed URL stream".to_string()))?;
            let normalized = normalize_url(source)
                .map_err(|error| GReaderError::BadRequest(format!("invalid feed URL: {error}")))?;
            state
                .fetch
                .validate_url(&normalized)
                .map_err(|error| GReaderError::BadRequest(format!("unsafe feed URL: {error}")))?;
            // Use the shared cached blocklist (same source as assert_feed_allowed).
            let blocklist = state
                .blocklist()
                .await
                .map_err(|error| GReaderError::Internal(anyhow::anyhow!(error.to_string())))?;
            if let Some(reason) = blocklist.blocked_reason(&normalized) {
                return Err(GReaderError::BadRequest(format!(
                    "feed is blocked: {reason}"
                )));
            }
            let host = url::Url::parse(&normalized)
                .ok()
                .and_then(|url| url.host_str().map(str::to_string))
                .unwrap_or_else(|| "Feed".to_string());
            let title = first(&pairs, "t").filter(|title| !title.trim().is_empty());
            let (feed, _) =
                feeds::upsert_by_url(state.db.pool(), &normalized, title.unwrap_or(&host), None)
                    .await?;
            subscriptions::subscribe(state.db.pool(), user.id, feed.id, title).await?;
            if let Some(label) = first(&pairs, "a") {
                let category = category_for_label(&state, user.id, label).await?;
                subscriptions::update_organization(
                    state.db.pool(),
                    user.id,
                    feed.id,
                    &[],
                    Some(category),
                )
                .await?;
            }
            enqueue_fetch_feed(&state.db, feed.id).await?;
        }
        "unsubscribe" => {
            for stream in streams {
                let protocol_id = parse_feed_protocol_id(stream)?;
                if let Some(feed_id) =
                    greader::resolve_feed(state.db.pool(), user.id, protocol_id).await?
                {
                    subscriptions::unsubscribe(state.db.pool(), user.id, feed_id).await?;
                }
            }
        }
        "edit" => {
            let protocol_id = parse_feed_protocol_id(stream)?;
            let feed_id = greader::resolve_feed(state.db.pool(), user.id, protocol_id)
                .await?
                .ok_or(GReaderError::NotFound)?;
            if let Some(title) = first(&pairs, "t") {
                let title = title.trim();
                subscriptions::set_custom_name(
                    state.db.pool(),
                    user.id,
                    feed_id,
                    (!title.is_empty()).then_some(title),
                )
                .await?;
            }
            if let Some(label) = first(&pairs, "a") {
                let category = category_for_label(&state, user.id, label).await?;
                let current = subscriptions::find(state.db.pool(), user.id, feed_id)
                    .await?
                    .ok_or(GReaderError::NotFound)?;
                subscriptions::update_organization(
                    state.db.pool(),
                    user.id,
                    feed_id,
                    &current.tags,
                    Some(category),
                )
                .await?;
            }
        }
        _ => return Err(GReaderError::BadRequest("unsupported action".to_string())),
    }
    Ok(plain("OK\n"))
}

fn parse_item_id(value: &str) -> Option<i64> {
    if let Some(hex) = value.strip_prefix("tag:google.com,2005:reader/item/") {
        return i64::from_str_radix(hex, 16).ok();
    }
    if value.len() == 16
        && value
            .chars()
            .any(|character| character.is_ascii_alphabetic())
    {
        return i64::from_str_radix(value, 16).ok();
    }
    value.parse().ok()
}

fn parse_item_ids(pairs: &Pairs) -> Vec<i64> {
    all(pairs, "i").filter_map(parse_item_id).collect()
}

fn has_state(stream: &str, state: &str) -> bool {
    stream.ends_with(state)
}

pub async fn edit_tag(
    State(state): State<AppState>,
    raw_query: RawQuery,
    raw_form: RawForm,
) -> Result<Response> {
    let pairs = merged(raw_query, raw_form);
    let user = authenticate_post(&state, &pairs).await?;
    let protocol_ids = parse_item_ids(&pairs);
    let article_ids = greader::resolve_articles(state.db.pool(), user.id, &protocol_ids).await?;
    let additions: Vec<_> = all(&pairs, "a").collect();
    let removals: Vec<_> = all(&pairs, "r").collect();
    let add_read = additions.iter().any(|tag| has_state(tag, READ));
    let remove_read = removals.iter().any(|tag| has_state(tag, READ));
    let add_unread = additions.iter().any(|tag| has_state(tag, KEPT_UNREAD));
    let remove_unread = removals.iter().any(|tag| has_state(tag, KEPT_UNREAD));
    let add_star = additions.iter().any(|tag| has_state(tag, STARRED));
    let remove_star = removals.iter().any(|tag| has_state(tag, STARRED));
    if (add_read || remove_unread) && (remove_read || add_unread) || add_star && remove_star {
        return Err(GReaderError::BadRequest(
            "conflicting tag operations".to_string(),
        ));
    }
    for article_id in article_ids {
        if add_read || remove_unread {
            read_articles::mark_read(state.db.pool(), user.id, article_id).await?;
        } else if remove_read || add_unread {
            read_articles::mark_unread(state.db.pool(), user.id, article_id).await?;
        }
        if add_star {
            bookmarks::add(state.db.pool(), user.id, article_id).await?;
        } else if remove_star {
            bookmarks::remove(state.db.pool(), user.id, article_id).await?;
        }
    }
    Ok(plain("OK\n"))
}

#[derive(Default)]
struct StreamFilter {
    feed_id: Option<Uuid>,
    only_read: bool,
    only_starred: bool,
    unread_only: bool,
}

async fn stream_filter(state: &AppState, user_id: Uuid, pairs: &Pairs) -> Result<StreamFilter> {
    let stream = first(pairs, "s").unwrap_or("user/-/state/com.google/reading-list");
    let mut filter = StreamFilter::default();
    if let Some(value) = stream.strip_prefix("feed/") {
        let protocol_id = value
            .parse()
            .map_err(|_| GReaderError::BadRequest("invalid feed stream".to_string()))?;
        filter.feed_id = Some(
            greader::resolve_feed(state.db.pool(), user_id, protocol_id)
                .await?
                .ok_or(GReaderError::NotFound)?,
        );
    } else if has_state(stream, STARRED) {
        filter.only_starred = true;
    } else if has_state(stream, READ) {
        filter.only_read = true;
    } else if !has_state(stream, READING_LIST) {
        return Err(GReaderError::BadRequest("unsupported stream".to_string()));
    }
    filter.unread_only = all(pairs, "xt").any(|target| has_state(target, READ));
    Ok(filter)
}

fn unix_time(value: Option<&str>) -> Option<OffsetDateTime> {
    value
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
}

async fn ids_for_pairs(
    state: &AppState,
    user_id: Uuid,
    pairs: &Pairs,
) -> Result<(Vec<i64>, Option<String>)> {
    let filter = stream_filter(state, user_id, pairs).await?;
    let limit = first(pairs, "n")
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(MAX_ITEMS)
        .min(MAX_ITEMS);
    let offset = first(pairs, "c")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);
    let ids = greader::item_ids(
        state.db.pool(),
        user_id,
        filter.feed_id,
        filter.only_read,
        filter.only_starred,
        filter.unread_only,
        unix_time(first(pairs, "ot")),
        unix_time(first(pairs, "nt")),
        first(pairs, "r") == Some("o"),
        limit + 1,
        offset,
    )
    .await?;
    let has_more = ids.len() as i64 > limit;
    let ids: Vec<_> = ids.into_iter().take(limit as usize).collect();
    let continuation = has_more.then(|| (offset + ids.len() as i64).to_string());
    Ok((ids, continuation))
}

pub async fn stream_item_ids(
    State(state): State<AppState>,
    headers: HeaderMap,
    raw_query: RawQuery,
) -> Result<Json<Value>> {
    let (user, _) = authenticate_get(&state, &headers).await?;
    let pairs = query_pairs(raw_query);
    let (ids, continuation) = ids_for_pairs(&state, user.id, &pairs).await?;
    let mut response = json!({
        "itemRefs": ids.into_iter().map(|id| json!({ "id": id.to_string() })).collect::<Vec<_>>()
    });
    if let Some(continuation) = continuation {
        response["continuation"] = json!(continuation);
    }
    Ok(Json(response))
}

fn item_json(
    user_id: Uuid,
    item: &greader::Item,
    clean_url: &str,
    enclosures: Vec<Value>,
) -> Value {
    let published = item.published_at.unwrap_or(item.updated_at);
    let published_seconds = published.unix_timestamp();
    let updated_seconds = item.updated_at.unix_timestamp();
    let mut categories = vec![state_id(user_id, READING_LIST)];
    if item.is_read {
        categories.push(state_id(user_id, READ));
    }
    if item.is_bookmarked {
        categories.push(state_id(user_id, STARRED));
    }
    if let Some(category) = &item.category_name {
        categories.push(label_id(user_id, category));
    }
    json!({
        "id": format!("tag:google.com,2005:reader/item/{:016x}", item.greader_id),
        "categories": categories,
        "title": item.title,
        "crawlTimeMsec": (item.updated_at.unix_timestamp_nanos() / 1_000_000).to_string(),
        "timestampUsec": (published.unix_timestamp_nanos() / 1_000).to_string(),
        "published": published_seconds,
        "updated": updated_seconds,
        "author": item.author.clone().unwrap_or_default(),
        "alternate": [{ "href": clean_url, "type": "text/html" }],
        "summary": { "direction": "ltr", "content": item.content },
        "content": { "direction": "ltr", "content": item.content },
        "origin": {
            "streamId": format!("feed/{}", item.feed_greader_id),
            "title": item.feed_title,
            "htmlUrl": item.feed_site_url.clone().unwrap_or_default(),
        },
        "enclosure": enclosures,
        "canonical": [{ "href": clean_url }],
    })
}

async fn contents_json(
    state: &AppState,
    user: &User,
    ids: &[i64],
    ascending: bool,
) -> Result<Value> {
    let rows = greader::items(state.db.pool(), user.id, ids, ascending).await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let enclosures = articles::list_enclosures(state.db.pool(), row.article_id)
            .await?
            .into_iter()
            .map(|enclosure| json!({
                "href": state.settings.tracking_url_policy.clean(&enclosure.url),
                "type": enclosure.mime_type.unwrap_or_else(|| "application/octet-stream".to_string()),
                "length": enclosure.size_bytes.unwrap_or(0),
            }))
            .collect();
        let clean_url = state.settings.tracking_url_policy.clean(&row.url);
        items.push(item_json(user.id, &row, &clean_url, enclosures));
    }
    let updated = items
        .iter()
        .filter_map(|item| item.get("updated").and_then(Value::as_i64))
        .max()
        .unwrap_or_else(|| OffsetDateTime::now_utc().unix_timestamp());
    Ok(json!({
        "direction": "ltr",
        "id": "user/-/state/com.google/reading-list",
        "title": "Reading List",
        "self": [{ "href": format!("{}/reader/api/0/stream/items/contents", state.settings.base_url.trim_end_matches('/')) }],
        "updated": updated,
        "author": user.name.clone().unwrap_or_else(|| user.email.clone()),
        "items": items,
    }))
}

pub async fn stream_item_contents(
    State(state): State<AppState>,
    raw_query: RawQuery,
    raw_form: RawForm,
) -> Result<Json<Value>> {
    let pairs = merged(raw_query, raw_form);
    let user = authenticate_post(&state, &pairs).await?;
    let ids = parse_item_ids(&pairs);
    Ok(Json(
        contents_json(&state, &user, &ids, first(&pairs, "r") == Some("o")).await?,
    ))
}

pub async fn stream_contents(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(stream): Path<String>,
    raw_query: RawQuery,
) -> Result<Json<Value>> {
    let (user, _) = authenticate_get(&state, &headers).await?;
    let mut pairs = query_pairs(raw_query);
    pairs.push(("s".to_string(), stream));
    let (ids, continuation) = ids_for_pairs(&state, user.id, &pairs).await?;
    let mut response = contents_json(&state, &user, &ids, first(&pairs, "r") == Some("o")).await?;
    if let Some(continuation) = continuation {
        response["continuation"] = json!(continuation);
    }
    Ok(Json(response))
}

pub async fn mark_all_as_read(
    State(state): State<AppState>,
    raw_query: RawQuery,
    raw_form: RawForm,
) -> Result<Response> {
    let pairs = merged(raw_query, raw_form);
    let user = authenticate_post(&state, &pairs).await?;
    let stream = first(&pairs, "s").unwrap_or("user/-/state/com.google/reading-list");
    let (feed_id, category_name) = if let Some(value) = stream.strip_prefix("feed/") {
        let protocol_id = value
            .parse()
            .map_err(|_| GReaderError::BadRequest("invalid feed stream".to_string()))?;
        (
            Some(
                greader::resolve_feed(state.db.pool(), user.id, protocol_id)
                    .await?
                    .ok_or(GReaderError::NotFound)?,
            ),
            None,
        )
    } else if let Some(label) = label_name(stream) {
        (None, Some(label))
    } else {
        (None, None)
    };
    let cutoff = first(&pairs, "ts")
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|value| {
            let seconds = if value.to_string().len() >= 16 {
                value / 1_000_000
            } else {
                value
            };
            OffsetDateTime::from_unix_timestamp(seconds).ok()
        })
        .unwrap_or_else(OffsetDateTime::now_utc);
    read_articles::mark_all_read_before(state.db.pool(), user.id, feed_id, category_name, cutoff)
        .await?;
    Ok(plain("OK\n"))
}

/// Compatibility catch-all: clients commonly probe optional endpoints.
pub async fn fallback() -> Json<Value> {
    Json(json!([]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_documented_item_id_forms() {
        assert_eq!(parse_item_id("12345"), Some(12345));
        assert_eq!(parse_item_id("000000000000048c"), Some(1164));
        assert_eq!(
            parse_item_id("tag:google.com,2005:reader/item/00000000148b9369"),
            Some(344691561)
        );
    }

    #[test]
    fn repeated_form_values_are_preserved() {
        let pairs = decode(b"i=1&i=2&a=read");
        assert_eq!(all(&pairs, "i").collect::<Vec<_>>(), vec!["1", "2"]);
    }
}
