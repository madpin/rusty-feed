//! JSON-only Fever compatibility adapter backed by the same personal state and
//! stable shared aliases as the Google Reader implementation.

use std::collections::BTreeMap;

use axum::extract::State;
use axum::Form;
use axum::Json;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use rustss_core::hash::sha256_hex;
use rustss_db::{api_tokens, bookmarks, greader, read_articles, users, User};

use crate::auth::token;
use crate::error::AppError;
use crate::state::AppState;

const MAX_ITEMS: usize = 50;
const MAX_ID_LIST: i64 = 10_000;

fn value<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn has(pairs: &[(String, String)], name: &str) -> bool {
    pairs.iter().any(|(key, _)| key == name)
}

async fn authenticate(state: &AppState, pairs: &[(String, String)]) -> Result<User, AppError> {
    let api_key = value(pairs, "api_key")
        .filter(|value| value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or(AppError::Unauthorized)?;
    let credential = api_tokens::find_active_by_hash(state.db.pool(), &token::hash(api_key))
        .await?
        .filter(|token| token.scopes.iter().any(|scope| scope == "fever"))
        .ok_or(AppError::Unauthorized)?;
    api_tokens::touch(state.db.pool(), credential.id).await?;
    users::find_by_id(state.db.pool(), credential.user_id)
        .await?
        .ok_or(AppError::Unauthorized)
}

fn group_id(user_id: Uuid, name: &str) -> i64 {
    let digest = sha256_hex(format!("{user_id}:{name}").as_bytes());
    i64::from_str_radix(&digest[..8], 16).unwrap_or(1) & 0x7fff_ffff
}

fn parse_ids(value: Option<&str>) -> Vec<i64> {
    value
        .into_iter()
        .flat_map(|value| value.split(','))
        .filter_map(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .take(MAX_ITEMS)
        .collect()
}

async fn ids(
    state: &AppState,
    user_id: Uuid,
    unread: bool,
    saved: bool,
) -> Result<Vec<i64>, AppError> {
    Ok(greader::item_ids(
        state.db.pool(),
        user_id,
        None,
        false,
        saved,
        unread,
        None,
        None,
        false,
        MAX_ID_LIST,
        0,
    )
    .await?)
}

async fn mutate(
    state: &AppState,
    user_id: Uuid,
    pairs: &[(String, String)],
) -> Result<(), AppError> {
    if value(pairs, "mark") != Some("item") {
        return Ok(());
    }
    let id = value(pairs, "id")
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| AppError::Validation("Fever item id is invalid".into()))?;
    let article = greader::resolve_articles(state.db.pool(), user_id, &[id])
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)?;
    match value(pairs, "as") {
        Some("read") => read_articles::mark_read(state.db.pool(), user_id, article).await?,
        Some("unread") => read_articles::mark_unread(state.db.pool(), user_id, article).await?,
        Some("saved") => bookmarks::add(state.db.pool(), user_id, article).await?,
        Some("unsaved") => bookmarks::remove(state.db.pool(), user_id, article).await?,
        _ => return Err(AppError::Validation("Fever item action is invalid".into())),
    }
    Ok(())
}

/// `POST /fever/` — Fever's form-encoded JSON endpoint.
pub async fn endpoint(
    State(state): State<AppState>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Json<Value>, AppError> {
    let user = authenticate(&state, &pairs).await?;
    mutate(&state, user.id, &pairs).await?;
    let subscriptions = greader::subscriptions(state.db.pool(), user.id).await?;
    let mut response = Map::from_iter([
        ("api_version".to_string(), json!(3)),
        ("auth".to_string(), json!(1)),
        (
            "last_refreshed_on_time".to_string(),
            json!(time::OffsetDateTime::now_utc().unix_timestamp()),
        ),
    ]);

    if has(&pairs, "groups") || has(&pairs, "feeds") {
        let mut grouped = BTreeMap::<String, Vec<i64>>::new();
        for feed in &subscriptions {
            grouped
                .entry(
                    feed.category_name
                        .clone()
                        .unwrap_or_else(|| "Unfiled".to_string()),
                )
                .or_default()
                .push(feed.greader_id);
        }
        if has(&pairs, "groups") {
            response.insert(
                "groups".into(),
                json!(grouped
                    .keys()
                    .map(|name| json!({"id": group_id(user.id, name), "title": name}))
                    .collect::<Vec<_>>()),
            );
            response.insert(
                "feeds_groups".into(),
                json!(grouped
                    .iter()
                    .map(|(name, ids)| json!({
                        "group_id": group_id(user.id, name),
                        "feed_ids": ids.iter().map(i64::to_string).collect::<Vec<_>>().join(","),
                    }))
                    .collect::<Vec<_>>()),
            );
        }
        if has(&pairs, "feeds") {
            response.insert(
                "feeds".into(),
                json!(subscriptions
                    .iter()
                    .map(|feed| json!({
                        "id": feed.greader_id,
                        "favicon_id": feed.greader_id,
                        "title": feed.title,
                        "url": feed.url,
                        "site_url": feed.site_url,
                        "is_spark": 0,
                        "last_updated_on_time": 0,
                    }))
                    .collect::<Vec<_>>()),
            );
            response.insert(
                "favicons".into(),
                json!(subscriptions
                    .iter()
                    .map(|feed| json!({"id": feed.greader_id, "data": ""}))
                    .collect::<Vec<_>>()),
            );
        }
    }

    if has(&pairs, "unread_item_ids") {
        response.insert(
            "unread_item_ids".into(),
            json!(ids(&state, user.id, true, false)
                .await?
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",")),
        );
    }
    if has(&pairs, "saved_item_ids") {
        response.insert(
            "saved_item_ids".into(),
            json!(ids(&state, user.id, false, true)
                .await?
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",")),
        );
    }
    if has(&pairs, "items") {
        let mut requested = parse_ids(value(&pairs, "with_ids"));
        if requested.is_empty() {
            requested = ids(&state, user.id, false, false).await?;
            if let Some(since) = value(&pairs, "since_id").and_then(|value| value.parse().ok()) {
                requested.retain(|id| *id > since);
            }
            if let Some(max) = value(&pairs, "max_id").and_then(|value| value.parse().ok()) {
                requested.retain(|id| *id < max);
            }
            requested.truncate(MAX_ITEMS);
        }
        let items = greader::items(state.db.pool(), user.id, &requested, false).await?;
        response.insert(
            "items".into(),
            json!(items
                .into_iter()
                .map(|item| json!({
                    "id": item.greader_id,
                    "feed_id": item.feed_greader_id,
                    "title": item.title,
                    "author": item.author.unwrap_or_default(),
                    "html": item.content,
                    "url": state.settings.tracking_url_policy.clean(&item.url),
                    "is_saved": i32::from(item.is_bookmarked),
                    "is_read": i32::from(item.is_read),
                    "created_on_time": item.published_at.unwrap_or(item.updated_at).unix_timestamp(),
                }))
                .collect::<Vec<_>>()),
        );
        response.insert("total_items".into(), json!(requested.len()));
    }
    Ok(Json(Value::Object(response)))
}
