//! Feed management: subscribe, inspect shared source policy, queue refreshes,
//! and unsubscribe. Content acquisition stays shared per feed, regardless of
//! subscriber count.

use std::collections::{HashMap, HashSet};

use askama::Template;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use rustss_core::feed_content::{FeedContentMode, FeedContentSettings};
use rustss_db::{articles, catalog, categories, feed_icons, feeds, subscriptions, Feed};
use rustss_feeds::{enqueue_fetch_feed, normalize_url};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

struct FeedRow {
    id: String,
    name: String,
    initial: String,
    has_icon: bool,
    url: String,
    health: String,
    content_mode: String,
    content_mode_label: String,
    refresh_cadence: String,
    tags: Vec<String>,
    tags_input: String,
    tag_choices: Vec<TagChoice>,
    folder_name: String,
    folder_id: String,
    category_choices: Vec<CategoryChoice>,
    can_configure: bool,
}

#[derive(Clone)]
struct CategoryOption {
    id: String,
    raw_id: Uuid,
    name: String,
    tree_name: String,
    depth: usize,
}

#[derive(Clone)]
struct CategoryChoice {
    id: String,
    name: String,
    selected: bool,
}

#[derive(Clone)]
struct TagOption {
    name: String,
}

#[derive(Clone)]
struct TagChoice {
    name: String,
    selected: bool,
}

struct FeedSettingsView {
    id: String,
    name: String,
    url: String,
    content_mode: String,
    selector: String,
    respect_robots_txt: bool,
    extraction_delay_ms: Option<i32>,
    fetch_interval_minutes: i32,
}

#[derive(Template)]
#[template(path = "feeds.html")]
struct FeedsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: crate::prefs::Preferences,
    is_admin: bool,
    feeds: Vec<FeedRow>,
    categories: Vec<CategoryOption>,
    tag_options: Vec<TagOption>,
    message: Option<String>,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "feed_settings.html")]
struct FeedSettingsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: crate::prefs::Preferences,
    is_admin: bool,
    feed: FeedSettingsView,
    message: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
pub struct AddForm {
    csrf_token: String,
    url: String,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct CategoryForm {
    csrf_token: String,
    name: String,
    #[serde(default)]
    parent_id: String,
}

#[derive(Deserialize)]
pub struct ContentSettingsForm {
    csrf_token: String,
    content_mode: String,
    #[serde(default)]
    selector: String,
    #[serde(default)]
    respect_robots_txt: Option<String>,
    #[serde(default)]
    extraction_delay_ms: String,
    fetch_interval_minutes: String,
}

fn content_settings(feed: &Feed) -> FeedContentSettings {
    FeedContentSettings::from_json(feed.extraction_settings.as_ref())
}

fn feed_initial(name: &str) -> String {
    name.chars()
        .find(|character| character.is_alphanumeric())
        .map(|character| character.to_uppercase().collect())
        .unwrap_or_else(|| "•".to_string())
}

fn refresh_cadence(feed: &Feed) -> String {
    if feed.consecutive_failures < 3 {
        return format!("Refresh every {} minutes", feed.fetch_interval_minutes);
    }
    if feed
        .failure_started_at
        .is_some_and(|started_at| started_at <= OffsetDateTime::now_utc() - Duration::days(14))
    {
        "Retry weekly while the feed is failing".to_string()
    } else {
        "Retry daily while the feed is failing".to_string()
    }
}

fn category_options(categories: &[categories::UserCategory]) -> Vec<CategoryOption> {
    let by_id: HashMap<Uuid, &categories::UserCategory> = categories
        .iter()
        .map(|category| (category.id, category))
        .collect();
    let mut options: Vec<_> = categories
        .iter()
        .map(|category| {
            let mut names = Vec::new();
            let mut current = Some(category.id);
            let mut visited = HashSet::new();
            while let Some(id) = current {
                let Some(category) = by_id.get(&id) else {
                    break;
                };
                if !visited.insert(id) {
                    break;
                }
                names.push(category.name.clone());
                current = category.parent_id;
            }
            names.reverse();
            CategoryOption {
                id: category.id.to_string(),
                raw_id: category.id,
                name: names.join(" / "),
                tree_name: category.name.clone(),
                depth: names.len().saturating_sub(1),
            }
        })
        .collect();
    options.sort_by(|left, right| left.name.cmp(&right.name));
    options
}

fn normalize_tags(value: &str) -> Result<Vec<String>, String> {
    let mut tags = Vec::new();
    let mut seen = HashSet::new();
    for tag in value.split([',', '\n']) {
        let tag = tag.trim().to_lowercase();
        if tag.is_empty() {
            continue;
        }
        if tag.chars().count() > 40 || tag.chars().any(char::is_control) {
            return Err("Each tag must be at most 40 printable characters.".to_string());
        }
        if seen.insert(tag.clone()) {
            tags.push(tag);
        }
    }
    if tags.len() > 20 {
        return Err("Use at most 20 tags per feed.".to_string());
    }
    Ok(tags)
}

fn parse_organization_form(
    values: Vec<(String, String)>,
) -> Result<(String, Vec<String>, Option<Uuid>), String> {
    let mut csrf_token = None;
    let mut tags = None;
    let mut tag_options = Vec::new();
    let mut folder_id = None;
    for (name, value) in values {
        match name.as_str() {
            "csrf_token" if csrf_token.is_none() => csrf_token = Some(value),
            "tags" if tags.is_none() => tags = Some(value),
            "tag_option" => tag_options.push(value),
            "folder_id" if folder_id.is_none() => {
                folder_id = Some(match value.trim() {
                    "" => None,
                    value => Some(
                        Uuid::parse_str(value)
                            .map_err(|_| "The selected folder was invalid.".to_string())?,
                    ),
                });
            }
            _ => return Err("The feed organization could not be processed.".to_string()),
        }
    }
    let csrf_token =
        csrf_token.ok_or_else(|| "The feed organization could not be processed.".to_string())?;
    let tags = std::iter::once(tags.unwrap_or_default())
        .chain(tag_options)
        .collect::<Vec<_>>()
        .join(",");
    Ok((csrf_token, normalize_tags(&tags)?, folder_id.flatten()))
}

fn feed_row(
    feed: Feed,
    can_configure: bool,
    organization: Option<&subscriptions::FeedOrganization>,
    categories: &[CategoryOption],
    tag_options: &[TagOption],
    has_icon: bool,
) -> FeedRow {
    let settings = content_settings(&feed);
    let category_id = organization.and_then(|organization| organization.category_id);
    let category_choices: Vec<_> = categories
        .iter()
        .map(|category| CategoryChoice {
            id: category.id.clone(),
            name: category.name.clone(),
            selected: Some(category.raw_id) == category_id,
        })
        .collect();
    let folder = category_choices.iter().find(|category| category.selected);
    let folder_name = folder
        .map(|category| category.name.clone())
        .unwrap_or_else(|| "Unfiled".to_string());
    let folder_id = folder
        .map(|category| category.id.clone())
        .unwrap_or_default();
    let tags = organization
        .map(|organization| organization.tags.clone())
        .unwrap_or_default();
    let tags_input = tags
        .iter()
        .filter(|tag| {
            !tag_options
                .iter()
                .any(|option| option.name.as_str() == tag.as_str())
        })
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let tag_choices = tag_options
        .iter()
        .map(|option| TagChoice {
            name: option.name.clone(),
            selected: tags.iter().any(|tag| tag == &option.name),
        })
        .collect();
    let refresh_cadence = refresh_cadence(&feed);
    FeedRow {
        id: feed.id.to_string(),
        initial: feed_initial(&feed.name),
        name: feed.name,
        url: feed.url,
        health: feed.health_status,
        content_mode: settings.mode.as_str().to_string(),
        content_mode_label: settings.mode.label().to_string(),
        refresh_cadence,
        tags,
        tags_input,
        tag_choices,
        folder_name,
        folder_id,
        category_choices,
        has_icon,
        can_configure,
    }
}

fn feed_settings_view(feed: &Feed) -> FeedSettingsView {
    let settings = content_settings(feed);
    FeedSettingsView {
        id: feed.id.to_string(),
        name: feed.name.clone(),
        url: feed.url.clone(),
        content_mode: settings.mode.as_str().to_string(),
        selector: settings.selector.unwrap_or_default(),
        respect_robots_txt: feed.respect_robots_txt,
        extraction_delay_ms: feed.extraction_delay_ms,
        fetch_interval_minutes: feed.fetch_interval_minutes,
    }
}

async fn render_page(
    state: &AppState,
    user_id: Uuid,
    user_email: String,
    is_admin: bool,
    jar: CookieJar,
    message: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = crate::prefs::load(&state.db, user_id).await;
    let (feed_records, category_records, tag_records, organizations) = tokio::try_join!(
        feeds::list_for_user(state.db.pool(), user_id),
        categories::list_for_user(state.db.pool(), user_id),
        categories::list_tags(state.db.pool(), user_id),
        subscriptions::list_organization(state.db.pool(), user_id),
    )?;
    let categories = category_options(&category_records);
    let tag_options: Vec<_> = tag_records
        .into_iter()
        .map(|tag| TagOption { name: tag.name })
        .collect();
    let organizations: HashMap<Uuid, subscriptions::FeedOrganization> = organizations
        .into_iter()
        .map(|organization| (organization.feed_id, organization))
        .collect();
    let feed_ids: Vec<Uuid> = feed_records.iter().map(|feed| feed.id).collect();
    let icon_ids = feed_icons::available_for_feeds(state.db.pool(), &feed_ids).await?;
    let feeds = feed_records
        .into_iter()
        .map(|feed| {
            let feed_id = feed.id;
            let has_icon = icon_ids.contains(&feed_id);
            feed_row(
                feed,
                is_admin,
                organizations.get(&feed_id),
                &categories,
                &tag_options,
                has_icon,
            )
        })
        .collect();
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = FeedsTemplate {
        theme: ui.theme.clone(),
        user_email,
        csrf_token,
        ui,
        is_admin,
        feeds,
        categories,
        tag_options,
        message,
        error,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, body).into_response())
}

async fn render_settings_page(
    state: &AppState,
    user_id: Uuid,
    user_email: String,
    jar: CookieJar,
    feed: &Feed,
    message: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = crate::prefs::load(&state.db, user_id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = FeedSettingsTemplate {
        theme: ui.theme.clone(),
        user_email,
        csrf_token,
        ui,
        is_admin: true,
        feed: feed_settings_view(feed),
        message,
        error,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, body).into_response())
}

fn parse_interval(value: &str) -> Result<i32, String> {
    value
        .trim()
        .parse::<i32>()
        .map(|minutes| minutes.clamp(5, 1440))
        .map_err(|_| "Refresh interval must be a whole number of minutes.".to_string())
}

fn parse_delay(value: &str) -> Result<Option<i32>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<i32>()
        .map(|delay| Some(delay.clamp(250, 120_000)))
        .map_err(|_| "Extraction delay must be a whole number of milliseconds.".to_string())
}

fn parse_robots_policy(value: Option<&str>, current: bool) -> Result<bool, String> {
    match value {
        None => Ok(current),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(_) => Err("Choose whether this shared feed should respect robots.txt.".to_string()),
    }
}

/// `GET /feeds`.
pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let is_admin = user.role == "admin";
    render_page(&state, user.id, user.email, is_admin, jar, None, None).await
}

/// `POST /feeds` — subscribe to a feed and queue the shared refresh path.
pub async fn add(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<AddForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let is_admin = user.role == "admin";
    let normalized = match normalize_url(&form.url) {
        Ok(url) => url,
        Err(error) => {
            return render_page(
                &state,
                user.id,
                user.email,
                is_admin,
                jar,
                None,
                Some(format!("Invalid feed URL: {error}")),
            )
            .await;
        }
    };
    if let Err(error) = state.fetch.validate_url(&normalized) {
        return render_page(
            &state,
            user.id,
            user.email,
            is_admin,
            jar,
            None,
            Some(format!("That URL cannot be used: {error}")),
        )
        .await;
    }

    let host = url::Url::parse(&normalized)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "Feed".to_string());
    let (feed, created) = feeds::upsert_by_url(state.db.pool(), &normalized, &host, None).await?;
    subscriptions::subscribe(state.db.pool(), user.id, feed.id, None).await?;
    let queued = enqueue_fetch_feed(&state.db, feed.id).await?.is_some();
    let refresh_message = if queued {
        "Refresh queued."
    } else {
        "A refresh is already queued."
    };
    let action = if created {
        "Subscribed to"
    } else {
        "Subscribed to existing"
    };
    render_page(
        &state,
        user.id,
        user.email,
        is_admin,
        jar,
        Some(format!("{action} “{}”. {refresh_message}", feed.name)),
        None,
    )
    .await
}

pub async fn create_category(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<CategoryForm>,
) -> Result<Response, AppError> {
    let is_admin = user.role == "admin";
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let name = form.name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return render_page(
            &state,
            user.id,
            user.email,
            is_admin,
            jar,
            None,
            Some("Folder names must be between 1 and 80 characters.".to_string()),
        )
        .await;
    }
    let parent_id = match form.parent_id.trim() {
        "" => None,
        value => match Uuid::parse_str(value) {
            Ok(id) => Some(id),
            Err(_) => {
                return render_page(
                    &state,
                    user.id,
                    user.email,
                    is_admin,
                    jar,
                    None,
                    Some("Choose a valid parent folder.".to_string()),
                )
                .await;
            }
        },
    };
    match categories::create(state.db.pool(), user.id, name, parent_id).await {
        Ok(_) => {
            render_page(
                &state,
                user.id,
                user.email,
                is_admin,
                jar,
                Some(format!("Folder “{name}” is ready.")),
                None,
            )
            .await
        }
        Err(rustss_db::DbError::NotFound) => {
            render_page(
                &state,
                user.id,
                user.email,
                is_admin,
                jar,
                None,
                Some("Choose a parent folder in your own library.".to_string()),
            )
            .await
        }
        Err(error) => Err(error.into()),
    }
}

pub async fn update_organization(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(values): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let is_admin = user.role == "admin";
    let (csrf_token, tags, folder_id) = match parse_organization_form(values) {
        Ok(form) => form,
        Err(error) => {
            return render_page(
                &state,
                user.id,
                user.email,
                is_admin,
                jar,
                None,
                Some(error),
            )
            .await;
        }
    };
    if !csrf::verify(&jar, &csrf_token) {
        return Err(AppError::Forbidden);
    }
    match subscriptions::update_organization(state.db.pool(), user.id, id, &tags, folder_id).await {
        Ok(()) => {
            render_page(
                &state,
                user.id,
                user.email,
                is_admin,
                jar,
                Some("Feed organization saved.".to_string()),
                None,
            )
            .await
        }
        Err(rustss_db::DbError::NotFound) => Err(AppError::NotFound),
        Err(error) => Err(error.into()),
    }
}

pub async fn icon(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    if user.role != "admin"
        && !subscriptions::is_subscribed(state.db.pool(), user.id, id).await?
        && !catalog::is_published_feed(state.db.pool(), id).await?
    {
        return Err(AppError::NotFound);
    }
    let icon = feed_icons::get(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let content_type = match icon.content_type.as_str() {
        "image/png" => "image/png",
        "image/jpeg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        "image/x-icon" => "image/x-icon",
        _ => return Err(AppError::NotFound),
    };
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "private, max-age=604800"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        icon.content,
    )
        .into_response())
}

/// `POST /feeds/{id}/refresh`.
pub async fn refresh(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !subscriptions::is_subscribed(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    let is_admin = user.role == "admin";
    let queued = enqueue_fetch_feed(&state.db, id).await?.is_some();
    let message = if queued {
        "Refresh queued.".to_string()
    } else {
        "A refresh is already queued.".to_string()
    };
    render_page(
        &state,
        user.id,
        user.email,
        is_admin,
        jar,
        Some(message),
        None,
    )
    .await
}

/// `POST /feeds/refresh-all`.
pub async fn refresh_all(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let feeds = feeds::list_for_user(state.db.pool(), user.id).await?;
    let mut queued = 0u64;
    for feed in &feeds {
        if enqueue_fetch_feed(&state.db, feed.id).await?.is_some() {
            queued += 1;
        }
    }
    let is_admin = user.role == "admin";
    render_page(
        &state,
        user.id,
        user.email,
        is_admin,
        jar,
        Some(format!(
            "Queued {queued} of {} feed refreshes.",
            feeds.len()
        )),
        None,
    )
    .await
}

/// `GET /feeds/{id}/settings`.
pub async fn settings_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let is_admin = user.role == "admin";
    if !is_admin && !subscriptions::is_subscribed(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    let feed = feeds::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !is_admin {
        return render_page(
            &state,
            user.id,
            user.email,
            false,
            jar,
            None,
            Some("Only an administrator can change this shared feed policy.".to_string()),
        )
        .await;
    }
    render_settings_page(&state, user.id, user.email, jar, &feed, None, None).await
}

/// `POST /feeds/{id}/settings`.
pub async fn update_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ContentSettingsForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if user.role != "admin" {
        return Err(AppError::Forbidden);
    }
    let feed = feeds::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let mode = match FeedContentMode::parse(form.content_mode.trim()) {
        Some(mode) => mode,
        None => {
            return render_settings_page(
                &state,
                user.id,
                user.email,
                jar,
                &feed,
                None,
                Some("Choose a valid content source.".to_string()),
            )
            .await;
        }
    };
    let fetch_interval_minutes = match parse_interval(&form.fetch_interval_minutes) {
        Ok(value) => value,
        Err(error) => {
            return render_settings_page(
                &state,
                user.id,
                user.email,
                jar,
                &feed,
                None,
                Some(error),
            )
            .await;
        }
    };
    let extraction_delay_ms = match parse_delay(&form.extraction_delay_ms) {
        Ok(value) => value,
        Err(error) => {
            return render_settings_page(
                &state,
                user.id,
                user.email,
                jar,
                &feed,
                None,
                Some(error),
            )
            .await;
        }
    };
    let respect_robots_txt =
        match parse_robots_policy(form.respect_robots_txt.as_deref(), feed.respect_robots_txt) {
            Ok(value) => value,
            Err(error) => {
                return render_settings_page(
                    &state,
                    user.id,
                    user.email,
                    jar,
                    &feed,
                    None,
                    Some(error),
                )
                .await;
            }
        };
    let selector = (!form.selector.trim().is_empty()).then(|| form.selector.trim().to_string());
    let settings = FeedContentSettings {
        mode,
        selector: if mode.fetches_article_pages() {
            selector
        } else {
            None
        },
    };
    feeds::update_content_settings(
        state.db.pool(),
        id,
        &settings.to_json(),
        respect_robots_txt,
        if mode.fetches_article_pages() {
            extraction_delay_ms
        } else {
            None
        },
        fetch_interval_minutes,
    )
    .await?;

    let reset = if mode.fetches_article_pages() {
        articles::reset_extraction_for_feed(state.db.pool(), id).await?
    } else {
        articles::restore_feed_content(state.db.pool(), id).await?
    };
    let queued = enqueue_fetch_feed(&state.db, id).await?.is_some();
    let updated = feeds::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let policy_message = if mode.fetches_article_pages() {
        format!("Shared source policy saved. Queued public-page processing; reset {reset} failed fallback(s).")
    } else {
        format!("Shared source policy saved. Restored RSS fallback for {reset} article(s).")
    };
    let message = if queued {
        policy_message
    } else {
        format!("{policy_message} A refresh is already queued.")
    };
    render_settings_page(
        &state,
        user.id,
        user.email,
        jar,
        &updated,
        Some(message),
        None,
    )
    .await
}

/// `POST /feeds/{id}/unsubscribe`.
pub async fn unsubscribe(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    subscriptions::unsubscribe(state.db.pool(), user.id, id).await?;
    Ok((jar, Redirect::to("/feeds")).into_response())
}
