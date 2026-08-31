//! Feed management: subscribe, inspect shared source policy, queue refreshes,
//! and unsubscribe. Content acquisition stays shared per feed, regardless of
//! subscriber count.

use std::collections::{BTreeSet, HashMap, HashSet};

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use rustss_core::content_rules::{
    apply_rewrite_rules, entry_filters_from_json, entry_is_visible, validate_entry_filters,
    validate_rewrite_rules, EntryFilter, RewriteRule,
};
use rustss_core::feed_content::{
    ContentLifetime, FeedContentMode, FeedContentSettings, SubscriptionContentSettings,
    SubscriptionEnclosureDelivery, SubscriptionEnclosureLoad, SubscriptionEnclosureSettings,
};
use rustss_core::youtube::SubscriptionYouTubeMode;
use rustss_db::{
    articles, catalog, categories, feed_icons, feeds, jobs, rewrite_rules, subscriptions, DbError,
    Feed,
};
use rustss_feeds::{discover_feed_urls, enqueue_fetch_feed, normalize_url, DiscoveredFeed};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::extract::Ajax;
use crate::state::AppState;

const MAX_BULK_FEED_SELECTION: usize = 100;
const MAX_CONTENT_BACKFILL: usize = 5_000;

#[derive(Default)]
struct RefreshQueueOutcome {
    queued: usize,
    already_queued: usize,
    failed: usize,
}

struct FeedRow {
    id: String,
    name: String,
    initial: String,
    has_icon: bool,
    url: String,
    health: String,
    content_mode: String,
    content_mode_label: String,
    content_lifetime_label: String,
    refresh_cadence: String,
    last_updated: String,
    last_updated_at: Option<String>,
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
    content_lifetime: String,
    youtube_player_mode: String,
    enclosure_delivery_mode: String,
    enclosure_load_mode: String,
    is_subscribed: bool,
    selector: String,
    respect_robots_txt: bool,
    extraction_delay_ms: Option<i32>,
    fetch_interval_minutes: i32,
    entry_filters_json: String,
    rewrite_rules_json: String,
}

struct RewriteVersionView {
    id: String,
    version: i32,
    created_at: String,
    rule_count: usize,
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
    bookmarklet_href: String,
}

#[derive(Template)]
#[template(path = "bookmarklet_subscribe.html")]
struct BookmarkletTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: crate::prefs::Preferences,
    is_admin: bool,
    source_url: String,
    candidates: Vec<DiscoveredFeed>,
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
    youtube_embeds_enabled: bool,
    feed: FeedSettingsView,
    rewrite_versions: Vec<RewriteVersionView>,
    message: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
pub struct AddForm {
    csrf_token: String,
    url: String,
}

#[derive(Deserialize)]
pub struct BookmarkletQuery {
    url: String,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct MoveFolderForm {
    csrf_token: String,
    #[serde(default)]
    folder_id: String,
}

#[derive(Deserialize)]
pub struct CollapseFolderForm {
    csrf_token: String,
    collapsed: bool,
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
    #[serde(default)]
    content_mode: Option<String>,
    #[serde(default)]
    content_lifetime: Option<String>,
    #[serde(default)]
    selector: String,
    #[serde(default)]
    respect_robots_txt: Option<String>,
    #[serde(default)]
    extraction_delay_ms: String,
    #[serde(default)]
    fetch_interval_minutes: String,
    #[serde(default)]
    youtube_player_mode: Option<String>,
    #[serde(default)]
    enclosure_delivery_mode: Option<String>,
    #[serde(default)]
    enclosure_load_mode: Option<String>,
    #[serde(default)]
    entry_filters_json: String,
    #[serde(default)]
    rewrite_rules_json: String,
}

#[derive(Deserialize)]
pub struct PersonalContentSettingsForm {
    csrf_token: String,
    content_mode: String,
    content_lifetime: String,
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

fn last_updated_label(updated_at: Option<OffsetDateTime>, now: OffsetDateTime) -> String {
    let Some(updated_at) = updated_at else {
        return "Not updated yet".to_string();
    };
    let elapsed = (now - updated_at).whole_seconds().max(0);
    if elapsed < 60 {
        "just now".to_string()
    } else if elapsed < Duration::HOUR.whole_seconds() {
        format!("{} min ago", elapsed / 60)
    } else if elapsed < Duration::DAY.whole_seconds() {
        format!("{} hr ago", elapsed / Duration::HOUR.whole_seconds())
    } else if elapsed < Duration::WEEK.whole_seconds() {
        format!("{} days ago", elapsed / Duration::DAY.whole_seconds())
    } else {
        updated_at.date().to_string()
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

fn parse_folder_id(value: &str) -> Result<Option<Uuid>, String> {
    match value.trim() {
        "" => Ok(None),
        value => Uuid::parse_str(value)
            .map(Some)
            .map_err(|_| "The selected folder was invalid.".to_string()),
    }
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
            "folder_id" if folder_id.is_none() => folder_id = Some(parse_folder_id(&value)?),
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

struct BulkForm {
    csrf_token: String,
    feed_ids: Vec<Uuid>,
    folder_id: Option<Uuid>,
    tags: Vec<String>,
    tag_operation: subscriptions::BulkTagOperation,
    content_mode: Option<String>,
    content_lifetime: Option<String>,
}

fn parse_bulk_form(values: Vec<(String, String)>) -> Result<BulkForm, String> {
    let mut csrf_token = None;
    let mut feed_ids = BTreeSet::new();
    let mut folder_id = None;
    let mut tags = None;
    let mut tag_operation = None;
    let mut content_mode = None;
    let mut content_lifetime = None;
    for (name, value) in values {
        match name.as_str() {
            "csrf_token" if csrf_token.is_none() => csrf_token = Some(value),
            "feed_id" => {
                let feed_id = Uuid::parse_str(&value)
                    .map_err(|_| "A selected feed was invalid.".to_string())?;
                feed_ids.insert(feed_id);
            }
            "folder_id" if folder_id.is_none() => folder_id = Some(parse_folder_id(&value)?),
            "tags" if tags.is_none() => tags = Some(value),
            "tag_operation" if tag_operation.is_none() => tag_operation = Some(value),
            "content_mode" if content_mode.is_none() => content_mode = Some(value),
            "content_lifetime" if content_lifetime.is_none() => content_lifetime = Some(value),
            _ => return Err("The bulk feed action could not be processed.".to_string()),
        }
    }
    let csrf_token =
        csrf_token.ok_or_else(|| "The bulk feed action could not be processed.".to_string())?;
    if feed_ids.is_empty() {
        return Err("Choose at least one feed first.".to_string());
    }
    if feed_ids.len() > MAX_BULK_FEED_SELECTION {
        return Err(format!(
            "Choose at most {MAX_BULK_FEED_SELECTION} feeds at once."
        ));
    }
    let tag_operation = match tag_operation.as_deref().unwrap_or("add") {
        "add" => subscriptions::BulkTagOperation::Add,
        "remove" => subscriptions::BulkTagOperation::Remove,
        "replace" => subscriptions::BulkTagOperation::Replace,
        _ => return Err("Choose a valid tag operation.".to_string()),
    };
    Ok(BulkForm {
        csrf_token,
        feed_ids: feed_ids.into_iter().collect(),
        folder_id: folder_id.flatten(),
        tags: normalize_tags(&tags.unwrap_or_default())?,
        tag_operation,
        content_mode,
        content_lifetime,
    })
}

async fn queue_refreshes(state: &AppState, feed_ids: &[Uuid]) -> RefreshQueueOutcome {
    let mut outcome = RefreshQueueOutcome::default();
    for feed_id in feed_ids {
        match enqueue_fetch_feed(&state.db, *feed_id).await {
            Ok(Some(_)) => outcome.queued += 1,
            Ok(None) => outcome.already_queued += 1,
            Err(error) => {
                outcome.failed += 1;
                tracing::warn!(feed_id = %feed_id, error = %error, "could not queue feed refresh");
            }
        }
    }
    outcome
}

async fn queue_page_extractions(state: &AppState, feed_id: Uuid) -> Result<usize, AppError> {
    let queued =
        jobs::enqueue_extractions_for_feed(state.db.pool(), feed_id, MAX_CONTENT_BACKFILL).await?;
    Ok(queued as usize)
}

fn feed_row(
    mut feed: Feed,
    can_configure: bool,
    organization: Option<&subscriptions::FeedOrganization>,
    categories: &[CategoryOption],
    tag_options: &[TagOption],
    has_icon: bool,
) -> FeedRow {
    if let Some(custom_name) = organization
        .and_then(|organization| organization.custom_name.as_deref())
        .filter(|name| !name.trim().is_empty())
    {
        feed.name = custom_name.to_string();
    }
    let settings = organization
        .map(|organization| SubscriptionContentSettings::from_json(organization.settings.as_ref()))
        .unwrap_or_default();
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
    let last_updated = last_updated_label(feed.last_successful_fetch_at, OffsetDateTime::now_utc());
    let last_updated_at = feed
        .last_successful_fetch_at
        .map(|updated_at| updated_at.to_string());
    FeedRow {
        id: feed.id.to_string(),
        initial: feed_initial(&feed.name),
        name: feed.name,
        url: feed.url,
        health: feed.health_status,
        content_mode: settings.mode.as_str().to_string(),
        content_mode_label: settings.mode.label().to_string(),
        content_lifetime_label: match settings.lifetime {
            ContentLifetime::Days(days) => format!("{days} day lifetime"),
            ContentLifetime::Infinite => "Infinite lifetime".to_string(),
        },
        refresh_cadence,
        last_updated,
        last_updated_at,
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

fn feed_settings_view(
    feed: &Feed,
    subscription_settings: Option<&serde_json::Value>,
    personal: Option<SubscriptionContentSettings>,
    youtube_mode: Option<SubscriptionYouTubeMode>,
    enclosure_settings: Option<SubscriptionEnclosureSettings>,
) -> FeedSettingsView {
    let shared = content_settings(feed);
    let is_subscribed = personal.is_some();
    let personal = personal.unwrap_or_default();
    let enclosure_settings = enclosure_settings.unwrap_or_default();
    FeedSettingsView {
        id: feed.id.to_string(),
        name: feed.name.clone(),
        url: feed.url.clone(),
        content_mode: personal.mode.as_str().to_string(),
        content_lifetime: personal.lifetime.form_value(),
        youtube_player_mode: youtube_mode.unwrap_or_default().as_str().to_string(),
        enclosure_delivery_mode: enclosure_settings.delivery.as_str().to_string(),
        enclosure_load_mode: enclosure_settings.load.as_str().to_string(),
        is_subscribed,
        selector: shared.selector.unwrap_or_default(),
        respect_robots_txt: feed.respect_robots_txt,
        extraction_delay_ms: feed.extraction_delay_ms,
        fetch_interval_minutes: feed.fetch_interval_minutes,
        entry_filters_json: serde_json::to_string_pretty(&entry_filters_from_json(
            subscription_settings,
        ))
        .unwrap_or_else(|_| "[]".to_string()),
        rewrite_rules_json: serde_json::to_string_pretty(&shared.rewrite_rules)
            .unwrap_or_else(|_| "[]".to_string()),
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
                true,
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
        bookmarklet_href: bookmarklet_href(&state.settings.base_url),
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, body).into_response())
}

fn bookmarklet_href(base_url: &str) -> String {
    let target = format!(
        "{}/bookmarklet/subscribe?url=",
        base_url.trim_end_matches('/')
    );
    let target = serde_json::to_string(&target)
        .unwrap_or_else(|_| "\"/bookmarklet/subscribe?url=\"".to_string());
    format!(
        "javascript:(()=>{{window.open({target}+encodeURIComponent(location.href),'_blank','noopener')}})()"
    )
}

async fn render_settings_page(
    state: &AppState,
    user_id: Uuid,
    user_email: String,
    jar: CookieJar,
    feed: &Feed,
    is_admin: bool,
    message: Option<String>,
) -> Result<Response, AppError> {
    let ui = crate::prefs::load(&state.db, user_id).await;
    let subscription = subscriptions::find(state.db.pool(), user_id, feed.id).await?;
    let personal = subscription
        .as_ref()
        .map(|subscription| SubscriptionContentSettings::from_json(subscription.settings.as_ref()));
    let youtube_mode = subscription
        .as_ref()
        .map(|subscription| SubscriptionYouTubeMode::from_json(subscription.settings.as_ref()));
    let enclosure_settings = subscription.as_ref().map(|subscription| {
        SubscriptionEnclosureSettings::from_json(subscription.settings.as_ref())
    });
    let rewrite_versions = if is_admin {
        rewrite_rules::list(state.db.pool(), feed.id, 10)
            .await?
            .into_iter()
            .map(|version| RewriteVersionView {
                id: version.id.to_string(),
                version: version.version,
                created_at: version.created_at.to_string(),
                rule_count: version.rules.as_array().map_or(0, Vec::len),
            })
            .collect()
    } else {
        Vec::new()
    };
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = FeedSettingsTemplate {
        theme: ui.theme.clone(),
        user_email,
        csrf_token,
        ui,
        is_admin,
        youtube_embeds_enabled: state.settings.youtube_policy.enabled(),
        feed: feed_settings_view(
            feed,
            subscription
                .as_ref()
                .and_then(|subscription| subscription.settings.as_ref()),
            personal,
            youtube_mode,
            enclosure_settings,
        ),
        rewrite_versions,
        message,
        error: None,
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

fn parse_entry_filters(value: &str) -> Result<Vec<EntryFilter>, AppError> {
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(value)
        .map_err(|error| AppError::Validation(format!("Entry filters must be valid JSON: {error}")))
}

fn parse_rewrite_rules(value: &str) -> Result<Vec<RewriteRule>, AppError> {
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(value)
        .map_err(|error| AppError::Validation(format!("Rewrite rules must be valid JSON: {error}")))
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

/// `GET /bookmarklet/subscribe` — discover advertised feeds and ask before subscribing.
pub async fn bookmarklet_subscribe(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Query(query): Query<BookmarkletQuery>,
) -> Result<Response, AppError> {
    if !state.feed_add_limiter.check(&user.id.to_string()).await {
        return Err(AppError::RateLimited);
    }
    let ui = crate::prefs::load(&state.db, user.id).await;
    let source_url = query.url.trim().to_string();
    let mut error = None;
    let candidates = if source_url.len() > 8_192 {
        error = Some("That page URL is too long to inspect safely.".to_string());
        Vec::new()
    } else {
        match normalize_url(&source_url) {
            Ok(url) => match state
                .fetch
                .get_bytes_with_metadata(&url, 2 * 1_024 * 1_024)
                .await
            {
                Ok(response) => discover_feed_urls(
                    &response.final_url,
                    response.content_type.as_deref(),
                    &response.body,
                )
                .into_iter()
                .filter(|candidate| state.fetch.validate_url(&candidate.url).is_ok())
                .collect(),
                Err(fetch_error) => {
                    error = Some(format!("RustSS could not inspect that page: {fetch_error}"));
                    Vec::new()
                }
            },
            Err(url_error) => {
                error = Some(format!("That page URL is invalid: {url_error}"));
                Vec::new()
            }
        }
    };
    if candidates.is_empty() && error.is_none() {
        error = Some(
            "That page does not advertise an RSS, Atom, or JSON Feed. You can still paste a known feed URL on Manage feeds."
                .to_string(),
        );
    }
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = BookmarkletTemplate {
        theme: ui.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui,
        is_admin: user.role == "admin",
        source_url,
        candidates,
        error,
    }
    .render()
    .map(Html)
    .map_err(|render_error| {
        AppError::Internal(anyhow::anyhow!("template render failed: {render_error}"))
    })?;
    Ok((jar, body).into_response())
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
    if !state.feed_add_limiter.check(&user.id.to_string()).await {
        return Err(AppError::RateLimited);
    }
    crate::verify::require_verified(&state, &user)?;
    crate::quota::assert_within(
        &state,
        user.id,
        rustss_core::quotas::QuotaKind::Subscriptions,
    )
    .await?;
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
    crate::quota::assert_within(&state, user.id, rustss_core::quotas::QuotaKind::Categories)
        .await?;
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
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(values): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let is_admin = user.role == "admin";
    let (csrf_token, tags, folder_id) = match parse_organization_form(values) {
        Ok(form) => form,
        Err(error) if is_ajax => return Err(AppError::Validation(error)),
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
        Ok(()) if is_ajax => Ok(StatusCode::NO_CONTENT.into_response()),
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
        Err(rustss_db::DbError::Validation(error)) if is_ajax => Err(AppError::Validation(error)),
        Err(rustss_db::DbError::Validation(error)) => {
            render_page(
                &state,
                user.id,
                user.email,
                is_admin,
                jar,
                None,
                Some(error),
            )
            .await
        }
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
    let feed = feeds::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let message = if feed.health_status == "disabled" {
        "This source is paused and was not refreshed.".to_string()
    } else if enqueue_fetch_feed(&state.db, id).await?.is_some() {
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
    let active: Vec<_> = feeds
        .iter()
        .filter(|feed| feed.health_status != "disabled")
        .map(|feed| feed.id)
        .collect();
    let outcome = queue_refreshes(&state, &active).await;
    let paused = feeds.len() - active.len();
    let is_admin = user.role == "admin";
    let mut message = format!(
        "Queued {} of {} active feed refreshes; {} were already queued.",
        outcome.queued,
        active.len(),
        outcome.already_queued
    );
    if paused > 0 {
        message.push_str(&format!(" {paused} paused source(s) were unchanged."));
    }
    if outcome.failed > 0 {
        message.push_str(&format!(" {} could not be queued.", outcome.failed));
    }
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

pub async fn move_to_folder(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<MoveFolderForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let is_admin = user.role == "admin";
    let folder_id = match parse_folder_id(&form.folder_id) {
        Ok(folder_id) => folder_id,
        Err(error) if is_ajax => return Err(AppError::Validation(error)),
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
            .await
        }
    };
    match subscriptions::move_many_to_category(state.db.pool(), user.id, &[id], folder_id).await {
        Ok(_) if is_ajax => Ok(StatusCode::NO_CONTENT.into_response()),
        Ok(_) => {
            let destination = folder_id.map_or("Unfiled", |_| "the selected folder");
            render_page(
                &state,
                user.id,
                user.email,
                is_admin,
                jar,
                Some(format!("Feed moved to {destination}.")),
                None,
            )
            .await
        }
        Err(DbError::NotFound) => Err(AppError::NotFound),
        Err(DbError::Validation(error)) if is_ajax => Err(AppError::Validation(error)),
        Err(DbError::Validation(error)) => {
            render_page(
                &state,
                user.id,
                user.email,
                is_admin,
                jar,
                None,
                Some(error),
            )
            .await
        }
        Err(error) => Err(error.into()),
    }
}

/// `POST /feeds/categories/{id}/collapse` — persist one personal folder's
/// disclosure state across browsers and sessions.
pub async fn set_category_collapsed(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CollapseFolderForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    match categories::set_collapsed(state.db.pool(), user.id, id, form.collapsed).await {
        Ok(()) if is_ajax => Ok(StatusCode::NO_CONTENT.into_response()),
        Ok(()) => Ok((jar, Redirect::to("/articles")).into_response()),
        Err(DbError::NotFound) => Err(AppError::NotFound),
        Err(error) => Err(error.into()),
    }
}

pub async fn bulk_move(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(values): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let is_admin = user.role == "admin";
    let form = match parse_bulk_form(values) {
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
            .await
        }
    };
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    match subscriptions::move_many_to_category(
        state.db.pool(),
        user.id,
        &form.feed_ids,
        form.folder_id,
    )
    .await
    {
        Ok(moved) => {
            let destination = form.folder_id.map_or("Unfiled", |_| "the selected folder");
            render_page(
                &state,
                user.id,
                user.email,
                is_admin,
                jar,
                Some(format!("Moved {moved} feed(s) to {destination}.")),
                None,
            )
            .await
        }
        Err(DbError::NotFound) => Err(AppError::NotFound),
        Err(DbError::Validation(error)) => {
            render_page(
                &state,
                user.id,
                user.email,
                is_admin,
                jar,
                None,
                Some(error),
            )
            .await
        }
        Err(error) => Err(error.into()),
    }
}

pub async fn bulk_tags(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(values): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let is_admin = user.role == "admin";
    let form = match parse_bulk_form(values) {
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
            .await
        }
    };
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if form.tags.is_empty() && form.tag_operation != subscriptions::BulkTagOperation::Replace {
        return render_page(
            &state,
            user.id,
            user.email,
            is_admin,
            jar,
            None,
            Some("Enter at least one tag to add or remove.".to_string()),
        )
        .await;
    }
    match subscriptions::update_tags_many(
        state.db.pool(),
        user.id,
        &form.feed_ids,
        form.tag_operation,
        &form.tags,
    )
    .await
    {
        Ok(updated) => {
            render_page(
                &state,
                user.id,
                user.email,
                is_admin,
                jar,
                Some(format!("Updated tags for {updated} feed(s).")),
                None,
            )
            .await
        }
        Err(DbError::NotFound) => Err(AppError::NotFound),
        Err(DbError::Validation(error)) => {
            render_page(
                &state,
                user.id,
                user.email,
                is_admin,
                jar,
                None,
                Some(error),
            )
            .await
        }
        Err(error) => Err(error.into()),
    }
}

/// Apply a personal content source + public-page lifetime to every selected
/// feed at once. Scoped strictly to the caller's own subscription rows.
pub async fn bulk_content(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(values): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let is_admin = user.role == "admin";
    let form = match parse_bulk_form(values) {
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
            .await
        }
    };
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let (Some(content_mode), Some(content_lifetime)) = (
        form.content_mode.as_deref(),
        form.content_lifetime.as_deref(),
    ) else {
        return render_page(
            &state,
            user.id,
            user.email,
            is_admin,
            jar,
            None,
            Some("Choose a content source and lifetime.".to_string()),
        )
        .await;
    };
    // Validate the selection once so an invalid choice fails fast and cleanly.
    if FeedContentMode::parse(content_mode).is_none()
        || ContentLifetime::parse(content_lifetime).is_none()
    {
        return render_page(
            &state,
            user.id,
            user.email,
            is_admin,
            jar,
            None,
            Some("Choose a valid content source and lifetime.".to_string()),
        )
        .await;
    }
    let mut updated = 0_usize;
    let mut queued = 0_usize;
    for feed_id in &form.feed_ids {
        // The bulk checkboxes only list the caller's feeds, but re-check so a
        // forged feed id cannot touch a subscription the user does not own.
        if !subscriptions::is_subscribed(state.db.pool(), user.id, *feed_id).await? {
            continue;
        }
        queued += save_personal_content_settings(
            &state,
            user.id,
            *feed_id,
            content_mode,
            content_lifetime,
            None,
            None,
        )
        .await?;
        updated += 1;
    }
    let message = if queued > 0 {
        format!(
            "Updated content settings for {updated} feed(s); queued {queued} page extraction(s)."
        )
    } else {
        format!("Updated content settings for {updated} feed(s).")
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

/// Destructive bulk operation scoped strictly to the caller's personal
/// subscription rows. Shared feeds and articles are never removed.
pub async fn bulk_unsubscribe(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(values): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let is_admin = user.role == "admin";
    let form = match parse_bulk_form(values) {
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
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !subscriptions::all_subscribed(state.db.pool(), user.id, &form.feed_ids).await? {
        return Err(AppError::NotFound);
    }
    let mut removed = 0_u64;
    for feed_id in form.feed_ids {
        if subscriptions::unsubscribe(state.db.pool(), user.id, feed_id).await? {
            removed += 1;
        }
    }
    render_page(
        &state,
        user.id,
        user.email,
        is_admin,
        jar,
        Some(format!(
            "Unsubscribed from {removed} selected feed(s). Shared content was preserved."
        )),
        None,
    )
    .await
}

pub async fn refresh_selected(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(values): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let is_admin = user.role == "admin";
    let form = match parse_bulk_form(values) {
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
            .await
        }
    };
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !subscriptions::all_subscribed(state.db.pool(), user.id, &form.feed_ids).await? {
        return Err(AppError::NotFound);
    }
    let selected = BTreeSet::from_iter(form.feed_ids.iter().copied());
    let feeds = feeds::list_for_user(state.db.pool(), user.id).await?;
    let active: Vec<_> = feeds
        .iter()
        .filter(|feed| selected.contains(&feed.id) && feed.health_status != "disabled")
        .map(|feed| feed.id)
        .collect();
    let paused = form.feed_ids.len() - active.len();
    let outcome = queue_refreshes(&state, &active).await;
    let mut message = format!(
        "Queued {} feed refresh(es); {} were already queued.",
        outcome.queued, outcome.already_queued
    );
    if paused > 0 {
        message.push_str(&format!(" {paused} paused source(s) were unchanged."));
    }
    if outcome.failed > 0 {
        message.push_str(&format!(" {} could not be queued.", outcome.failed));
    }
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

pub async fn refresh_everything(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if user.role != "admin" {
        return Err(AppError::Forbidden);
    }
    let active_feed_ids = feeds::list_active_ids(state.db.pool()).await?;
    let feed_ids = feeds::reset_refresh_schedule(state.db.pool(), &active_feed_ids).await?;
    let rescheduled = jobs::run_pending_feed_fetches_now(state.db.pool(), &feed_ids).await?;
    let outcome = queue_refreshes(&state, &feed_ids).await;
    let mut message = format!(
        "Reset scheduling for {} active shared source(s), queued {} new refresh(es), and released {rescheduled} delayed refresh(es). Paused sources were unchanged.",
        feed_ids.len(), outcome.queued
    );
    if outcome.already_queued > 0 {
        message.push_str(&format!(" {} were already queued.", outcome.already_queued));
    }
    if outcome.failed > 0 {
        message.push_str(&format!(" {} could not be queued.", outcome.failed));
    }
    render_page(&state, user.id, user.email, true, jar, Some(message), None).await
}

/// `GET /feeds/{id}/settings`.
pub async fn settings_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let is_admin = user.role == "admin";
    let is_subscribed = subscriptions::is_subscribed(state.db.pool(), user.id, id).await?;
    if !is_admin && !is_subscribed {
        return Err(AppError::NotFound);
    }
    let feed = feeds::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    render_settings_page(&state, user.id, user.email, jar, &feed, is_admin, None).await
}

async fn save_personal_content_settings(
    state: &AppState,
    user_id: Uuid,
    feed_id: Uuid,
    content_mode: &str,
    content_lifetime: &str,
    youtube_mode: Option<SubscriptionYouTubeMode>,
    enclosure_settings: Option<SubscriptionEnclosureSettings>,
) -> Result<usize, AppError> {
    let mode = FeedContentMode::parse(content_mode)
        .ok_or_else(|| AppError::Validation("Choose a valid content source.".to_string()))?;
    let lifetime = ContentLifetime::parse(content_lifetime)
        .ok_or_else(|| AppError::Validation("Choose a valid content lifetime.".to_string()))?;
    let content = SubscriptionContentSettings { mode, lifetime };
    if let Some(youtube_mode) = youtube_mode {
        subscriptions::update_reader_settings(
            state.db.pool(),
            user_id,
            feed_id,
            content,
            youtube_mode,
            enclosure_settings.unwrap_or_default(),
        )
        .await?;
    } else {
        subscriptions::update_content_settings(state.db.pool(), user_id, feed_id, content).await?;
    }
    let shared_lifetime =
        subscriptions::requested_article_page_lifetime(state.db.pool(), feed_id).await?;
    articles::apply_article_page_lifetime(state.db.pool(), feed_id, shared_lifetime).await?;
    if mode.fetches_article_pages() {
        queue_page_extractions(state, feed_id).await
    } else {
        Ok(0)
    }
}

/// `POST /feeds/{id}/content-settings` — personal quick settings from Library.
pub async fn update_personal_content_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<PersonalContentSettingsForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !subscriptions::is_subscribed(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    save_personal_content_settings(
        &state,
        user.id,
        id,
        &form.content_mode,
        &form.content_lifetime,
        None,
        None,
    )
    .await?;
    if is_ajax {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Ok((jar, Redirect::to(&format!("/articles?feed={id}"))).into_response())
    }
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
    let is_admin = user.role == "admin";
    let is_subscribed = subscriptions::is_subscribed(state.db.pool(), user.id, id).await?;
    if !is_admin && !is_subscribed {
        return Err(AppError::NotFound);
    }
    let feed = feeds::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let mut messages = Vec::new();
    if is_subscribed {
        let entry_filters = parse_entry_filters(&form.entry_filters_json)?;
        validate_entry_filters(&entry_filters).map_err(AppError::Validation)?;
        let youtube_mode = match form.youtube_player_mode.as_deref() {
            Some(value) => SubscriptionYouTubeMode::parse(value).ok_or_else(|| {
                AppError::Validation("Choose a valid YouTube player preference.".to_string())
            })?,
            None => SubscriptionYouTubeMode::Inherit,
        };
        let enclosure_settings = SubscriptionEnclosureSettings {
            delivery: match form.enclosure_delivery_mode.as_deref() {
                Some(value) => SubscriptionEnclosureDelivery::parse(value).ok_or_else(|| {
                    AppError::Validation("Choose a valid attachment delivery override.".to_string())
                })?,
                None => SubscriptionEnclosureDelivery::Inherit,
            },
            load: match form.enclosure_load_mode.as_deref() {
                Some(value) => SubscriptionEnclosureLoad::parse(value).ok_or_else(|| {
                    AppError::Validation("Choose a valid attachment loading override.".to_string())
                })?,
                None => SubscriptionEnclosureLoad::Inherit,
            },
        };
        let queued = save_personal_content_settings(
            &state,
            user.id,
            id,
            form.content_mode.as_deref().unwrap_or_default(),
            form.content_lifetime.as_deref().unwrap_or_default(),
            Some(youtube_mode),
            Some(enclosure_settings),
        )
        .await?;
        messages.push(format!(
            "Your content preference was saved. {queued} public-page update(s) were queued."
        ));
        messages.push("Your YouTube player override was saved.".to_string());
        messages.push("Your attachment overrides were saved.".to_string());
        subscriptions::update_entry_filters(state.db.pool(), user.id, id, &entry_filters).await?;
        messages.push("Your entry filters were saved.".to_string());
    }

    if is_admin {
        let rewrite_rules = parse_rewrite_rules(&form.rewrite_rules_json)?;
        validate_rewrite_rules(&rewrite_rules).map_err(AppError::Validation)?;
        let fetch_interval_minutes =
            parse_interval(&form.fetch_interval_minutes).map_err(AppError::Validation)?;
        let extraction_delay_ms =
            parse_delay(&form.extraction_delay_ms).map_err(AppError::Validation)?;
        let respect_robots_txt =
            parse_robots_policy(form.respect_robots_txt.as_deref(), feed.respect_robots_txt)
                .map_err(AppError::Validation)?;
        let selector = (!form.selector.trim().is_empty()).then(|| form.selector.trim().to_string());
        let mut shared = content_settings(&feed);
        let rewrite_rules_changed = shared.rewrite_rules != rewrite_rules;
        shared.selector = selector;
        shared.rewrite_rules = rewrite_rules;
        feeds::update_content_settings(
            state.db.pool(),
            id,
            &shared.to_json(),
            respect_robots_txt,
            extraction_delay_ms,
            fetch_interval_minutes,
        )
        .await?;
        rewrite_rules::create(
            state.db.pool(),
            id,
            user.id,
            &serde_json::to_value(&shared.rewrite_rules).map_err(|_| {
                AppError::Internal(anyhow::anyhow!("rewrite rules could not be serialized"))
            })?,
        )
        .await?;
        if rewrite_rules_changed {
            feeds::clear_fetch_validators(state.db.pool(), id).await?;
            let invalidated =
                articles::invalidate_article_page_processing_for_feed(state.db.pool(), id).await?;
            messages.push(format!(
                "Rewrite rules changed; the next shared refresh will rebuild RSS content and {invalidated} stored public-page copy/copies."
            ));
        }
        messages.push("Shared acquisition safety settings were saved.".to_string());
    }

    let refresh_queued = enqueue_fetch_feed(&state.db, id).await?.is_some();
    let updated = feeds::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    messages.push(if refresh_queued {
        "A shared feed refresh was queued.".to_string()
    } else {
        "A shared feed refresh is already queued.".to_string()
    });
    render_settings_page(
        &state,
        user.id,
        user.email,
        jar,
        &updated,
        is_admin,
        Some(messages.join(" ")),
    )
    .await
}

/// Validate both rule sets and preview their effect against recent shared items
/// without changing stored content or personal state.
pub async fn preview_rules(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ContentSettingsForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let is_admin = user.role == "admin";
    let subscribed = subscriptions::is_subscribed(state.db.pool(), user.id, id).await?;
    if !is_admin && !subscribed {
        return Err(AppError::NotFound);
    }
    let feed = feeds::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let entry_filters = if subscribed {
        parse_entry_filters(&form.entry_filters_json)?
    } else {
        Vec::new()
    };
    validate_entry_filters(&entry_filters).map_err(AppError::Validation)?;
    let rewrite_rules = if is_admin {
        parse_rewrite_rules(&form.rewrite_rules_json)?
    } else {
        Vec::new()
    };
    validate_rewrite_rules(&rewrite_rules).map_err(AppError::Validation)?;
    let recent = articles::list_for_feed(state.db.pool(), id, 20).await?;
    let mut changed = 0;
    let mut filtered = 0;
    for article in &recent {
        if is_admin
            && apply_rewrite_rules(&article.content, &rewrite_rules)
                .map_err(AppError::Validation)?
                != article.content
        {
            changed += 1;
        }
        if subscribed
            && !entry_is_visible(
                &article.title,
                &article.content,
                article.author.as_deref(),
                &article.url,
                &entry_filters,
            )
            .map_err(AppError::Validation)?
        {
            filtered += 1;
        }
    }
    render_settings_page(
        &state,
        user.id,
        user.email,
        jar,
        &feed,
        is_admin,
        Some(format!(
            "Preview only: {changed} of {} recent articles would be rewritten; {filtered} would be hidden from your Library.",
            recent.len()
        )),
    )
    .await
}

pub async fn restore_rewrite_rules(
    State(state): State<AppState>,
    crate::auth::extractor::AdminUser(user): crate::auth::extractor::AdminUser,
    jar: CookieJar,
    Path((feed_id, version_id)): Path<(Uuid, Uuid)>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let feed = feeds::find_by_id(state.db.pool(), feed_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let version = rewrite_rules::find(state.db.pool(), version_id, feed_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let rules: Vec<RewriteRule> = serde_json::from_value(version.rules)
        .map_err(|_| AppError::Validation("Stored rewrite-rule version is invalid.".into()))?;
    validate_rewrite_rules(&rules).map_err(AppError::Validation)?;
    let mut shared = content_settings(&feed);
    shared.rewrite_rules = rules;
    feeds::update_content_settings(
        state.db.pool(),
        feed_id,
        &shared.to_json(),
        feed.respect_robots_txt,
        feed.extraction_delay_ms,
        feed.fetch_interval_minutes,
    )
    .await?;
    rewrite_rules::create(
        state.db.pool(),
        feed_id,
        user.id,
        &serde_json::to_value(&shared.rewrite_rules).unwrap_or_default(),
    )
    .await?;
    feeds::clear_fetch_validators(state.db.pool(), feed_id).await?;
    let invalidated =
        articles::invalidate_article_page_processing_for_feed(state.db.pool(), feed_id).await?;
    let _ = enqueue_fetch_feed(&state.db, feed_id).await?;
    let updated = feeds::find_by_id(state.db.pool(), feed_id)
        .await?
        .ok_or(AppError::NotFound)?;
    render_settings_page(
        &state,
        user.id,
        user.email,
        jar,
        &updated,
        true,
        Some(format!(
            "Rewrite rules restored from version {}. The next shared refresh will rebuild RSS content and {} stored public-page copy/copies.",
            version.version, invalidated
        )),
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
