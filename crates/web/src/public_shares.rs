//! Revocable public views of stored, sanitized shared articles.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{header, HeaderName, HeaderValue};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use lol_html::{element, rewrite_str, RewriteStrSettings};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::Deserialize;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use rusty_feed_core::hash::sha256_hex;
use rusty_feed_core::html::{sanitize_article_html_with_policies, ARTICLE_PAGE_PROCESSING_VERSION};
use rusty_feed_db::{articles, feeds, public_shares};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

const SHARE_TOKEN_BYTES: usize = 32;
const MAX_SLUG_LEN: usize = 80;

#[derive(Clone)]
pub(crate) struct ShareControls {
    pub enabled: bool,
    pub active: bool,
    pub url: Option<String>,
    pub slug: String,
    pub content_source: String,
    pub show_title: bool,
    pub show_metadata: bool,
    pub show_source_link: bool,
    pub include_images: bool,
    pub lifetime_days: u16,
    pub infinite: bool,
    pub allow_infinite: bool,
    pub max_lifetime_days: u16,
    pub expires_label: Option<String>,
    pub token_unavailable: bool,
}

#[derive(Template)]
#[template(path = "public_article.html")]
struct PublicArticleTemplate {
    browser_title: String,
    show_title: bool,
    title: String,
    show_metadata: bool,
    feed_name: String,
    author: Option<String>,
    published: Option<String>,
    source_url: Option<String>,
    content: String,
    images_omitted: bool,
    using_rss_fallback: bool,
}

#[derive(Deserialize)]
pub struct ShareForm {
    csrf_token: String,
    #[serde(default)]
    slug: String,
    content_source: String,
    #[serde(default)]
    lifetime_days: Option<u16>,
    #[serde(default)]
    infinite: Option<String>,
    #[serde(default)]
    show_title: Option<String>,
    #[serde(default)]
    show_metadata: Option<String>,
    #[serde(default)]
    show_source_link: Option<String>,
    #[serde(default)]
    include_images: Option<String>,
}

#[derive(Deserialize)]
pub struct RevokeForm {
    csrf_token: String,
}

pub(crate) async fn controls(
    state: &AppState,
    user_id: Uuid,
    article_id: Uuid,
    article_title: &str,
    default_content_source: &str,
) -> Result<ShareControls, AppError> {
    let default_slug = slugify(article_title);
    if !state.settings.public_article_sharing_enabled {
        return Ok(ShareControls {
            enabled: false,
            active: false,
            url: None,
            slug: default_slug,
            content_source: default_content_source.to_string(),
            show_title: true,
            show_metadata: true,
            show_source_link: true,
            include_images: false,
            lifetime_days: state.settings.public_share_default_lifetime_days,
            infinite: false,
            allow_infinite: state.settings.public_share_allow_infinite,
            max_lifetime_days: state.settings.public_share_max_lifetime_days,
            expires_label: None,
            token_unavailable: false,
        });
    }

    let row = public_shares::find_for_user(state.db.pool(), user_id, article_id).await?;
    let now = OffsetDateTime::now_utc();
    let active = row.as_ref().is_some_and(|share| {
        share.revoked_at.is_none() && share.expires_at.is_none_or(|at| at > now)
    });
    let url = if active {
        row.as_ref()
            .and_then(|share| {
                state
                    .public_share_tokens
                    .decrypt_str(&share.token_ciphertext)
                    .ok()
            })
            .map(|token| {
                share_url(
                    state,
                    &token,
                    row.as_ref().map_or("article", |share| &share.slug),
                )
            })
    } else {
        None
    };
    let token_unavailable = active && url.is_none();
    let expires_label = row.as_ref().and_then(|share| {
        if share.revoked_at.is_some() {
            Some("Revoked".to_string())
        } else {
            share
                .expires_at
                .map(|at| {
                    if at <= now {
                        "Expired".to_string()
                    } else {
                        format!("Expires {}", at.date())
                    }
                })
                .or_else(|| Some("Does not expire".to_string()))
        }
    });
    let lifetime_days = row
        .as_ref()
        .and_then(|share| share.expires_at)
        .map(|expires| {
            let remaining = (expires - now).whole_days().saturating_add(1).max(1);
            u16::try_from(remaining)
                .unwrap_or(state.settings.public_share_max_lifetime_days)
                .min(state.settings.public_share_max_lifetime_days)
        })
        .unwrap_or(state.settings.public_share_default_lifetime_days);

    Ok(ShareControls {
        enabled: true,
        active,
        url,
        slug: row
            .as_ref()
            .map_or(default_slug, |share| share.slug.clone()),
        content_source: row.as_ref().map_or_else(
            || default_content_source.to_string(),
            |share| share.content_source.clone(),
        ),
        show_title: row.as_ref().is_none_or(|share| share.show_title),
        show_metadata: row.as_ref().is_none_or(|share| share.show_metadata),
        show_source_link: row.as_ref().is_none_or(|share| share.show_source_link),
        include_images: row.as_ref().is_some_and(|share| share.include_images),
        lifetime_days,
        infinite: row.as_ref().is_some_and(|share| share.expires_at.is_none()),
        allow_infinite: state.settings.public_share_allow_infinite,
        max_lifetime_days: state.settings.public_share_max_lifetime_days,
        expires_label,
        token_unavailable,
    })
}

/// Create or rotate a public article link.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(article_id): Path<Uuid>,
    Form(form): Form<ShareForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !state.settings.public_article_sharing_enabled {
        return Err(AppError::NotFound);
    }
    if !articles::user_can_access(state.db.pool(), user.id, article_id).await? {
        return Err(AppError::NotFound);
    }
    crate::verify::deny_demo(&user)?;
    crate::verify::require_verified(&state, &user)?;
    crate::quota::assert_within(
        &state,
        user.id,
        rusty_feed_core::quotas::QuotaKind::PublicShares,
    )
    .await?;
    let article = articles::find_by_id(state.db.pool(), article_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let content_source = match form.content_source.as_str() {
        "rss" => "rss",
        "article_page" => "article_page",
        _ => {
            return Err(AppError::Validation(
                "Choose RSS or public-page content.".into(),
            ))
        }
    };
    let infinite = form.infinite.is_some();
    let lifetime_days = form
        .lifetime_days
        .unwrap_or(state.settings.public_share_default_lifetime_days);
    if infinite && !state.settings.public_share_allow_infinite {
        return Err(AppError::Validation(
            "This instance does not allow non-expiring public links.".into(),
        ));
    }
    if !infinite && !(1..=state.settings.public_share_max_lifetime_days).contains(&lifetime_days) {
        return Err(AppError::Validation(format!(
            "Share lifetime must be between 1 and {} days.",
            state.settings.public_share_max_lifetime_days
        )));
    }
    let expires_at =
        (!infinite).then(|| OffsetDateTime::now_utc() + Duration::days(i64::from(lifetime_days)));
    let requested_slug = if form.slug.trim().is_empty() {
        &article.title
    } else {
        &form.slug
    };
    let slug = slugify(requested_slug);
    let mut token_bytes = [0_u8; SHARE_TOKEN_BYTES];
    OsRng.fill_bytes(&mut token_bytes);
    let token = URL_SAFE_NO_PAD.encode(token_bytes);
    let token_hash = sha256_hex(token.as_bytes());
    let token_ciphertext = state
        .public_share_tokens
        .encrypt_str(&token)
        .map_err(|error| AppError::Internal(anyhow::anyhow!(error)))?;
    public_shares::upsert(
        state.db.pool(),
        user.id,
        article_id,
        public_shares::PublicShareDraft {
            token_hash: &token_hash,
            token_ciphertext: &token_ciphertext,
            slug: &slug,
            content_source,
            show_title: form.show_title.is_some(),
            show_metadata: form.show_metadata.is_some(),
            show_source_link: form.show_source_link.is_some(),
            include_images: form.include_images.is_some(),
            expires_at,
        },
    )
    .await?;
    Ok(Redirect::to(&format!("/articles/{article_id}")).into_response())
}

/// Revoke the caller-owned public link immediately.
pub async fn revoke(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(article_id): Path<Uuid>,
    Form(form): Form<RevokeForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !articles::user_can_access(state.db.pool(), user.id, article_id).await? {
        return Err(AppError::NotFound);
    }
    public_shares::revoke(state.db.pool(), user.id, article_id).await?;
    Ok(Redirect::to(&format!("/articles/{article_id}")).into_response())
}

/// Render a token-authorized article without session or user state.
pub async fn show(
    State(state): State<AppState>,
    Path((token, _slug)): Path<(String, String)>,
) -> Result<Response, AppError> {
    if !state.settings.public_article_sharing_enabled || !valid_token(&token) {
        return Err(AppError::NotFound);
    }
    let share = public_shares::find_active_by_hash(state.db.pool(), &sha256_hex(token.as_bytes()))
        .await?
        .ok_or(AppError::NotFound)?;
    let article = articles::find_by_id(state.db.pool(), share.article_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let feed = feeds::find_by_id(state.db.pool(), article.feed_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let now = OffsetDateTime::now_utc();
    let page_is_current = article.article_page_content.is_some()
        && article.article_page_processing_version >= ARTICLE_PAGE_PROCESSING_VERSION
        && article.article_page_expires_at.is_none_or(|at| at > now);
    let use_article_page = share.content_source == "article_page" && page_is_current;
    let stored_content = if use_article_page {
        article.article_page_content.as_deref().unwrap_or_default()
    } else {
        article
            .feed_content
            .as_deref()
            .unwrap_or(article.content.as_str())
    };
    let sanitized = sanitize_article_html_with_policies(
        stored_content,
        &article.url,
        &state.settings.tracking_url_policy,
        &state.settings.tracking_pixel_policy,
    );
    let content = if share.include_images {
        sanitized.html
    } else {
        remove_embedded_media(&sanitized.html)?
    };
    let source_url = share
        .show_source_link
        .then(|| state.settings.tracking_url_policy.clean(&article.url))
        .and_then(|url| safe_http_url(&url));
    let template = PublicArticleTemplate {
        browser_title: if share.show_title {
            article.title.clone()
        } else {
            "Shared article".to_string()
        },
        show_title: share.show_title,
        title: article.title,
        show_metadata: share.show_metadata,
        feed_name: feed.name,
        author: article.author,
        published: article.published_at.map(|at| at.date().to_string()),
        source_url,
        content,
        images_omitted: !share.include_images,
        using_rss_fallback: share.content_source == "article_page" && !use_article_page,
    };
    let mut response =
        Html(template.render().map_err(|error| {
            AppError::Internal(anyhow::anyhow!("template render failed: {error}"))
        })?)
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store, max-age=0"),
    );
    response.headers_mut().insert(
        HeaderName::from_static("x-robots-tag"),
        HeaderValue::from_static("noindex, nofollow, noarchive"),
    );
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'self'; img-src http: https: data:; font-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ),
    );
    Ok(response)
}

fn share_url(state: &AppState, token: &str, slug: &str) -> String {
    format!(
        "{}/shared/{token}/{slug}",
        state.settings.base_url.trim_end_matches('/')
    )
}

fn valid_token(token: &str) -> bool {
    token.len() == 43
        && URL_SAFE_NO_PAD
            .decode(token)
            .is_ok_and(|bytes| bytes.len() == SHARE_TOKEN_BYTES)
}

fn slugify(value: &str) -> String {
    let mut output = String::with_capacity(value.len().min(MAX_SLUG_LEN));
    let mut separator = false;
    for character in value.chars() {
        if output.len() >= MAX_SLUG_LEN {
            break;
        }
        if character.is_ascii_alphanumeric() {
            if separator && !output.is_empty() {
                output.push('-');
            }
            separator = false;
            output.push(character.to_ascii_lowercase());
        } else {
            separator = true;
        }
    }
    output.truncate(MAX_SLUG_LEN);
    while output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() {
        "article".to_string()
    } else {
        output
    }
}

fn remove_embedded_media(html: &str) -> Result<String, AppError> {
    rewrite_str(
        html,
        RewriteStrSettings::new()
            .append_element_content_handler(element!("picture", |element| {
                element.remove();
                Ok(())
            }))
            .append_element_content_handler(element!("img", |element| {
                element.remove();
                Ok(())
            }))
            .append_element_content_handler(element!("video", |element| {
                element.remove();
                Ok(())
            }))
            .append_element_content_handler(element!("audio", |element| {
                element.remove();
                Ok(())
            }))
            .append_element_content_handler(element!("source", |element| {
                element.remove();
                Ok(())
            })),
    )
    .map_err(|error| AppError::Internal(anyhow::anyhow!("media removal failed: {error}")))
}

fn safe_http_url(value: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::{remove_embedded_media, slugify, valid_token, SHARE_TOKEN_BYTES};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    #[test]
    fn slug_is_bounded_and_url_safe() {
        assert_eq!(
            slugify(" A pleasant Article — 2026! "),
            "a-pleasant-article-2026"
        );
        assert_eq!(slugify("日本語"), "article");
        assert!(slugify(&"word ".repeat(100)).len() <= 80);
    }

    #[test]
    fn tokens_require_the_full_random_value() {
        assert!(valid_token(
            &URL_SAFE_NO_PAD.encode([7_u8; SHARE_TOKEN_BYTES])
        ));
        assert!(!valid_token("short"));
    }

    #[test]
    fn privacy_default_removes_remote_media_elements() {
        let html = "<p>Text</p><picture><source srcset=\"a.webp\"><img src=\"a.jpg\"></picture><audio src=\"a.mp3\"></audio>";
        let output = remove_embedded_media(html).unwrap();
        assert!(output.contains("<p>Text</p>"));
        assert!(!output.contains("img"));
        assert!(!output.contains("audio"));
    }
}
