use std::collections::BTreeSet;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use rustss_core::html::sanitize_article_html;
use rustss_db::{catalog, feed_icons, jobs, User};
use rustss_feeds::enqueue_fetch_feed;

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::catalog_view::{content_size, frequency};
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

const MAX_CATALOG_SELECTION: usize = 30;

struct Facet {
    value: String,
    selected: bool,
}

struct CatalogView {
    feed_id: String,
    initial: String,
    has_icon: bool,
    title: String,
    description: Option<String>,
    category: String,
    country: String,
    language: String,
    featured: bool,
    url: String,
    health: String,
    article_count: i64,
    average_content: String,
    last_post: String,
    frequency: String,
    subscribed: bool,
}

struct PreviewStatsView {
    sampled_article_count: i16,
    full_content_count: i16,
    partial_content_count: i16,
    http_401_count: i16,
    http_402_count: i16,
    http_403_count: i16,
    http_451_count: i16,
    robots_disallowed_count: i16,
    rate_limited_count: i16,
    reader_failed_count: i16,
    fetch_failed_count: i16,
    average_rss_content: String,
    average_processed_content: String,
    availability: String,
    checked_at: String,
}

struct PreviewArticleView {
    title: String,
    url: Option<String>,
    published: Option<String>,
    rss_content: Option<String>,
}

struct CatalogPreviewView {
    feed_id: String,
    title: String,
    description: Option<String>,
    category: String,
    country: String,
    language: String,
    show_processed: bool,
    rss_url: String,
    processed_url: String,
    stats: Option<PreviewStatsView>,
    articles: Vec<PreviewArticleView>,
    needs_analysis: bool,
    analysis_message: Option<String>,
}

#[derive(Template)]
#[template(path = "catalog.html")]
struct CatalogTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    entries: Vec<CatalogView>,
    categories: Vec<Facet>,
    countries: Vec<Facet>,
    languages: Vec<Facet>,
    message: Option<String>,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "catalog_preview.html")]
struct CatalogPreviewTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    preview: CatalogPreviewView,
}

#[derive(Deserialize)]
pub struct CatalogQuery {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Deserialize)]
pub struct PreviewQuery {
    #[serde(default)]
    view: Option<String>,
    #[serde(default)]
    analysis: Option<String>,
}

#[derive(Deserialize)]
pub struct PreviewForm {
    csrf_token: String,
}

fn selected(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn feed_initial(name: &str) -> String {
    name.chars()
        .find(|character| character.is_alphanumeric())
        .map(|character| character.to_uppercase().collect())
        .unwrap_or_else(|| "•".to_string())
}

fn view(entry: catalog::CatalogEntry, has_icon: bool) -> CatalogView {
    CatalogView {
        feed_id: entry.feed_id.to_string(),
        initial: feed_initial(&entry.title),
        has_icon,
        title: entry.title,
        description: entry.description,
        category: entry.category,
        country: entry.country,
        language: entry.language,
        featured: entry.featured,
        url: entry.url,
        health: entry.health_status,
        article_count: entry.article_count,
        average_content: content_size(entry.average_rss_content_length),
        last_post: entry
            .last_post_at
            .map(|date| date.date().to_string())
            .unwrap_or_else(|| "No dated posts yet".to_string()),
        frequency: frequency(entry.average_interval_hours),
        subscribed: entry.subscribed,
    }
}

fn preview_url(feed_id: Uuid, show_processed: bool) -> String {
    let view = if show_processed { "processed" } else { "rss" };
    format!("/catalog/{feed_id}/preview?view={view}")
}

fn safe_preview_url(value: &str) -> Option<String> {
    let mut url = url::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    url.set_fragment(None);
    Some(url.to_string())
}

fn plain_preview_text(content: &str, base_url: &str) -> String {
    let content = sanitize_article_html(content, base_url);
    let mut output = String::with_capacity(content.len());
    let mut in_tag = false;
    for character in content.chars() {
        match character {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                output.push(' ');
            }
            _ if !in_tag => output.push(character),
            _ => {}
        }
    }
    output
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(3_000)
        .collect()
}

fn preview_article(article: catalog::CatalogPreviewArticle) -> PreviewArticleView {
    let url = safe_preview_url(&article.url);
    let published = article.published_at.map(|date| date.date().to_string());
    let rss_content = article
        .feed_content
        .as_deref()
        .map(|content| plain_preview_text(content, &article.url));
    PreviewArticleView {
        title: article.title,
        url,
        published,
        rss_content,
    }
}

fn preview_stats(stats: catalog::ContentProbeStats) -> PreviewStatsView {
    let completed = stats.full_content_count
        + stats.partial_content_count
        + stats.http_401_count
        + stats.http_402_count
        + stats.http_403_count
        + stats.http_451_count
        + stats.reader_failed_count
        + stats.fetch_failed_count;
    let availability = if stats.sampled_article_count == 0 {
        "No recent articles were available to test.".to_string()
    } else if stats.rate_limited_count > 0 {
        "Sampling stopped after the publisher rate-limited the anonymous check; this result is incomplete."
            .to_string()
    } else if stats.robots_disallowed_count == stats.sampled_article_count {
        "Robots.txt disallowed every sampled public page.".to_string()
    } else if completed == 0 {
        "No public-page request could be completed for the sampled items.".to_string()
    } else if stats.full_content_count == completed {
        "A verified public article body was extracted from every completed sample.".to_string()
    } else if stats.full_content_count > 0 {
        format!(
            "A verified public article body was extracted from {} of {} completed samples.",
            stats.full_content_count, completed
        )
    } else {
        "Completed public-page samples did not yield a verified full article body.".to_string()
    };
    PreviewStatsView {
        sampled_article_count: stats.sampled_article_count,
        full_content_count: stats.full_content_count,
        partial_content_count: stats.partial_content_count,
        http_401_count: stats.http_401_count,
        http_402_count: stats.http_402_count,
        http_403_count: stats.http_403_count,
        http_451_count: stats.http_451_count,
        robots_disallowed_count: stats.robots_disallowed_count,
        rate_limited_count: stats.rate_limited_count,
        reader_failed_count: stats.reader_failed_count,
        fetch_failed_count: stats.fetch_failed_count,
        average_rss_content: content_size(stats.average_rss_body_chars.map(f64::from)),
        average_processed_content: stats
            .average_processed_body_chars
            .map(|value| content_size(Some(f64::from(value))))
            .unwrap_or_else(|| "No readable public body".to_string()),
        availability,
        checked_at: stats.checked_at.to_string(),
    }
}

fn analysis_message(value: Option<&str>) -> Option<String> {
    match value {
        Some("queued") => Some(
            "A shared public-content analysis was queued. Reload this page shortly for its result."
                .to_string(),
        ),
        Some("current") => Some("The shared preview analysis is already current.".to_string()),
        Some("no_articles") => Some(
            "This source has no recent items available for analysis yet. Return after it has refreshed."
                .to_string(),
        ),
        _ => None,
    }
}

async fn render_page(
    state: &AppState,
    user: &User,
    jar: CookieJar,
    query: CatalogQuery,
    message: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let category = selected(query.category);
    let country = selected(query.country);
    let language = selected(query.language);
    let ui = prefs::load(&state.db, user.id).await;
    let entries = catalog::list_published_for_user(
        state.db.pool(),
        user.id,
        category.as_deref(),
        country.as_deref(),
        language.as_deref(),
    )
    .await?;
    let feed_ids: Vec<Uuid> = entries.iter().map(|entry| entry.feed_id).collect();
    let icon_ids = feed_icons::available_for_feeds(state.db.pool(), &feed_ids).await?;
    let entries = entries
        .into_iter()
        .map(|entry| {
            let has_icon = icon_ids.contains(&entry.feed_id);
            view(entry, has_icon)
        })
        .collect();
    let (categories, countries, languages) = tokio::try_join!(
        catalog::categories(state.db.pool()),
        catalog::countries(state.db.pool()),
        catalog::languages(state.db.pool()),
    )?;
    let categories = categories
        .into_iter()
        .map(|value| Facet {
            selected: category.as_deref() == Some(value.as_str()),
            value,
        })
        .collect();
    let countries = countries
        .into_iter()
        .map(|value| Facet {
            selected: country.as_deref() == Some(value.as_str()),
            value,
        })
        .collect();
    let languages = languages
        .into_iter()
        .map(|value| Facet {
            selected: language.as_deref() == Some(value.as_str()),
            value,
        })
        .collect();
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = CatalogTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui,
        is_admin: user.role == "admin",
        entries,
        categories,
        countries,
        languages,
        message,
        error,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    Ok((jar, body).into_response())
}

fn parse_subscription_form(values: Vec<(String, String)>) -> Result<(String, Vec<Uuid>), String> {
    let mut csrf_token = None;
    let mut feed_ids = BTreeSet::new();
    for (name, value) in values {
        match name.as_str() {
            "csrf_token" if csrf_token.is_none() => csrf_token = Some(value),
            "feed_id" => {
                let feed_id =
                    Uuid::parse_str(&value).map_err(|_| "The selected source was invalid.")?;
                feed_ids.insert(feed_id);
            }
            _ => return Err("The catalog selection could not be processed.".to_string()),
        }
    }
    let csrf_token =
        csrf_token.ok_or_else(|| "The catalog selection could not be processed.".to_string())?;
    if feed_ids.is_empty() {
        return Err("Choose at least one source to subscribe.".to_string());
    }
    if feed_ids.len() > MAX_CATALOG_SELECTION {
        return Err(format!(
            "Choose at most {MAX_CATALOG_SELECTION} sources at once."
        ));
    }
    Ok((csrf_token, feed_ids.into_iter().collect()))
}

pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Query(query): Query<CatalogQuery>,
) -> Result<Response, AppError> {
    render_page(&state, &user, jar, query, None, None).await
}

async fn render_preview_page(
    state: &AppState,
    user: &User,
    jar: CookieJar,
    feed_id: Uuid,
    query: PreviewQuery,
) -> Result<Response, AppError> {
    let show_processed = query.view.as_deref() == Some("processed");
    let (feed, articles, stats, needs_analysis) = tokio::try_join!(
        catalog::preview_feed(state.db.pool(), feed_id),
        catalog::preview_articles(state.db.pool(), feed_id),
        catalog::content_probe_stats(state.db.pool(), feed_id),
        catalog::probe_needs_refresh(state.db.pool(), feed_id),
    )?;
    let feed = feed.ok_or(AppError::NotFound)?;
    let articles: Vec<_> = articles.into_iter().map(preview_article).collect();
    let needs_analysis = needs_analysis && !articles.is_empty();
    let ui = prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = CatalogPreviewTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui,
        is_admin: user.role == "admin",
        preview: CatalogPreviewView {
            feed_id: feed_id.to_string(),
            title: feed.title,
            description: feed.description,
            category: feed.category,
            country: feed.country,
            language: feed.language,
            show_processed,
            rss_url: preview_url(feed_id, false),
            processed_url: preview_url(feed_id, true),
            stats: stats.map(preview_stats),
            articles,
            needs_analysis,
            analysis_message: analysis_message(query.analysis.as_deref()),
        },
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    let mut response = (jar, body).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}

pub async fn preview(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(feed_id): Path<Uuid>,
    Query(query): Query<PreviewQuery>,
) -> Result<Response, AppError> {
    render_preview_page(&state, &user, jar, feed_id, query).await
}

pub async fn analyze_preview(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
    jar: CookieJar,
    Path(feed_id): Path<Uuid>,
    Query(query): Query<PreviewQuery>,
    Form(form): Form<PreviewForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !catalog::is_published_feed(state.db.pool(), feed_id).await? {
        return Err(AppError::NotFound);
    }
    let view = if query.view.as_deref() == Some("processed") {
        "processed"
    } else {
        "rss"
    };
    let analysis = if catalog::preview_articles(state.db.pool(), feed_id)
        .await?
        .is_empty()
    {
        "no_articles"
    } else if catalog::probe_needs_refresh(state.db.pool(), feed_id).await? {
        let dedup = format!("catalog_content_probe:{feed_id}");
        let _ = jobs::enqueue(
            state.db.pool(),
            "catalog_content_probe",
            serde_json::json!({ "feed_id": feed_id }),
            Some(&dedup),
            None,
            1,
        )
        .await?;
        "queued"
    } else {
        "current"
    };
    Ok((
        jar,
        Redirect::to(&format!(
            "/catalog/{feed_id}/preview?view={view}&analysis={analysis}"
        )),
    )
        .into_response())
}

pub async fn subscribe(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(values): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let (csrf_token, feed_ids) = match parse_subscription_form(values) {
        Ok(form) => form,
        Err(error) => {
            return render_page(
                &state,
                &user,
                jar,
                CatalogQuery {
                    category: None,
                    country: None,
                    language: None,
                },
                None,
                Some(error),
            )
            .await;
        }
    };
    if !csrf::verify(&jar, &csrf_token) {
        return Err(AppError::Forbidden);
    }
    let result = match catalog::subscribe_published_many(state.db.pool(), user.id, &feed_ids).await
    {
        Ok(result) => result,
        Err(rustss_db::DbError::NotFound) => {
            return render_page(
                &state,
                &user,
                jar,
                CatalogQuery {
                    category: None,
                    country: None,
                    language: None,
                },
                None,
                Some("One or more selected sources are no longer available.".to_string()),
            )
            .await;
        }
        Err(error) => return Err(error.into()),
    };
    let existing = result.attempted - result.created;
    let mut queued = 0;
    for feed_id in &feed_ids {
        if enqueue_fetch_feed(&state.db, *feed_id).await?.is_some() {
            queued += 1;
        }
    }
    let message = if existing == 0 {
        format!(
            "Subscribed to {} source(s); queued {queued} shared refresh(es).",
            result.created
        )
    } else {
        format!(
            "Subscribed to {} new source(s); {} were already in your library. Queued {queued} shared refresh(es).",
            result.created, existing
        )
    };
    render_page(
        &state,
        &user,
        jar,
        CatalogQuery {
            category: None,
            country: None,
            language: None,
        },
        Some(message),
        None,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_deduplicated_catalog_choices() {
        let id = Uuid::now_v7();
        let (csrf, ids) = parse_subscription_form(vec![
            ("csrf_token".to_string(), "token".to_string()),
            ("feed_id".to_string(), id.to_string()),
            ("feed_id".to_string(), id.to_string()),
        ])
        .unwrap();
        assert_eq!(csrf, "token");
        assert_eq!(ids, vec![id]);
    }
}
