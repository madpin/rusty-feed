//! Article-authorized, SSRF-safe delivery for shared reader media.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use rustss_core::hash::sha256_hex;
use rustss_db::{articles, media_cache, MediaCacheEntry};
use rustss_feeds::FeedError;
use serde::Deserialize;

use crate::auth::extractor::AuthUser;
use crate::{AppError, AppState};

const HERO_IMAGE_MAX_BYTES: usize = 8 * 1_024 * 1_024;

#[derive(Debug, Default, Deserialize)]
pub struct MediaQuery {
    download: Option<u8>,
}

/// `GET /articles/{id}/media/{position}` — serve one authorized enclosure.
pub async fn get(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((article_id, position)): Path<(Uuid, i32)>,
    Query(query): Query<MediaQuery>,
    request_headers: HeaderMap,
) -> Result<Response, AppError> {
    if !state.settings.media_proxy_enabled || position < 0 {
        return Err(AppError::NotFound);
    }
    if !articles::user_can_access(state.db.pool(), user.id, article_id).await? {
        return Err(AppError::NotFound);
    }
    let enclosure = articles::find_enclosure(state.db.pool(), article_id, position)
        .await?
        .ok_or(AppError::NotFound)?;
    let cache_key = sha256_hex(enclosure.url.as_bytes());
    let entry = cached_entry(
        &state,
        &cache_key,
        &enclosure.url,
        state.settings.media_proxy_max_bytes,
    )
    .await?;

    serve_entry(
        &enclosure.media_kind,
        &enclosure.url,
        entry,
        &request_headers,
        query.download == Some(1),
        &state.settings.media_proxy_extra_inline_mime_types,
    )
}

/// `GET /articles/{id}/hero` — serve an authorized header image without
/// exposing the reader's address to the publisher.
pub async fn hero(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(article_id): Path<Uuid>,
    request_headers: HeaderMap,
) -> Result<Response, AppError> {
    if !state.settings.media_proxy_enabled
        || !articles::user_can_access(state.db.pool(), user.id, article_id).await?
    {
        return Err(AppError::NotFound);
    }
    let article = articles::find_by_id(state.db.pool(), article_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let source_url = article.image_url.ok_or(AppError::NotFound)?;
    let cache_key = sha256_hex(format!("hero:v1:{source_url}").as_bytes());
    let entry = cached_entry(
        &state,
        &cache_key,
        &source_url,
        state
            .settings
            .media_proxy_max_bytes
            .min(HERO_IMAGE_MAX_BYTES),
    )
    .await?;
    serve_entry(
        "image",
        &source_url,
        entry,
        &request_headers,
        false,
        &state.settings.media_proxy_extra_inline_mime_types,
    )
}

async fn cached_entry(
    state: &AppState,
    cache_key: &str,
    source_url: &str,
    max_bytes: usize,
) -> Result<MediaCacheEntry, AppError> {
    if let Some(entry) = fresh_entry(state, cache_key, source_url).await? {
        return Ok(entry);
    }
    let lock = state.media_fetch_lock(cache_key).await;
    let _guard = lock.lock().await;
    if let Some(entry) = fresh_entry(state, cache_key, source_url).await? {
        return Ok(entry);
    }
    fetch_and_cache_coalesced(state, cache_key, source_url, max_bytes).await
}

async fn fetch_and_cache_coalesced(
    state: &AppState,
    cache_key: &str,
    source_url: &str,
    max_bytes: usize,
) -> Result<MediaCacheEntry, AppError> {
    // The session advisory lock coordinates cache misses across all processes
    // connected to this PostgreSQL cluster. The in-process Moka lock above
    // keeps the common single-instance path cheap.
    let mut connection = media_cache::acquire_fetch_lock(state.db.pool(), cache_key).await?;
    let result = async {
        if let Some(entry) = fresh_entry(state, cache_key, source_url).await? {
            Ok(entry)
        } else {
            fetch_and_cache(state, cache_key, source_url, max_bytes).await
        }
    }
    .await;
    if let Err(error) = media_cache::release_fetch_lock(&mut connection, cache_key).await {
        tracing::error!(error = %error, "failed to release media cache advisory lock");
    }
    result
}

async fn fresh_entry(
    state: &AppState,
    cache_key: &str,
    source_url: &str,
) -> Result<Option<MediaCacheEntry>, AppError> {
    Ok(media_cache::find_fresh(state.db.pool(), cache_key)
        .await?
        .filter(|entry| entry.source_url == source_url))
}

async fn fetch_and_cache(
    state: &AppState,
    cache_key: &str,
    source_url: &str,
    max_bytes: usize,
) -> Result<MediaCacheEntry, AppError> {
    let fetched = state
        .fetch
        .get_bytes_with_metadata(source_url, max_bytes)
        .await;
    let now = OffsetDateTime::now_utc();
    match fetched {
        Ok(fetched) => {
            let content_type = normalize_content_type(fetched.content_type.as_deref());
            let expires_at = now + duration(state.settings.media_proxy_cache_ttl);
            media_cache::store_success(
                state.db.pool(),
                cache_key,
                source_url,
                content_type.as_deref(),
                &fetched.body,
                expires_at,
            )
            .await?;
            media_cache::trim_to_budget(
                state.db.pool(),
                i64::try_from(state.settings.media_proxy_cache_budget_bytes).unwrap_or(i64::MAX),
            )
            .await?;
            Ok(MediaCacheEntry {
                cache_key: cache_key.to_string(),
                source_url: source_url.to_string(),
                status: "ready".to_string(),
                content_type,
                body: Some(fetched.body),
                failure_kind: None,
                fetched_at: now,
                expires_at,
            })
        }
        Err(error) => {
            let failure_kind = match error {
                FeedError::TooLarge => "too-large",
                FeedError::InvalidUrl(_) | FeedError::UnsupportedScheme(_) => "unsafe-url",
                _ => "unavailable",
            };
            let expires_at = now + duration(state.settings.media_proxy_negative_ttl);
            media_cache::store_failure(
                state.db.pool(),
                cache_key,
                source_url,
                failure_kind,
                expires_at,
            )
            .await?;
            Ok(MediaCacheEntry {
                cache_key: cache_key.to_string(),
                source_url: source_url.to_string(),
                status: "failed".to_string(),
                content_type: None,
                body: None,
                failure_kind: Some(failure_kind.to_string()),
                fetched_at: now,
                expires_at,
            })
        }
    }
}

fn duration(value: std::time::Duration) -> Duration {
    Duration::seconds(i64::try_from(value.as_secs()).unwrap_or(i64::MAX))
}

fn normalize_content_type(value: Option<&str>) -> Option<String> {
    value
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn safe_inline_content_type(
    kind: &str,
    value: Option<&str>,
    extra_inline_mime_types: &[String],
) -> Option<String> {
    let value = normalize_content_type(value)?;
    let safe = match kind {
        "audio" => matches!(
            value.as_str(),
            "audio/aac"
                | "audio/flac"
                | "audio/m4a"
                | "audio/mp4"
                | "audio/mpeg"
                | "audio/ogg"
                | "audio/wav"
                | "audio/webm"
                | "audio/x-wav"
                | "application/ogg"
        ),
        "video" => matches!(
            value.as_str(),
            "video/mp4" | "video/ogg" | "video/quicktime" | "video/webm"
        ),
        "image" => matches!(
            value.as_str(),
            "image/avif" | "image/gif" | "image/jpeg" | "image/png" | "image/webp"
        ),
        _ => false,
    };
    let configured = value.starts_with(&format!("{kind}/"))
        && extra_inline_mime_types.iter().any(|extra| extra == &value);
    (safe || configured).then_some(value)
}

fn serve_entry(
    media_kind: &str,
    source_url: &str,
    entry: MediaCacheEntry,
    request_headers: &HeaderMap,
    force_download: bool,
    extra_inline_mime_types: &[String],
) -> Result<Response, AppError> {
    if entry.status == "failed" {
        let status = if entry.failure_kind.as_deref() == Some("too-large") {
            StatusCode::PAYLOAD_TOO_LARGE
        } else {
            StatusCode::BAD_GATEWAY
        };
        return Ok(media_error(
            status,
            "Media is temporarily unavailable through RustSS.",
        ));
    }
    let body = entry.body.ok_or_else(|| {
        AppError::Internal(anyhow::anyhow!("ready media cache entry has no body"))
    })?;
    let download = force_download || media_kind == "file";
    let inline_type = (!download)
        .then(|| {
            safe_inline_content_type(
                media_kind,
                entry.content_type.as_deref(),
                extra_inline_mime_types,
            )
        })
        .flatten();
    if !download && inline_type.is_none() {
        return Ok(media_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "The publisher returned a media type that RustSS will not render inline.",
        ));
    }

    let total = body.len();
    let range = match request_headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) => match parse_range(value, total) {
            Some(range) => Some(range),
            None => return Ok(range_not_satisfiable(total)),
        },
        None => None,
    };
    let (status, bytes, content_range) = if let Some((start, end)) = range {
        (
            StatusCode::PARTIAL_CONTENT,
            body[start..=end].to_vec(),
            Some(format!("bytes {start}-{end}/{total}")),
        )
    } else {
        (StatusCode::OK, body, None)
    };

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(inline_type.as_deref().unwrap_or("application/octet-stream"))
            .map_err(|error| AppError::Internal(error.into()))?,
    );
    response_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response_headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&bytes.len().to_string())
            .map_err(|error| AppError::Internal(error.into()))?,
    );
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=300"),
    );
    response_headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response_headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if let Some(content_range) = content_range {
        response_headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&content_range)
                .map_err(|error| AppError::Internal(error.into()))?,
        );
    }
    if download {
        let filename = download_filename(source_url);
        response_headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
                .map_err(|error| AppError::Internal(error.into()))?,
        );
    }
    Ok((status, response_headers, Body::from(bytes)).into_response())
}

fn parse_range(value: &str, total: usize) -> Option<(usize, usize)> {
    let spec = value.strip_prefix("bytes=")?;
    if total == 0 || spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<usize>().ok()?;
        if suffix == 0 {
            return None;
        }
        let start = total.saturating_sub(suffix);
        return Some((start, total - 1));
    }
    let start = start.parse::<usize>().ok()?;
    if start >= total {
        return None;
    }
    let end = if end.is_empty() {
        total - 1
    } else {
        end.parse::<usize>().ok()?.min(total - 1)
    };
    (start <= end).then_some((start, end))
}

fn range_not_satisfiable(total: usize) -> Response {
    (
        StatusCode::RANGE_NOT_SATISFIABLE,
        [
            (header::CONTENT_RANGE, format!("bytes */{total}")),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        "Requested byte range is not available.",
    )
        .into_response()
}

fn media_error(status: StatusCode, message: &'static str) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        message,
    )
        .into_response()
}

fn download_filename(source_url: &str) -> String {
    let candidate = url::Url::parse(source_url)
        .ok()
        .and_then(|url| url.path_segments()?.next_back().map(str::to_string))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "attachment".to_string());
    let filename: String = candidate
        .chars()
        .take(120)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if filename.is_empty() {
        "attachment".to_string()
    } else {
        filename
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_ranges() {
        assert_eq!(parse_range("bytes=0-3", 10), Some((0, 3)));
        assert_eq!(parse_range("bytes=4-", 10), Some((4, 9)));
        assert_eq!(parse_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(parse_range("bytes=0-99", 10), Some((0, 9)));
        assert_eq!(parse_range("bytes=10-", 10), None);
        assert_eq!(parse_range("bytes=0-1,4-5", 10), None);
    }

    #[test]
    fn only_passive_media_types_render_inline() {
        assert_eq!(
            safe_inline_content_type("audio", Some("audio/mpeg; charset=binary"), &[]),
            Some("audio/mpeg".to_string())
        );
        assert!(safe_inline_content_type("image", Some("image/svg+xml"), &[]).is_none());
        assert!(safe_inline_content_type("video", Some("text/html"), &[]).is_none());
        assert!(safe_inline_content_type("file", Some("application/pdf"), &[]).is_none());
        assert_eq!(
            safe_inline_content_type("image", Some("image/jxl"), &["image/jxl".to_string()]),
            Some("image/jxl".to_string())
        );
    }

    #[test]
    fn sanitizes_download_filenames() {
        assert_eq!(
            download_filename("https://media.example/path/my%20file.pdf"),
            "my_20file.pdf"
        );
        assert_eq!(download_filename("https://media.example/"), "attachment");
    }
}
