//! Isolated, article-scoped publisher rendering for Live View.
//!
//! Publisher HTML is fetched through [`SafeFetchClient`], rendered in an
//! opaque-origin sandbox, and authorized with short-lived encrypted bearer
//! tickets so the iframe never receives RustSS cookies. Static resources are
//! rewritten to similarly scoped tickets; this is intentionally not a general
//! URL proxy.

use std::time::Duration as StdDuration;

use anyhow::{anyhow, bail, Context};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use lol_html::html_content::ContentType;
use lol_html::{element, rewrite_str, text, RewriteStrSettings};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use url::Url;
use uuid::Uuid;

use rustss_core::crypto::Cryptor;
use rustss_core::hash::sha256_hex;
use rustss_db::{articles, Article};

use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

const LIVE_TICKET_LIFETIME: Duration = Duration::minutes(15);
const LIVE_RESOURCE_TICKET_LIFETIME: Duration = Duration::hours(1);
const LIVE_PAGE_CACHE_TTL: StdDuration = StdDuration::from_secs(120);
const LIVE_RESOURCE_CACHE_TTL: StdDuration = StdDuration::from_secs(15 * 60);
const LIVE_FAILURE_CACHE_TTL: StdDuration = StdDuration::from_secs(15);
const MAX_PAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_RESOURCE_BYTES: usize = 3 * 1024 * 1024;

const LIVE_CSP: &str = "sandbox allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-modals allow-downloads; default-src * data: blob:; script-src * 'unsafe-inline' 'unsafe-eval' blob:; style-src * 'unsafe-inline'; img-src * data: blob:; media-src * data: blob:; font-src * data:; connect-src *; frame-src *; object-src 'none'; base-uri *; form-action *; frame-ancestors 'self'";

#[derive(Debug, Serialize, Deserialize)]
struct LivePageTicket {
    article_id: Uuid,
    user_id: Uuid,
    expires_at: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ResourceKind {
    Script,
    Stylesheet,
    Asset,
}

impl ResourceKind {
    fn cache_name(self) -> &'static str {
        match self {
            Self::Script => "script",
            Self::Stylesheet => "style",
            Self::Asset => "asset",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct LiveResourceTicket {
    article_id: Uuid,
    url: String,
    kind: ResourceKind,
    expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FetchedPage {
    html: String,
    final_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FetchedResource {
    body: String,
    content_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedPage {
    page: Option<FetchedPage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedResource {
    resource: Option<FetchedResource>,
}

#[derive(Debug, Deserialize)]
pub struct TicketQuery {
    ticket: String,
}

pub(crate) fn issue_url(
    state: &AppState,
    user_id: Uuid,
    article_id: Uuid,
) -> Result<String, AppError> {
    let ticket = seal(
        &state.live_view_tokens,
        &LivePageTicket {
            article_id,
            user_id,
            expires_at: ticket_expiry(),
        },
    )?;
    Ok(format!(
        "/articles/{article_id}/live?ticket={}",
        percent_encode(&ticket)
    ))
}

pub async fn ticket(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(article_id): Path<Uuid>,
) -> Result<Response, AppError> {
    if !articles::user_can_access(state.db.pool(), user.id, article_id).await? {
        return Err(AppError::NotFound);
    }
    Ok((
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        issue_url(&state, user.id, article_id)?,
    )
        .into_response())
}

pub async fn page(
    State(state): State<AppState>,
    Path(article_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<TicketQuery>,
) -> Result<Response, AppError> {
    let ticket: LivePageTicket = open(&state.live_view_tokens, &query.ticket)?;
    if ticket.article_id != article_id
        || ticket.expires_at < OffsetDateTime::now_utc().unix_timestamp()
        || !articles::user_can_access(state.db.pool(), ticket.user_id, article_id).await?
    {
        return Err(AppError::NotFound);
    }
    let article = articles::find_by_id(state.db.pool(), article_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let canonical_url = state.settings.tracking_url_policy.clean(&article.url);
    // Absolute RustSS origin so proxied resource URLs escape the publisher
    // `<base href>` and resolve back to this instance.
    let origin = public_origin(&headers, &state.settings);
    let cache_key = format!("shared:live-view:page:{article_id}");
    let fetched = cached_page(&state, &cache_key, article_id, &canonical_url).await;

    let html = match fetched {
        Some(page) => rewrite_publisher_html(
            &page.html,
            &page.final_url,
            article_id,
            &state.live_view_tokens,
            &origin,
        )
        .unwrap_or_else(|error| {
            tracing::warn!(%article_id, error = %error, "live view HTML rewrite failed");
            fallback_document(
                &article,
                &canonical_url,
                &state.settings.tracking_url_policy,
                &state.settings.tracking_pixel_policy,
                "The live page could not be prepared.",
            )
        }),
        None => fallback_document(
            &article,
            &canonical_url,
            &state.settings.tracking_url_policy,
            &state.settings.tracking_pixel_policy,
            "The publisher is temporarily unavailable. Showing RustSS's stored copy.",
        ),
    };

    Ok(html_response(html))
}

pub async fn resource(
    State(state): State<AppState>,
    Path(article_id): Path<Uuid>,
    Query(query): Query<TicketQuery>,
) -> Result<Response, AppError> {
    let ticket: LiveResourceTicket = open(&state.live_view_tokens, &query.ticket)?;
    if ticket.article_id != article_id
        || ticket.expires_at < OffsetDateTime::now_utc().unix_timestamp()
    {
        return Err(AppError::NotFound);
    }
    state
        .fetch
        .validate_url(&ticket.url)
        .map_err(|_| AppError::NotFound)?;

    let cache_key = format!(
        "shared:live-view:resource:{}:{}",
        ticket.kind.cache_name(),
        sha256_hex(ticket.url.as_bytes())
    );
    let mut fetched = cached_resource(&state, &cache_key, article_id, &ticket.url, ticket.kind)
        .await
        .ok_or_else(|| AppError::Internal(anyhow!("publisher resource is unavailable")))?;

    if fetched.content_type.starts_with("text/css") {
        let css = String::from_utf8(BASE64.decode(&fetched.body).context("invalid cached CSS")?)
            .context("publisher CSS is not UTF-8")?;
        // Nested url()s inside an already same-origin stylesheet resolve against
        // the stylesheet URL, so path-absolute proxy links are correct here.
        let rewritten =
            rewrite_css_urls(&css, &ticket.url, article_id, &state.live_view_tokens, "")?;
        fetched.body = BASE64.encode(rewritten);
    }

    resource_response(fetched, ticket.kind)
}

async fn cached_page(
    state: &AppState,
    cache_key: &str,
    article_id: Uuid,
    source_url: &str,
) -> Option<FetchedPage> {
    if let Some(cached) = state.cache.get::<CachedPage>(cache_key).await {
        return cached.page;
    }
    let lock = state.live_view_fetch_lock(cache_key).await;
    let _guard = lock.lock().await;
    if let Some(cached) = state.cache.get::<CachedPage>(cache_key).await {
        return cached.page;
    }

    let (cached, ttl) = match fetch_page(state, article_id, source_url).await {
        Ok(page) => (CachedPage { page: Some(page) }, LIVE_PAGE_CACHE_TTL),
        Err(error) => {
            tracing::warn!(%article_id, error = %error, "live view publisher fetch failed");
            (CachedPage { page: None }, LIVE_FAILURE_CACHE_TTL)
        }
    };
    state.cache.set(cache_key, &cached, ttl).await;
    cached.page
}

async fn cached_resource(
    state: &AppState,
    cache_key: &str,
    article_id: Uuid,
    source_url: &str,
    kind: ResourceKind,
) -> Option<FetchedResource> {
    if let Some(cached) = state.cache.get::<CachedResource>(cache_key).await {
        return cached.resource;
    }
    let lock = state.live_view_fetch_lock(cache_key).await;
    let _guard = lock.lock().await;
    if let Some(cached) = state.cache.get::<CachedResource>(cache_key).await {
        return cached.resource;
    }

    let (cached, ttl) = match fetch_resource(state, article_id, source_url, kind).await {
        Ok(resource) => (
            CachedResource {
                resource: Some(resource),
            },
            LIVE_RESOURCE_CACHE_TTL,
        ),
        Err(error) => {
            tracing::warn!(url = %source_url, error = %error, "live view resource fetch failed");
            (CachedResource { resource: None }, LIVE_FAILURE_CACHE_TTL)
        }
    };
    state.cache.set(cache_key, &cached, ttl).await;
    cached.resource
}

async fn fetch_page(
    state: &AppState,
    article_id: Uuid,
    source_url: &str,
) -> anyhow::Result<FetchedPage> {
    let article = articles::find_by_id(state.db.pool(), article_id)
        .await?
        .ok_or_else(|| anyhow!("article unavailable"))?;
    let fetch = state.fetch_for_feed(article.feed_id).await?;
    let response = fetch
        .get(source_url)
        .await?
        .header(
            header::ACCEPT,
            "text/html,application/xhtml+xml;q=0.9,*/*;q=0.2",
        )
        .send()
        .await?;
    if !response.status().is_success() {
        bail!("publisher returned HTTP {}", response.status());
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !content_type.is_empty()
        && !content_type.contains("text/html")
        && !content_type.contains("application/xhtml+xml")
    {
        bail!("publisher response is not HTML");
    }
    let final_url = response.url().to_string();
    let body = read_capped(response, MAX_PAGE_BYTES).await?;
    Ok(FetchedPage {
        // Decode using the page's declared charset so non-UTF-8 publisher pages
        // don't render as mojibake in Live View (FND-193, §6.1).
        html: rustss_core::charset::decode_html_bytes(&body, Some(&content_type)),
        final_url,
    })
}

async fn fetch_resource(
    state: &AppState,
    article_id: Uuid,
    source_url: &str,
    kind: ResourceKind,
) -> anyhow::Result<FetchedResource> {
    let article = articles::find_by_id(state.db.pool(), article_id)
        .await?
        .ok_or_else(|| anyhow!("article unavailable"))?;
    let fetch = state.fetch_for_feed(article.feed_id).await?;
    let response = fetch
        .get(source_url)
        .await?
        .header(header::ACCEPT, "*/*")
        .send()
        .await?;
    if !response.status().is_success() {
        bail!("publisher resource returned HTTP {}", response.status());
    }
    let upstream_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let content_type = safe_resource_content_type(kind, &upstream_type)?;
    let body = read_capped(response, MAX_RESOURCE_BYTES).await?;
    Ok(FetchedResource {
        body: BASE64.encode(body),
        content_type,
    })
}

async fn read_capped(mut response: reqwest::Response, max: usize) -> anyhow::Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > max as u64)
    {
        bail!("publisher response exceeds the live-view size limit");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > max {
            bail!("publisher response exceeds the live-view size limit");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Attributes lazy-loading libraries use to defer the real media URL until their
/// own JavaScript promotes it. Live View runs in an isolated sandbox where that
/// script rarely executes, so we promote the deferred source ourselves.
const LAZY_SRC_ATTRS: [&str; 8] = [
    "data-src",
    "data-lazy-src",
    "data-original",
    "data-original-src",
    "data-actualsrc",
    "data-hi-res-src",
    "data-lazy",
    "data-url",
];
const LAZY_SRCSET_ATTRS: [&str; 3] = ["data-srcset", "data-lazy-srcset", "data-original-srcset"];

fn rewrite_publisher_html(
    html: &str,
    base_url: &str,
    article_id: Uuid,
    cryptor: &Cryptor,
    origin: &str,
) -> anyhow::Result<String> {
    let base = Url::parse(base_url).context("invalid final publisher URL")?;
    let base_for_head = base.to_string();
    let script_base = base.clone();
    let style_base = base.clone();
    let media_base = base.clone();
    let video_base = base.clone();
    let inline_style_base = base.clone();
    let style_tag_base = base.clone();
    let anchor_base = base.clone();
    let form_base = base;
    let script_tokens = cryptor.clone();
    let style_tokens = cryptor.clone();
    let media_tokens = cryptor.clone();
    let video_tokens = cryptor.clone();
    let inline_style_tokens = cryptor.clone();
    let style_tag_tokens = cryptor.clone();
    let script_origin = origin.to_string();
    let style_origin = origin.to_string();
    let media_origin = origin.to_string();
    let video_origin = origin.to_string();
    let inline_style_origin = origin.to_string();
    let style_tag_origin = origin.to_string();

    rewrite_str(
        html,
        RewriteStrSettings::new()
            .append_element_content_handler(element!("base", |element| {
                element.remove();
                Ok(())
            }))
            .append_element_content_handler(element!("meta[http-equiv]", |element| {
                let directive = element
                    .get_attribute("http-equiv")
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if matches!(
                    directive.as_str(),
                    "content-security-policy" | "x-frame-options" | "refresh"
                ) {
                    element.remove();
                }
                Ok(())
            }))
            .append_element_content_handler(element!("head", move |element| {
                element.prepend(
                    &format!(
                        "<base href=\"{}\"><meta name=\"referrer\" content=\"no-referrer\">",
                        base_for_head
                    ),
                    ContentType::Html,
                );
                Ok(())
            }))
            .append_element_content_handler(element!("script[src]", move |element| {
                rewrite_resource_attribute(
                    element,
                    "src",
                    &script_base,
                    article_id,
                    &script_tokens,
                    ResourceKind::Script,
                    &script_origin,
                )
            }))
            .append_element_content_handler(element!("link[href]", move |element| {
                let stylesheet = element
                    .get_attribute("rel")
                    .is_some_and(|rel| rel.to_ascii_lowercase().contains("stylesheet"));
                if stylesheet {
                    rewrite_resource_attribute(
                        element,
                        "href",
                        &style_base,
                        article_id,
                        &style_tokens,
                        ResourceKind::Stylesheet,
                        &style_origin,
                    )?;
                }
                Ok(())
            }))
            .append_element_content_handler(element!("img, source", move |element| {
                // Promote deferred/lazy sources so images render without the
                // publisher's own JavaScript, then proxy them like any asset.
                let deferred_src = LAZY_SRC_ATTRS.iter().find_map(|attr| {
                    element
                        .get_attribute(attr)
                        .filter(|value| !value.trim().is_empty())
                });
                let chosen_src = deferred_src.or_else(|| {
                    element
                        .get_attribute("src")
                        .filter(|value| !value.trim().is_empty())
                });
                if let Some(raw) = chosen_src {
                    if let Some(absolute) = resolve_public_url(&media_base, &raw) {
                        element.set_attribute(
                            "src",
                            &resource_url(
                                &media_tokens,
                                article_id,
                                absolute.as_str(),
                                ResourceKind::Asset,
                                &media_origin,
                            )?,
                        )?;
                    }
                }
                let deferred_srcset = LAZY_SRCSET_ATTRS.iter().find_map(|attr| {
                    element
                        .get_attribute(attr)
                        .filter(|value| !value.trim().is_empty())
                });
                let chosen_srcset = deferred_srcset.or_else(|| {
                    element
                        .get_attribute("srcset")
                        .filter(|value| !value.trim().is_empty())
                });
                if let Some(raw) = chosen_srcset {
                    let rewritten = rewrite_srcset(
                        &raw,
                        &media_base,
                        article_id,
                        &media_tokens,
                        &media_origin,
                    )?;
                    element.set_attribute("srcset", &rewritten)?;
                }
                // Drop the now-redundant deferral hints so any lazy-loader
                // script that survives the sandbox cannot re-point src/srcset at
                // an un-proxied publisher URL. Load eagerly, too.
                for attr in LAZY_SRC_ATTRS.iter().chain(LAZY_SRCSET_ATTRS.iter()) {
                    element.remove_attribute(attr);
                }
                element.remove_attribute("loading");
                Ok(())
            }))
            .append_element_content_handler(element!(
                "video[src],audio[src],video[poster]",
                move |element| {
                    let attribute = if element.has_attribute("src") {
                        "src"
                    } else {
                        "poster"
                    };
                    rewrite_resource_attribute(
                        element,
                        attribute,
                        &video_base,
                        article_id,
                        &video_tokens,
                        ResourceKind::Asset,
                        &video_origin,
                    )
                }
            ))
            .append_element_content_handler(element!("[style]", move |element| {
                let Some(style) = element.get_attribute("style") else {
                    return Ok(());
                };
                element.set_attribute(
                    "style",
                    &rewrite_css_urls(
                        &style,
                        inline_style_base.as_str(),
                        article_id,
                        &inline_style_tokens,
                        &inline_style_origin,
                    )?,
                )?;
                Ok(())
            }))
            .append_element_content_handler(text!("style", move |chunk| {
                let rewritten = rewrite_css_urls(
                    chunk.as_str(),
                    style_tag_base.as_str(),
                    article_id,
                    &style_tag_tokens,
                    &style_tag_origin,
                )?;
                chunk.replace(&rewritten, ContentType::Html);
                Ok(())
            }))
            .append_element_content_handler(element!("a[href]", move |element| {
                if let Some(href) = element.get_attribute("href") {
                    if let Some(absolute) = resolve_public_url(&anchor_base, &href) {
                        element.set_attribute("href", absolute.as_str())?;
                    }
                }
                element.set_attribute("target", "_blank")?;
                element.set_attribute("rel", "noopener noreferrer")?;
                Ok(())
            }))
            .append_element_content_handler(element!("form", move |element| {
                if let Some(action) = element.get_attribute("action") {
                    if let Some(absolute) = resolve_public_url(&form_base, &action) {
                        element.set_attribute("action", absolute.as_str())?;
                    }
                }
                element.set_attribute("target", "_blank")?;
                Ok(())
            })),
    )
    .map_err(|error| anyhow!(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
fn rewrite_resource_attribute(
    element: &mut lol_html::html_content::Element<'_, '_>,
    attribute: &str,
    base: &Url,
    article_id: Uuid,
    cryptor: &Cryptor,
    kind: ResourceKind,
    origin: &str,
) -> lol_html::HandlerResult {
    let Some(value) = element.get_attribute(attribute) else {
        return Ok(());
    };
    if let Some(absolute) = resolve_public_url(base, &value) {
        element.set_attribute(
            attribute,
            &resource_url(cryptor, article_id, absolute.as_str(), kind, origin)?,
        )?;
    }
    Ok(())
}

fn rewrite_srcset(
    srcset: &str,
    base: &Url,
    article_id: Uuid,
    cryptor: &Cryptor,
    origin: &str,
) -> anyhow::Result<String> {
    if srcset
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("data:")
    {
        return Ok(srcset.to_string());
    }
    let mut candidates = Vec::new();
    for candidate in srcset.split(',') {
        let mut parts = candidate.split_whitespace();
        let Some(source) = parts.next() else {
            continue;
        };
        let descriptor = parts.collect::<Vec<_>>().join(" ");
        let rewritten = match resolve_public_url(base, source) {
            Some(url) => resource_url(
                cryptor,
                article_id,
                url.as_str(),
                ResourceKind::Asset,
                origin,
            )?,
            None => source.to_string(),
        };
        candidates.push(if descriptor.is_empty() {
            rewritten
        } else {
            format!("{rewritten} {descriptor}")
        });
    }
    Ok(candidates.join(", "))
}

fn rewrite_css_urls(
    css: &str,
    base_url: &str,
    article_id: Uuid,
    cryptor: &Cryptor,
    origin: &str,
) -> anyhow::Result<String> {
    let Ok(base) = Url::parse(base_url) else {
        return Ok(css.to_string());
    };
    let lower = css.to_ascii_lowercase();
    let mut output = String::with_capacity(css.len());
    let mut cursor = 0;
    while let Some(relative_start) = lower[cursor..].find("url(") {
        let start = cursor + relative_start;
        output.push_str(&css[cursor..start + 4]);
        let value_start = start + 4;
        let Some(relative_end) = css[value_start..].find(')') else {
            output.push_str(&css[value_start..]);
            return Ok(output);
        };
        let end = value_start + relative_end;
        let raw = css[value_start..end].trim().trim_matches(['\'', '"']);
        if let Some(url) = resolve_public_url(&base, raw) {
            output.push('"');
            output.push_str(&resource_url(
                cryptor,
                article_id,
                url.as_str(),
                ResourceKind::Asset,
                origin,
            )?);
            output.push('"');
        } else {
            output.push_str(&css[value_start..end]);
        }
        output.push(')');
        cursor = end + 1;
    }
    output.push_str(&css[cursor..]);
    Ok(output)
}

fn resolve_public_url(base: &Url, value: &str) -> Option<Url> {
    let value = value.trim();
    if value.is_empty()
        || value.starts_with('#')
        || value.starts_with("data:")
        || value.starts_with("blob:")
        || value.starts_with("javascript:")
        || value.starts_with("mailto:")
        || value.starts_with("tel:")
    {
        return None;
    }
    base.join(value)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
}

fn resource_url(
    cryptor: &Cryptor,
    article_id: Uuid,
    url: &str,
    kind: ResourceKind,
    origin: &str,
) -> anyhow::Result<String> {
    let ticket = seal(
        cryptor,
        &LiveResourceTicket {
            article_id,
            url: url.to_string(),
            kind,
            expires_at: resource_ticket_expiry(),
        },
    )?;
    // `origin` is the absolute RustSS origin when rewriting the top-level
    // document (so proxied URLs bypass the publisher `<base href>`), or empty
    // when rewriting an already same-origin stylesheet (path-absolute is fine).
    Ok(format!(
        "{origin}/articles/{article_id}/live/resource?ticket={}",
        percent_encode(&ticket)
    ))
}

fn ticket_expiry() -> i64 {
    (OffsetDateTime::now_utc() + LIVE_TICKET_LIFETIME).unix_timestamp()
}

fn resource_ticket_expiry() -> i64 {
    (OffsetDateTime::now_utc() + LIVE_RESOURCE_TICKET_LIFETIME).unix_timestamp()
}

fn seal<T: Serialize>(cryptor: &Cryptor, value: &T) -> anyhow::Result<String> {
    let payload = serde_json::to_vec(value).context("could not serialize live-view ticket")?;
    Ok(cryptor.encrypt(&payload)?)
}

fn open<T: DeserializeOwned>(cryptor: &Cryptor, ticket: &str) -> Result<T, AppError> {
    let payload = cryptor.decrypt(ticket).map_err(|_| AppError::NotFound)?;
    serde_json::from_slice(&payload).map_err(|_| AppError::NotFound)
}

fn percent_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// Best-effort external origin (`scheme://host`) the browser used to reach us,
/// preferring forwarded proxy headers, then the request `Host`, then the
/// configured base URL. Used to make Live View proxy links absolute so they
/// escape the injected publisher `<base href>`.
fn public_origin(headers: &HeaderMap, settings: &crate::state::AppSettings) -> String {
    fn first(headers: &HeaderMap, name: &str) -> Option<String> {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    if let Some(host) = first(headers, "x-forwarded-host").or_else(|| first(headers, "host")) {
        let scheme = first(headers, "x-forwarded-proto")
            .map(|scheme| scheme.to_ascii_lowercase())
            .filter(|scheme| scheme == "http" || scheme == "https")
            .unwrap_or_else(|| {
                if settings.base_url.starts_with("https") {
                    "https".to_string()
                } else {
                    "http".to_string()
                }
            });
        return format!("{scheme}://{host}");
    }
    settings.base_url.trim_end_matches('/').to_string()
}

fn safe_resource_content_type(kind: ResourceKind, upstream: &str) -> anyhow::Result<String> {
    match kind {
        ResourceKind::Script => Ok("application/javascript; charset=utf-8".to_string()),
        ResourceKind::Stylesheet => Ok("text/css; charset=utf-8".to_string()),
        ResourceKind::Asset
            if upstream.starts_with("image/")
                || upstream.starts_with("font/")
                || upstream.starts_with("audio/")
                || upstream.starts_with("video/")
                || upstream == "text/css"
                || matches!(
                    upstream,
                    "application/font-woff"
                        | "application/font-woff2"
                        | "application/vnd.ms-fontobject"
                        | "application/octet-stream"
                ) =>
        {
            Ok(if upstream.is_empty() {
                "application/octet-stream".to_string()
            } else {
                upstream.to_string()
            })
        }
        ResourceKind::Asset => bail!("publisher resource type is not safe for Live View"),
    }
}

fn html_response(html: String) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(LIVE_CSP),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    (headers, html).into_response()
}

fn resource_response(fetched: FetchedResource, kind: ResourceKind) -> Result<Response, AppError> {
    let body = BASE64
        .decode(fetched.body)
        .context("invalid cached live-view resource")?;
    let content_type = safe_resource_content_type(kind, &fetched.content_type)?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type).context("invalid publisher content type")?,
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=900"),
    );
    Ok((headers, Body::from(body)).into_response())
}

fn fallback_document(
    article: &Article,
    canonical_url: &str,
    tracking_url_policy: &rustss_core::tracking_url::TrackingUrlPolicy,
    tracking_pixel_policy: &rustss_core::tracking_pixel::TrackingPixelPolicy,
    message: &str,
) -> String {
    let title = escape_html(&article.title);
    let url = escape_html(canonical_url);
    let content = article
        .article_page_content
        .as_deref()
        .or(article.feed_content.as_deref())
        .unwrap_or(&article.content);
    let content = rustss_core::html::sanitize_article_html_with_policies(
        content,
        canonical_url,
        tracking_url_policy,
        tracking_pixel_policy,
    )
    .html;
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title><style>body{{max-width:48rem;margin:0 auto;padding:clamp(1rem,4vw,3rem);font:18px/1.7 system-ui,sans-serif;color:#202124;background:#fff}}.notice{{padding:.8rem 1rem;border:1px solid #d8dee8;border-radius:.7rem;background:#f6f8fb;color:#4b5563}}img,video{{max-width:100%;height:auto}}a{{color:#1769aa}}</style></head><body><p class=\"notice\">{} <a href=\"{url}\" target=\"_blank\" rel=\"noopener noreferrer\">Open the publisher page</a>.</p><h1>{title}</h1><main>{content}</main></body></html>",
        escape_html(message)
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attribute_value_containing(html: &str, name: &str, needle: &str) -> Option<String> {
        let marker = format!("{name}=\"");
        html.split(&marker)
            .skip(1)
            .filter_map(|fragment| fragment.split('"').next())
            .find(|value| value.contains(needle))
            .map(|value| value.replace("&amp;", "&"))
    }

    #[test]
    fn rewrites_framing_policy_and_static_resources() {
        let cryptor = Cryptor::new(b"test-live-view-secret");
        let article_id = Uuid::now_v7();
        let rewritten = rewrite_publisher_html(
            r#"<!doctype html><html><head><base href="https://wrong.test/"><meta http-equiv="Content-Security-Policy" content="frame-ancestors 'none'"><link rel="stylesheet" href="/site.css"><style>.hero{background:url('cover.jpg')}</style></head><body><script src="app.js"></script><img src="hero.jpg"><img data-src="lazy.jpg" loading="lazy"><div style="background:url('hero-bg.jpg')"></div><a href="/story">Story</a></body></html>"#,
            "https://publisher.example/posts/one",
            article_id,
            &cryptor,
            "https://rustss.example",
        )
        .unwrap();

        assert!(rewritten.contains("<base href=\"https://publisher.example/posts/one\">"));
        assert!(!rewritten.contains("frame-ancestors 'none'"));
        // Proxy links are absolute to the RustSS origin so they bypass the base.
        assert!(rewritten.contains(&format!(
            "https://rustss.example/articles/{article_id}/live/resource?ticket="
        )));
        assert!(!rewritten.contains("hero-bg.jpg"));
        assert!(!rewritten.contains("cover.jpg"));
        // Deferred/lazy image source was promoted and proxied; the eager hint removed.
        assert!(!rewritten.contains("lazy.jpg"));
        assert!(!rewritten.contains("loading=\"lazy\""));
        assert!(rewritten.contains("href=\"https://publisher.example/story\""));
        assert!(rewritten.contains("target=\"_blank\""));
    }

    #[test]
    fn live_ticket_is_article_scoped_and_tamper_evident() {
        let cryptor = Cryptor::new(b"test-live-view-secret");
        let value = LivePageTicket {
            article_id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            expires_at: ticket_expiry(),
        };
        let token = seal(&cryptor, &value).unwrap();
        let opened: LivePageTicket = open(&cryptor, &token).unwrap();
        assert_eq!(opened.article_id, value.article_id);

        let mut tampered = token.into_bytes();
        let last = tampered.len() - 1;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
        assert!(open::<LivePageTicket>(&cryptor, &String::from_utf8(tampered).unwrap()).is_err());
    }

    #[test]
    fn rewrites_nested_css_assets() {
        let cryptor = Cryptor::new(b"test-live-view-secret");
        let article_id = Uuid::now_v7();
        let css = rewrite_css_urls(
            "body{background:url('../img/paper.png')} .x{mask:url(data:image/png;base64,abc)}",
            "https://publisher.example/css/site.css",
            article_id,
            &cryptor,
            "",
        )
        .unwrap();
        assert!(css.contains(&format!("/articles/{article_id}/live/resource?ticket=")));
        assert!(css.contains("data:image/png;base64,abc"));
    }

    #[tokio::test]
    async fn route_bypasses_framing_headers_and_reuses_shared_fetches() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        use rustss_cache::Cache;
        use rustss_db::models::NewArticle;
        use rustss_db::{feeds, subscriptions, users, Db};
        use rustss_feeds::SafeFetchClient;

        let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let db = Db::connect(&database_url).await.unwrap();
        db.migrate().await.unwrap();
        let publisher = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/story"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(StdDuration::from_millis(50))
                    .insert_header("x-frame-options", "DENY")
                    .set_body_raw(
                        r#"<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="frame-ancestors 'none'"><link rel="stylesheet" href="/site.css"></head><body><h1>LIVE PUBLISHER SENTINEL</h1><script src="/app.js"></script></body></html>"#,
                        "text/html",
                    ),
            )
            .mount(&publisher)
            .await;
        Mock::given(method("GET"))
            .and(path("/site.css"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("body{background-image:url('/paper.png')}", "text/css"),
            )
            .mount(&publisher)
            .await;

        let state = AppState::new(
            db,
            Cache::in_memory().await,
            crate::AppSettings::default(),
            String::new,
        )
        .with_fetch(SafeFetchClient::new_allow_private(StdDuration::from_secs(10)).unwrap());
        let pool = state.db.pool();
        let user = users::create(
            pool,
            &format!("live-view-{}@example.com", Uuid::now_v7()),
            None,
            None,
            "user",
            None,
        )
        .await
        .unwrap();
        let (feed, _) = feeds::upsert_by_url(
            pool,
            &format!("https://live-view.example/{}.xml", Uuid::now_v7()),
            "Live View feed",
            None,
        )
        .await
        .unwrap();
        subscriptions::subscribe(pool, user.id, feed.id, None)
            .await
            .unwrap();
        let (article, _) = articles::upsert(
            pool,
            &NewArticle {
                feed_id: feed.id,
                url: format!("{}/story", publisher.uri()),
                comments_url: None,
                guid: None,
                title: "Live View article".into(),
                author: None,
                content: "<p>Stored fallback</p>".into(),
                excerpt: None,
                image_url: None,
                enclosures: Vec::new(),
                published_at: None,
            },
        )
        .await
        .unwrap();
        let live_url = issue_url(&state, user.id, article.id).unwrap();
        let app = crate::router(state);

        let first = app
            .clone()
            .oneshot(Request::get(&live_url).body(Body::empty()).unwrap());
        let second = app
            .clone()
            .oneshot(Request::get(&live_url).body(Body::empty()).unwrap());
        let (first, second) = tokio::join!(first, second);
        let mut stylesheet_url = None;
        for response in [first.unwrap(), second.unwrap()] {
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .unwrap()
                .to_str()
                .unwrap()
                .contains("sandbox allow-scripts"));
            let html =
                String::from_utf8_lossy(&response.into_body().collect().await.unwrap().to_bytes())
                    .to_string();
            assert!(html.contains("LIVE PUBLISHER SENTINEL"), "{html}");
            assert!(!html.contains("frame-ancestors 'none'"));
            stylesheet_url = attribute_value_containing(&html, "href", "/live/resource?ticket=");
        }

        let stylesheet = app
            .oneshot(
                Request::get(stylesheet_url.expect("stylesheet should be rewritten"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stylesheet.status(), StatusCode::OK);
        assert_eq!(
            stylesheet.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/css; charset=utf-8"
        );
        let css =
            String::from_utf8_lossy(&stylesheet.into_body().collect().await.unwrap().to_bytes())
                .to_string();
        assert!(css.contains(&format!("/articles/{}/live/resource?ticket=", article.id)));

        let requests = publisher.received_requests().await.unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.url.path() == "/story")
                .count(),
            1,
            "the shared page cache must deduplicate readers"
        );
    }
}
