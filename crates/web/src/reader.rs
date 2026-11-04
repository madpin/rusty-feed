//! The reading experience: article list, per-article actions (read/bookmark),
//! and article reading (side panel, standalone page, and inline fragment).

use std::collections::{BTreeMap, HashMap, HashSet};

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use rusty_feed_core::ai_budget::AiBudget;
use rusty_feed_core::feed_content::{
    SubscriptionContentMode, SubscriptionContentSettings, SubscriptionEnclosureDelivery,
    SubscriptionEnclosureLoad, SubscriptionEnclosureSettings,
};
use rusty_feed_core::html::{
    apply_external_link_navigation, sanitize_article_html_with_policies,
    ARTICLE_PAGE_PROCESSING_VERSION,
};
use rusty_feed_core::reading::{apply_adhd_reading, autolink_bare_urls};
use rusty_feed_core::youtube::{SubscriptionYouTubeMode, YouTubePolicy, YouTubeVideo};
use rusty_feed_db::{
    articles, bookmarks, categories, cost_ledger, feed_icons, feedback, feeds, jobs, llm_profiles,
    patterns, read_articles, seen_articles, stories, subscriptions, youtube_metadata,
};
use rusty_feed_search::personalize;
use rusty_feed_search::{extract_keywords, relevance_score, ExplicitFeedback};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::extract::Ajax;
use crate::prefs;
use crate::public_shares::{self, ShareControls};
use crate::state::AppState;

/// Candidate window scored when sorting by relevance.
const RELEVANCE_CANDIDATES: i64 = 120;
/// Default implicit bounce threshold (ARCH §11.2).
const BOUNCE_THRESHOLD: f64 = 0.25;
const TEMPORARY_PAGE_LIFETIME_DAYS: u16 = 1;

struct ArticleView {
    id: String,
    feed_id: String,
    title: String,
    excerpt: Option<String>,
    image_url: Option<String>,
    feed_name: String,
    published: Option<String>,
    is_read: bool,
    is_seen: bool,
    is_bookmarked: bool,
    has_summary: bool,
    story_count: i32,
    can_summarize: bool,
    is_liked: bool,
    is_disliked: bool,
    score: Option<i64>,
}

#[derive(Clone)]
struct EnclosureView {
    title: String,
    url: String,
    open_url: String,
    details: Option<String>,
    is_audio: bool,
    is_video: bool,
    is_image: bool,
    can_embed: bool,
    load_on_demand: bool,
    show_direct_link: bool,
    publisher_url: String,
    show_publisher_link: bool,
    proxied: bool,
}

#[derive(Clone)]
struct YouTubeView {
    title: String,
    player_name: String,
    watch_url: String,
    start_label: Option<String>,
    duration_label: Option<String>,
    embed_url: Option<String>,
    autoplay: bool,
    referrer_policy: String,
}

#[derive(Clone)]
struct FeedNav {
    id: String,
    name: String,
    initial: String,
    has_icon: bool,
    active: bool,
    href: String,
    folder_id: String,
    tags_input: String,
    content_mode: String,
    content_lifetime: String,
    unread_count: i64,
}

struct FolderNav {
    id: String,
    name: String,
    depth: usize,
    feed_count: i64,
    active: bool,
    collapsed: bool,
    href: String,
    feeds: Vec<FeedNav>,
}

struct TagNav {
    name: String,
    feed_count: i64,
    active: bool,
    href: String,
}

struct FolderNavContext<'a> {
    selected: Option<Uuid>,
    tag: Option<&'a str>,
    unread: Option<bool>,
    sort: &'a str,
    summary_only: bool,
    feeds_by_folder: &'a mut HashMap<Uuid, Vec<FeedNav>>,
}

#[derive(Template)]
#[template(path = "articles.html")]
struct ArticlesTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    articles: Vec<ArticleView>,
    folders: Vec<FolderNav>,
    unfiled_feeds: Vec<FeedNav>,
    tags: Vec<TagNav>,
    recent_url: String,
    unread_url: String,
    relevance_url: String,
    summary_url: String,
    unseen_url: String,
    scope_label: String,
    feed_filter: Option<String>,
    category_filter: Option<String>,
    tag_filter: Option<String>,
    unfiled_url: String,
    unread_only: bool,
    unseen_only: bool,
    summary_only: bool,
    unfiled_only: bool,
    reading_mode: String,
    side_panel: bool,
    sort: String,
    back_url: String,
    has_prev: bool,
    has_next: bool,
    prev_url: String,
    next_url: String,
    filtered_count: usize,
    show_ai_key_cue: bool,
}

#[derive(Template)]
#[template(path = "article.html")]
struct ArticleTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    id: String,
    title: String,
    feed_name: String,
    url: Option<String>,
    url_label: String,
    alternate_url: Option<String>,
    alternate_url_label: String,
    live_view_url: Option<String>,
    content_mode: String,
    default_content_mode: String,
    content_mode_return_url: String,
    content_mode_panel_url: String,
    comments_url: Option<String>,
    published: Option<String>,
    image_url: Option<String>,
    enclosures: Vec<EnclosureView>,
    youtube_videos: Vec<YouTubeView>,
    content: String, // sanitized HTML
    content_notice: Option<String>,
    content_pending: bool,
    blocked_tracking_pixels: Option<u32>,
    is_read: bool,
    is_bookmarked: bool,
    is_liked: bool,
    is_disliked: bool,
    summary: Option<String>,
    key_points: Vec<String>,
    topics: Vec<String>,
    can_summarize: bool,
    story_summary: Option<String>,
    story_sources: Vec<StorySource>,
    external_new_tab: bool,
    share: ShareControls,
}

struct StorySource {
    title: String,
    href: String,
    feed_name: String,
}

#[derive(Template)]
#[template(path = "article_panel.html")]
struct ArticlePanelTemplate {
    csrf_token: String,
    ui: prefs::Preferences,
    id: String,
    title: String,
    feed_name: String,
    url: Option<String>,
    url_label: String,
    alternate_url: Option<String>,
    alternate_url_label: String,
    live_view_url: Option<String>,
    content_mode: String,
    default_content_mode: String,
    content_mode_return_url: String,
    content_mode_panel_url: String,
    comments_url: Option<String>,
    published: Option<String>,
    image_url: Option<String>,
    enclosures: Vec<EnclosureView>,
    youtube_videos: Vec<YouTubeView>,
    content: String,
    content_notice: Option<String>,
    content_pending: bool,
    blocked_tracking_pixels: Option<u32>,
    is_read: bool,
    is_bookmarked: bool,
    is_liked: bool,
    is_disliked: bool,
    summary: Option<String>,
    key_points: Vec<String>,
    topics: Vec<String>,
    can_summarize: bool,
    story_summary: Option<String>,
    story_sources: Vec<StorySource>,
    external_new_tab: bool,
    share: ShareControls,
}

#[derive(Template)]
#[template(path = "articles_fragment.html")]
struct ArticlesFragmentTemplate {
    csrf_token: String,
    articles: Vec<ArticleView>,
    reading_mode: String,
    side_panel: bool,
    back_url: String,
    has_next: bool,
    next_url: String,
}

struct ReaderView {
    id: String,
    title: String,
    feed_name: String,
    url: Option<String>,
    url_label: String,
    alternate_url: Option<String>,
    alternate_url_label: String,
    live_view_url: Option<String>,
    content_mode: String,
    default_content_mode: String,
    content_mode_return_url: String,
    content_mode_panel_url: String,
    comments_url: Option<String>,
    published: Option<String>,
    image_url: Option<String>,
    enclosures: Vec<EnclosureView>,
    youtube_videos: Vec<YouTubeView>,
    content: String,
    content_notice: Option<String>,
    content_pending: bool,
    blocked_tracking_pixels: Option<u32>,
    is_read: bool,
    is_bookmarked: bool,
    is_liked: bool,
    is_disliked: bool,
    summary: Option<String>,
    key_points: Vec<String>,
    topics: Vec<String>,
    can_summarize: bool,
    story_summary: Option<String>,
    story_sources: Vec<StorySource>,
    external_new_tab: bool,
    share: ShareControls,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ContentAvailability {
    Ready,
    Pending,
    Failed,
}

impl ContentAvailability {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Pending => "pending",
            Self::Failed => "failed",
        }
    }
}

fn article_page_is_current(article: &rusty_feed_db::Article) -> bool {
    article.article_page_content.is_some()
        && article.article_page_processing_version >= ARTICLE_PAGE_PROCESSING_VERSION
        && article
            .article_page_expires_at
            .is_none_or(|expires_at| expires_at > OffsetDateTime::now_utc())
}

async fn enqueue_article_page_on_access(
    state: &AppState,
    article: &rusty_feed_db::Article,
    temporary: bool,
) -> Result<(), AppError> {
    let dedup = format!("extract:{}:{}", article.id, article.content_revision);
    // A user is actively waiting on this page, so use the reserved interactive
    // lane rather than the background lane it would share with feed refreshes
    // and bulk backfills.
    jobs::enqueue_in_lane(
        state.db.pool(),
        jobs::QueueLane::Interactive,
        "extract_content",
        serde_json::json!({
            "article_id": article.id,
            "content_revision": article.content_revision,
            "on_demand_lifetime_days": temporary.then_some(TEMPORARY_PAGE_LIFETIME_DAYS),
        }),
        Some(&dedup),
        None,
        3,
    )
    .await?;
    Ok(())
}

async fn selected_content(
    state: &AppState,
    article: &rusty_feed_db::Article,
    mode: SubscriptionContentMode,
    temporary: bool,
) -> Result<(String, Option<String>, ContentAvailability, u32), AppError> {
    if mode != SubscriptionContentMode::ArticlePage {
        return Ok((
            article
                .feed_content
                .clone()
                .unwrap_or_else(|| article.content.clone()),
            (mode == SubscriptionContentMode::Live).then(|| {
                if safe_external_url(Some(&article.url)).is_some() {
                    "Live View is selected for this feed. Stored RSS content remains available when Live View cannot run."
                } else {
                    "Live View is selected for this feed, but this item has no public publisher page. Showing its stored RSS content instead."
                }
                .to_string()
            }),
            ContentAvailability::Ready,
            article.feed_blocked_tracking_pixels.max(0) as u32,
        ));
    }

    if safe_external_url(Some(&article.url)).is_none() {
        return Ok((
            article
                .feed_content
                .clone()
                .unwrap_or_else(|| article.content.clone()),
            Some(
                "This feed item has no public article page. Showing its stored feed content and attachments."
                    .to_string(),
            ),
            ContentAvailability::Failed,
            article.feed_blocked_tracking_pixels.max(0) as u32,
        ));
    }

    if article_page_is_current(article) {
        return Ok((
            article.article_page_content.clone().unwrap_or_default(),
            None,
            ContentAvailability::Ready,
            article.article_page_blocked_tracking_pixels.max(0) as u32,
        ));
    }

    if article.extraction_status == "failed"
        && article.updated_at > OffsetDateTime::now_utc() - Duration::minutes(15)
    {
        return Ok((
            article
                .feed_content
                .clone()
                .unwrap_or_else(|| article.content.clone()),
            Some(
                "The public page could not be updated recently. Showing the stored RSS content; Rusty Feed will retry on a later access."
                    .to_string(),
            ),
            ContentAvailability::Failed,
            article.feed_blocked_tracking_pixels.max(0) as u32,
        ));
    }

    enqueue_article_page_on_access(state, article, temporary).await?;
    Ok((
        article
            .feed_content
            .clone()
            .unwrap_or_else(|| article.content.clone()),
        Some(
            "The requested public-page content is being updated now. Showing the stored RSS content in the meantime."
                .to_string(),
        ),
        ContentAvailability::Pending,
        article.feed_blocked_tracking_pixels.max(0) as u32,
    ))
}

#[derive(Deserialize)]
pub struct ArticlesQuery {
    #[serde(default)]
    unread: Option<String>,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    feed: Option<Uuid>,
    #[serde(default)]
    category: Option<Uuid>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    unfiled: Option<String>,
    /// `recent` (default) or `relevance`.
    #[serde(default)]
    sort: Option<String>,
    /// `1` shows only articles with a ready AI summary (implies unread).
    #[serde(default)]
    summary: Option<String>,
    /// `1` hides articles the user has already opened.
    #[serde(default)]
    unseen: Option<String>,
    /// `items` returns the next card page for infinite scroll.
    #[serde(default)]
    fragment: Option<String>,
}

#[derive(Deserialize)]
pub struct FeedbackForm {
    csrf_token: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    time_spent: Option<f64>,
    #[serde(default)]
    next: Option<String>,
}

#[derive(Deserialize)]
pub struct ActionForm {
    csrf_token: String,
    #[serde(default)]
    next: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct ContentViewQuery {
    #[serde(default)]
    content: Option<String>,
}

impl ContentViewQuery {
    fn mode(&self) -> Option<SubscriptionContentMode> {
        self.content
            .as_deref()
            .and_then(SubscriptionContentMode::parse)
    }
}

#[derive(Deserialize)]
pub struct ContentModeForm {
    csrf_token: String,
    content_mode: String,
    #[serde(default)]
    next: Option<String>,
}

#[derive(Deserialize)]
pub struct MarkAllReadForm {
    csrf_token: String,
    #[serde(default)]
    next: Option<String>,
    #[serde(default)]
    feed: Option<Uuid>,
    #[serde(default)]
    category: Option<Uuid>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    unfiled: Option<String>,
}

/// Only allow same-origin relative redirects (avoid open-redirects).
fn safe_next(next: Option<String>) -> String {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") => n,
        _ => "/articles".to_string(),
    }
}

fn safe_external_url(value: Option<&str>) -> Option<String> {
    let value = value?;
    let mut url = url::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    match url.host()? {
        url::Host::Domain(host)
            if host.trim_end_matches('.').eq_ignore_ascii_case("localhost")
                || host
                    .trim_end_matches('.')
                    .to_ascii_lowercase()
                    .ends_with(".localhost") =>
        {
            return None;
        }
        url::Host::Ipv4(ip) if !rusty_feed_feeds::is_global_ip(&std::net::IpAddr::V4(ip)) => {
            return None;
        }
        url::Host::Ipv6(ip) if !rusty_feed_feeds::is_global_ip(&std::net::IpAddr::V6(ip)) => {
            return None;
        }
        _ => {}
    }
    url.set_fragment(None);
    Some(url.to_string())
}

fn feed_initial(name: &str) -> String {
    name.chars()
        .find(|character| character.is_alphanumeric())
        .map(|character| character.to_uppercase().collect())
        .unwrap_or_else(|| "•".to_string())
}

fn normalized_tag(value: Option<String>) -> Result<Option<String>, String> {
    let Some(tag) = value else {
        return Ok(None);
    };
    let tag = tag.trim().to_string();
    if tag.is_empty() || tag.chars().count() > 40 || tag.chars().any(char::is_control) {
        return Err("Tag filters must be between 1 and 40 printable characters.".to_string());
    }
    Ok(Some(tag))
}

#[allow(clippy::too_many_arguments)]
fn articles_url(
    feed_id: Option<Uuid>,
    category_id: Option<Uuid>,
    unfiled: bool,
    tag: Option<&str>,
    unread: Option<bool>,
    sort: &str,
    offset: Option<i64>,
    summary_only: bool,
) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    if let Some(unread) = unread {
        query.append_pair("unread", if unread { "1" } else { "0" });
    }
    if let Some(feed_id) = feed_id {
        query.append_pair("feed", &feed_id.to_string());
    }
    if let Some(category_id) = category_id {
        query.append_pair("category", &category_id.to_string());
    }
    if unfiled {
        query.append_pair("unfiled", "1");
    }
    if let Some(tag) = tag {
        query.append_pair("tag", tag);
    }
    if sort == "relevance" {
        query.append_pair("sort", "relevance");
    }
    if let Some(offset) = offset.filter(|offset| *offset > 0) {
        query.append_pair("offset", &offset.to_string());
    }
    if summary_only {
        query.append_pair("summary", "1");
    }
    let query = query.finish();
    if query.is_empty() {
        "/articles".to_string()
    } else {
        format!("/articles?{query}")
    }
}

fn with_unseen(url: String, unseen: bool) -> String {
    if !unseen {
        return url;
    }
    if url.contains('?') {
        format!("{url}&unseen=1")
    } else {
        format!("{url}?unseen=1")
    }
}

fn append_folder_nav(
    parent_id: Option<Uuid>,
    depth: usize,
    children: &mut BTreeMap<Option<Uuid>, Vec<categories::CategoryWithFeedCount>>,
    seen: &mut HashSet<Uuid>,
    output: &mut Vec<FolderNav>,
    context: &mut FolderNavContext<'_>,
) {
    let Some(categories) = children.remove(&parent_id) else {
        return;
    };
    for category in categories {
        if !seen.insert(category.id) {
            continue;
        }
        let category_id = category.id;
        output.push(FolderNav {
            id: category_id.to_string(),
            name: category.name,
            depth,
            feed_count: category.feed_count,
            active: Some(category_id) == context.selected,
            collapsed: category.collapsed,
            href: articles_url(
                None,
                Some(category_id),
                false,
                context.tag,
                context.unread,
                context.sort,
                None,
                context.summary_only,
            ),
            feeds: context
                .feeds_by_folder
                .remove(&category_id)
                .unwrap_or_default(),
        });
        append_folder_nav(
            Some(category_id),
            depth + 1,
            children,
            seen,
            output,
            context,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn folder_nav(
    categories: Vec<categories::CategoryWithFeedCount>,
    selected: Option<Uuid>,
    tag: Option<&str>,
    unread: Option<bool>,
    sort: &str,
    summary_only: bool,
    feeds_by_folder: &mut HashMap<Uuid, Vec<FeedNav>>,
) -> Vec<FolderNav> {
    let mut children = BTreeMap::<Option<Uuid>, Vec<_>>::new();
    for category in categories {
        children
            .entry(category.parent_id)
            .or_default()
            .push(category);
    }
    let mut context = FolderNavContext {
        selected,
        tag,
        unread,
        sort,
        summary_only,
        feeds_by_folder,
    };
    let mut output = Vec::new();
    let mut seen = HashSet::new();
    append_folder_nav(None, 0, &mut children, &mut seen, &mut output, &mut context);
    for category in children.into_values().flatten() {
        if seen.insert(category.id) {
            let category_id = category.id;
            output.push(FolderNav {
                id: category_id.to_string(),
                name: category.name,
                depth: 0,
                feed_count: category.feed_count,
                active: Some(category_id) == context.selected,
                collapsed: category.collapsed,
                href: articles_url(
                    None,
                    Some(category_id),
                    false,
                    context.tag,
                    context.unread,
                    context.sort,
                    None,
                    context.summary_only,
                ),
                feeds: context
                    .feeds_by_folder
                    .remove(&category_id)
                    .unwrap_or_default(),
            });
        }
    }
    output
}

/// `GET /articles` — the user's reading list.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Query(q): Query<ArticlesQuery>,
) -> Result<Response, AppError> {
    let prefs = prefs::load(&state.db, user.id).await;
    let is_admin = user.role == "admin";
    let per_page = prefs.articles_per_page.clamp(5, 100) as i64;
    let unread_filter = match q.unread.as_deref() {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    };
    // "Unread AI summaries" is one atomic filter — show only summaries the
    // reader hasn't seen yet — so it forces the unread scope regardless of the
    // "show read articles" preference, the same way the plain "Unread only"
    // link already overrides it.
    let summary_only = q.summary.as_deref() == Some("1");
    let unseen_only = q.unseen.as_deref() == Some("1");
    let tag = normalized_tag(q.tag).map_err(AppError::Validation)?;
    let unread_only =
        unread_filter.unwrap_or(tag.is_none() && !prefs.show_read_articles) || summary_only;
    let offset = q.offset.unwrap_or(0).max(0);
    let feed_id = q.feed;
    let unfiled = q.unfiled.as_deref() == Some("1");
    let category_id = if unfiled { None } else { q.category };
    if let Some(category_id) = category_id {
        if !categories::belongs_to_user(state.db.pool(), user.id, category_id).await? {
            return Err(AppError::NotFound);
        }
    }
    let sort = if q.sort.as_deref() == Some("relevance") {
        "relevance"
    } else {
        "recent"
    };

    let filter = articles::LibraryFilter {
        feed_id,
        category_id,
        unfiled,
        tag: tag.as_deref(),
        unseen_only,
    };
    let (article_views, has_next, filtered_count) = if sort == "relevance" {
        relevance_page(
            &state,
            &prefs,
            user.id,
            filter,
            unread_only,
            summary_only,
            per_page,
            offset,
        )
        .await?
    } else {
        recency_page(
            &state,
            &prefs,
            user.id,
            filter,
            unread_only,
            summary_only,
            per_page,
            offset,
        )
        .await?
    };

    if q.fragment.as_deref() == Some("items") {
        let side_panel = prefs.reading_mode == "side-panel" && !article_views.is_empty();
        let back_url = with_unseen(
            articles_url(
                feed_id,
                category_id,
                unfiled,
                tag.as_deref(),
                Some(unread_only),
                sort,
                Some(offset),
                summary_only,
            ),
            unseen_only,
        );
        let next_url = with_unseen(
            articles_url(
                feed_id,
                category_id,
                unfiled,
                tag.as_deref(),
                Some(unread_only),
                sort,
                Some(offset + per_page),
                summary_only,
            ),
            unseen_only,
        );
        let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
        let body = ArticlesFragmentTemplate {
            csrf_token,
            articles: article_views,
            reading_mode: prefs.reading_mode,
            side_panel,
            back_url,
            has_next,
            next_url,
        }
        .render()
        .map(Html)
        .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
        let mut response = (jar, body).into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
        return Ok(response);
    }

    let (feed_records, category_records, tag_records, organizations, unread_counts) = tokio::try_join!(
        feeds::list_for_user(state.db.pool(), user.id),
        categories::list_with_feed_counts(state.db.pool(), user.id),
        categories::list_tags(state.db.pool(), user.id),
        subscriptions::list_organization(state.db.pool(), user.id),
        articles::unread_counts_by_feed(state.db.pool(), user.id),
    )?;
    let unread_by_feed: HashMap<Uuid, i64> = unread_counts.into_iter().collect();
    let category_name = category_id.and_then(|category_id| {
        category_records
            .iter()
            .find(|category| category.id == category_id)
            .map(|category| category.name.as_str())
    });
    let scope_label = if unfiled {
        "Unfiled".to_string()
    } else {
        match (category_name, tag.as_deref()) {
            (Some(category), Some(tag)) => format!("{category} · #{tag}"),
            (Some(category), None) => format!("Folder: {category}"),
            (None, Some(tag)) => format!("Tag: #{tag}"),
            (None, None) if unseen_only => "Unseen".to_string(),
            (None, None) if summary_only => "Unread AI summaries".to_string(),
            (None, None) if unread_only => "Unread only".to_string(),
            (None, None) if sort == "relevance" => "Personalized".to_string(),
            (None, None) => "Most recent".to_string(),
        }
    };
    let organizations: HashMap<Uuid, subscriptions::FeedOrganization> = organizations
        .into_iter()
        .map(|organization| (organization.feed_id, organization))
        .collect();
    let feed_ids: Vec<Uuid> = feed_records.iter().map(|feed| feed.id).collect();
    let icon_ids = feed_icons::available_for_feeds(state.db.pool(), &feed_ids).await?;
    let mut feeds_by_folder = HashMap::<Uuid, Vec<FeedNav>>::new();
    let mut unfiled_feeds = Vec::new();
    for feed in feed_records {
        let organization = organizations.get(&feed.id);
        let display_name = organization
            .and_then(|organization| organization.custom_name.as_deref())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&feed.name)
            .to_string();
        let content_settings = organization
            .map(|organization| {
                SubscriptionContentSettings::from_json(organization.settings.as_ref())
            })
            .unwrap_or_default();
        let id = feed.id;
        let nav = FeedNav {
            id: id.to_string(),
            initial: feed_initial(&display_name),
            has_icon: icon_ids.contains(&id),
            active: Some(id) == feed_id,
            href: articles_url(
                Some(id),
                None,
                false,
                None,
                Some(unread_only),
                sort,
                None,
                summary_only,
            ),
            folder_id: organization
                .and_then(|organization| organization.category_id)
                .map(|folder_id| folder_id.to_string())
                .unwrap_or_default(),
            tags_input: organization
                .map(|organization| organization.tags.join(", "))
                .unwrap_or_default(),
            content_mode: content_settings.mode.as_str().to_string(),
            content_lifetime: content_settings.lifetime.form_value(),
            unread_count: unread_by_feed.get(&id).copied().unwrap_or(0),
            name: display_name,
        };
        if let Some(folder_id) = organization.and_then(|organization| organization.category_id) {
            feeds_by_folder.entry(folder_id).or_default().push(nav);
        } else {
            unfiled_feeds.push(nav);
        }
    }
    let folders = folder_nav(
        category_records,
        category_id,
        tag.as_deref(),
        Some(unread_only),
        sort,
        summary_only,
        &mut feeds_by_folder,
    );
    let tags = tag_records
        .into_iter()
        .map(|tag_record| {
            let href = articles_url(
                None,
                None,
                false,
                Some(&tag_record.name),
                None,
                "recent",
                None,
                false,
            );
            TagNav {
                active: tag
                    .as_deref()
                    .is_some_and(|selected| selected.eq_ignore_ascii_case(&tag_record.name)),
                name: tag_record.name,
                feed_count: tag_record.feed_count,
                href,
            }
        })
        .collect();
    let side_panel = prefs.reading_mode == "side-panel" && !article_views.is_empty();
    let back_url = with_unseen(
        articles_url(
            feed_id,
            category_id,
            unfiled,
            tag.as_deref(),
            Some(unread_only),
            sort,
            Some(offset),
            summary_only,
        ),
        unseen_only,
    );
    // Nudge keyless users to bring their own AI key once, at the top of the list,
    // instead of repeating the pitch inside every article (§8.1.3).
    let show_ai_key_cue = state.settings.summarization_enabled
        && !rusty_feed_db::llm_profiles::has_enabled_user_profile(state.db.pool(), user.id)
            .await
            .unwrap_or(false);
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);

    let body = ArticlesTemplate {
        theme: prefs.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui: prefs.clone(),
        is_admin,
        articles: article_views,
        folders,
        unfiled_feeds,
        tags,
        recent_url: articles_url(
            None,
            category_id,
            unfiled,
            tag.as_deref(),
            Some(false),
            "recent",
            None,
            false,
        ),
        unread_url: articles_url(
            None,
            category_id,
            unfiled,
            tag.as_deref(),
            Some(true),
            "recent",
            None,
            false,
        ),
        relevance_url: articles_url(
            None,
            category_id,
            unfiled,
            tag.as_deref(),
            Some(false),
            "relevance",
            None,
            false,
        ),
        summary_url: articles_url(
            None,
            category_id,
            unfiled,
            tag.as_deref(),
            None,
            "recent",
            None,
            true,
        ),
        unseen_url: with_unseen(
            articles_url(
                feed_id,
                category_id,
                unfiled,
                tag.as_deref(),
                Some(true),
                "recent",
                None,
                false,
            ),
            true,
        ),
        unfiled_url: articles_url(
            None,
            None,
            true,
            None,
            Some(unread_only),
            sort,
            None,
            summary_only,
        ),
        scope_label,
        feed_filter: feed_id.map(|feed_id| feed_id.to_string()),
        category_filter: category_id.map(|category_id| category_id.to_string()),
        tag_filter: tag.clone(),
        unfiled_only: unfiled,
        unread_only,
        unseen_only,
        summary_only,
        reading_mode: prefs.reading_mode,
        side_panel,
        sort: sort.to_string(),
        back_url,
        has_prev: offset > 0,
        has_next,
        prev_url: with_unseen(
            articles_url(
                feed_id,
                category_id,
                unfiled,
                tag.as_deref(),
                Some(unread_only),
                sort,
                Some((offset - per_page).max(0)),
                summary_only,
            ),
            unseen_only,
        ),
        next_url: with_unseen(
            articles_url(
                feed_id,
                category_id,
                unfiled,
                tag.as_deref(),
                Some(unread_only),
                sort,
                Some(offset + per_page),
                summary_only,
            ),
            unseen_only,
        ),
        filtered_count,
        show_ai_key_cue,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;

    let mut response = (jar, body).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}

/// Recency-ordered page (newest first), fetching one extra row for `has_next`.
#[allow(clippy::too_many_arguments)]
async fn recency_page(
    state: &AppState,
    preferences: &prefs::Preferences,
    user_id: Uuid,
    filter: articles::LibraryFilter<'_>,
    unread_only: bool,
    summary_only: bool,
    per_page: i64,
    offset: i64,
) -> Result<(Vec<ArticleView>, bool, usize), AppError> {
    let mut items = articles::list_for_user(
        state.db.pool(),
        user_id,
        filter,
        unread_only,
        summary_only,
        (per_page * 5 + 1).min(501),
        offset,
    )
    .await?;
    let before = items.len();
    items.retain(article_passes_personal_filters);
    let filtered_count = before.saturating_sub(items.len());
    let has_next = items.len() as i64 > per_page || before as i64 > per_page * 5;
    items.truncate(per_page as usize);
    let views = items
        .into_iter()
        .map(|a| ArticleView {
            id: a.id.to_string(),
            feed_id: a.feed_id.to_string(),
            title: a.title,
            excerpt: a.excerpt,
            image_url: safe_external_url(a.image_url.as_deref()),
            feed_name: a.feed_name,
            published: a.published_at.map(|date| preferences.format_date(date)),
            is_read: a.is_read,
            is_seen: a.is_seen,
            is_bookmarked: a.is_bookmarked,
            has_summary: a.has_summary,
            story_count: a.story_count,
            can_summarize: state.settings.summarization_enabled && !a.has_summary,
            is_liked: a.feedback_value.is_some_and(|value| value > 0.0),
            is_disliked: a.feedback_value.is_some_and(|value| value < 0.0),
            score: None,
        })
        .collect();
    Ok((views, has_next, filtered_count))
}

/// Relevance-ranked page: score a recent candidate window against the user's
/// patterns (one batched weights lookup), sort by score, then paginate.
#[allow(clippy::too_many_arguments)]
async fn relevance_page(
    state: &AppState,
    preferences: &prefs::Preferences,
    user_id: Uuid,
    filter: articles::LibraryFilter<'_>,
    unread_only: bool,
    summary_only: bool,
    per_page: i64,
    offset: i64,
) -> Result<(Vec<ArticleView>, bool, usize), AppError> {
    let mut candidates = articles::list_for_user(
        state.db.pool(),
        user_id,
        filter,
        unread_only,
        summary_only,
        RELEVANCE_CANDIDATES,
        0,
    )
    .await?;
    let before = candidates.len();
    candidates.retain(article_passes_personal_filters);
    let filtered_count = before.saturating_sub(candidates.len());

    // Extract keywords per candidate (title + excerpt) and look up all weights
    // in a single query.
    let keywords: Vec<_> = candidates
        .iter()
        .map(|a| {
            let text = format!("{} {}", a.title, a.excerpt.as_deref().unwrap_or(""));
            extract_keywords(&text, 30)
        })
        .collect();
    let mut all_terms: Vec<String> = keywords
        .iter()
        .flat_map(|ks| ks.iter().map(|k| k.term.clone()))
        .collect();
    all_terms.sort();
    all_terms.dedup();
    let weights: std::collections::HashMap<String, f64> =
        patterns::weights_for(state.db.pool(), user_id, &all_terms)
            .await?
            .into_iter()
            .collect();

    let mut scored: Vec<(articles::ArticleListItem, f64)> = candidates
        .into_iter()
        .zip(keywords)
        .map(|(a, ks)| {
            let total: f64 = ks
                .iter()
                .filter_map(|k| weights.get(&k.term).map(|w| w * k.relevance))
                .sum();
            (a, relevance_score(total))
        })
        .collect();
    scored.sort_by(|x, y| {
        y.1.partial_cmp(&x.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| y.0.published_at.cmp(&x.0.published_at))
    });

    let start = offset as usize;
    let has_next = scored.len() > start + per_page as usize;
    let views = scored
        .into_iter()
        .skip(start)
        .take(per_page as usize)
        .map(|(a, score)| ArticleView {
            id: a.id.to_string(),
            feed_id: a.feed_id.to_string(),
            title: a.title,
            excerpt: a.excerpt,
            image_url: safe_external_url(a.image_url.as_deref()),
            feed_name: a.feed_name,
            published: a.published_at.map(|date| preferences.format_date(date)),
            is_read: a.is_read,
            is_seen: a.is_seen,
            is_bookmarked: a.is_bookmarked,
            has_summary: a.has_summary,
            story_count: a.story_count,
            can_summarize: state.settings.summarization_enabled && !a.has_summary,
            is_liked: a.feedback_value.is_some_and(|value| value > 0.0),
            is_disliked: a.feedback_value.is_some_and(|value| value < 0.0),
            score: Some((score.clamp(0.0, 1.0) * 100.0).round() as i64),
        })
        .collect();
    Ok((views, has_next, filtered_count))
}

fn article_passes_personal_filters(article: &articles::ArticleListItem) -> bool {
    let filters = rusty_feed_core::content_rules::entry_filters_from_json(
        article.subscription_settings.as_ref(),
    );
    rusty_feed_core::content_rules::entry_is_visible(
        &article.title,
        article.excerpt.as_deref().unwrap_or_default(),
        article.author.as_deref(),
        &article.url,
        &filters,
    )
    .unwrap_or(false)
}

/// `POST /articles/{id}/feedback` — explicit 👍/👎 or an implicit reading signal.
pub async fn feedback(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<FeedbackForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    authorize_action(&state, user.id, id).await?;
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;

    match form.value.as_deref() {
        Some("up") => {
            personalize::record_explicit(
                &state.db,
                &state.cache,
                user.id,
                &article,
                ExplicitFeedback::Up,
            )
            .await?;
        }
        Some("down") => {
            personalize::record_explicit(
                &state.db,
                &state.cache,
                user.id,
                &article,
                ExplicitFeedback::Down,
            )
            .await?;
        }
        _ => {
            if let Some(time_spent) = form.time_spent {
                let estimated = estimate_reading_seconds(&article.content);
                personalize::record_implicit(
                    &state.db,
                    &state.cache,
                    user.id,
                    &article,
                    time_spent,
                    estimated,
                    BOUNCE_THRESHOLD,
                )
                .await?;
            }
        }
    }
    Ok(action_response(is_ajax, form.next))
}

/// Estimate reading time (seconds) at ~200 wpm.
fn present_reader_html(html: &str, new_tab: bool, adhd: bool) -> String {
    let linked = autolink_bare_urls(html);
    let navigable = apply_external_link_navigation(&linked, new_tab);
    if adhd {
        apply_adhd_reading(&navigable)
    } else {
        navigable
    }
}

async fn note_article_opened(
    state: &AppState,
    user_id: Uuid,
    article_id: Uuid,
    auto_mark_read: bool,
) -> Result<(), AppError> {
    seen_articles::mark_seen(state.db.pool(), user_id, article_id).await?;
    if auto_mark_read {
        read_articles::mark_read(state.db.pool(), user_id, article_id).await?;
    }
    Ok(())
}

fn story_view(coverage: stories::ArticleCoverage) -> (Option<String>, Vec<StorySource>) {
    if coverage.sources.is_empty() {
        return (None, Vec::new());
    }
    let sources = coverage
        .sources
        .into_iter()
        .map(|source| StorySource {
            href: format!("/articles/{}", source.article_id),
            title: source.title,
            feed_name: source.feed_name,
        })
        .collect();
    (coverage.summary, sources)
}

fn estimate_reading_seconds(content: &str) -> f64 {
    let words = content.split_whitespace().count().max(1) as f64;
    words / 200.0 * 60.0
}

fn enclosure_mime_is_safe(kind: &str, mime_type: Option<&str>, enclosure_url: &str) -> bool {
    let inferred_extension = || {
        url::Url::parse(enclosure_url)
            .ok()
            .and_then(|url| {
                url.path_segments()
                    .and_then(Iterator::last)
                    .and_then(|name| {
                        name.rsplit_once('.')
                            .map(|(_, extension)| extension.to_owned())
                    })
            })
            .map(|extension| extension.to_ascii_lowercase())
    };
    let Some(mime_type) = mime_type else {
        return matches!(
            (kind, inferred_extension().as_deref()),
            (
                "audio",
                Some("aac" | "flac" | "m4a" | "mp3" | "oga" | "ogg" | "opus" | "wav")
            ) | ("video", Some("m4v" | "mov" | "mp4" | "ogv" | "webm"))
                | (
                    "image",
                    Some("avif" | "gif" | "jpeg" | "jpg" | "png" | "webp")
                )
        );
    };
    match kind {
        "audio" => matches!(
            mime_type,
            "audio/aac"
                | "audio/flac"
                | "audio/mp4"
                | "audio/mpeg"
                | "audio/ogg"
                | "audio/opus"
                | "audio/wav"
                | "audio/webm"
                | "audio/x-wav"
                | "application/ogg"
        ),
        "video" => matches!(
            mime_type,
            "video/mp4" | "video/ogg" | "video/quicktime" | "video/webm"
        ),
        "image" => matches!(
            mime_type,
            "image/avif" | "image/gif" | "image/jpeg" | "image/png" | "image/webp"
        ),
        _ => false,
    }
}

fn enclosure_size(size_bytes: i64) -> String {
    const KIB: f64 = 1_024.0;
    const MIB: f64 = KIB * 1_024.0;
    const GIB: f64 = MIB * 1_024.0;
    let bytes = size_bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KB", bytes / KIB)
    } else {
        format!("{size_bytes} B")
    }
}

fn enclosure_duration(seconds: i64) -> String {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn enclosure_views(
    article_id: Uuid,
    stored: Vec<rusty_feed_db::ArticleEnclosure>,
    preferences: &prefs::Preferences,
    media_proxy_enabled: bool,
    delivery_mode: &str,
    load_mode: &str,
    extra_inline_mime_types: &[String],
) -> Vec<EnclosureView> {
    stored
        .into_iter()
        .filter_map(|enclosure| {
            let publisher_url = safe_external_url(Some(&enclosure.url))?;
            let proxied = media_proxy_enabled
                && match delivery_mode {
                    "proxy" => true,
                    "mixed-content" => publisher_url.starts_with("http://"),
                    _ => false,
                };
            let position = enclosure.position;
            let url = if proxied {
                format!("/articles/{article_id}/media/{position}")
            } else {
                publisher_url.clone()
            };
            let open_url = if proxied {
                format!("/articles/{article_id}/media/{position}?download=1")
            } else {
                publisher_url.clone()
            };
            let kind_allowed = preferences
                .enclosure_player_kinds
                .iter()
                .any(|kind| kind == &enclosure.media_kind);
            let mime_is_safe = enclosure_mime_is_safe(
                &enclosure.media_kind,
                enclosure.mime_type.as_deref(),
                &enclosure.url,
            ) || enclosure.mime_type.as_deref().is_some_and(|value| {
                value.starts_with(&format!("{}/", enclosure.media_kind))
                    && extra_inline_mime_types.iter().any(|extra| extra == value)
            });
            let can_embed = load_mode != "links-only" && kind_allowed && mime_is_safe;
            let fallback_title = match enclosure.media_kind.as_str() {
                "audio" => "Audio attachment",
                "video" => "Video attachment",
                "image" => "Image attachment",
                _ => "File attachment",
            };
            let mut details = Vec::new();
            if let Some(duration) = enclosure.duration_seconds {
                details.push(enclosure_duration(duration));
            }
            if let Some(size) = enclosure.size_bytes {
                details.push(enclosure_size(size));
            }
            if let Some(mime_type) = enclosure.mime_type.as_deref() {
                details.push(mime_type.to_string());
            }
            Some(EnclosureView {
                title: enclosure
                    .title
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| fallback_title.to_string()),
                url,
                open_url,
                details: (!details.is_empty()).then(|| details.join(" · ")),
                is_audio: enclosure.media_kind == "audio",
                is_video: enclosure.media_kind == "video",
                is_image: enclosure.media_kind == "image",
                can_embed,
                load_on_demand: load_mode == "on-demand",
                show_direct_link: preferences.enclosure_show_links || !can_embed,
                publisher_url,
                show_publisher_link: proxied && preferences.enclosure_show_links,
                proxied,
            })
        })
        .collect()
}

fn reader_image_url(
    article_id: Uuid,
    raw_url: Option<&str>,
    media_proxy_enabled: bool,
    delivery_mode: &str,
) -> Option<String> {
    let publisher_url = safe_external_url(raw_url)?;
    let proxied = media_proxy_enabled
        && match delivery_mode {
            "proxy" => true,
            "mixed-content" => publisher_url.starts_with("http://"),
            _ => false,
        };
    Some(if proxied {
        format!("/articles/{article_id}/hero")
    } else {
        publisher_url
    })
}

fn youtube_views(
    videos: Vec<YouTubeVideo>,
    article_title: &str,
    allow_player: bool,
    provider: &str,
    preferences: &prefs::Preferences,
    policy: &YouTubePolicy,
    durations: &HashMap<String, i32>,
) -> Vec<YouTubeView> {
    let multiple = videos.len() > 1;
    videos
        .into_iter()
        .enumerate()
        .map(|(index, video)| {
            let start_seconds = video.start_seconds;
            YouTubeView {
                title: if multiple {
                    format!("YouTube video {} — {article_title}", index + 1)
                } else {
                    format!("YouTube video — {article_title}")
                },
                player_name: if provider == "alternative" {
                    policy
                        .alternative_name()
                        .unwrap_or("Alternative player")
                        .to_string()
                } else {
                    "YouTube privacy-enhanced player".to_string()
                },
                watch_url: video.watch_url,
                start_label: start_seconds.map(format_timestamp),
                duration_label: durations.get(&video.id).copied().map(format_duration),
                embed_url: if allow_player {
                    policy.embed_url_for(
                        provider,
                        &video.id,
                        start_seconds,
                        preferences.youtube_autoplay,
                        &preferences.youtube_captions,
                    )
                } else {
                    None
                },
                autoplay: preferences.youtube_autoplay,
                referrer_policy: if provider == "alternative" {
                    "no-referrer".to_string()
                } else {
                    "strict-origin-when-cross-origin".to_string()
                },
            }
        })
        .collect()
}

fn format_duration(seconds: i32) -> String {
    let seconds = seconds.max(0) as u32;
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("Duration {hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("Duration {minutes}:{seconds:02}")
    }
}

fn format_timestamp(seconds: u32) -> String {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("Starts at {hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("Starts at {minutes}:{seconds:02}")
    }
}

/// Reject an on-demand summary that would exceed the AI budget/quota (FND-187).
///
/// Mirrors the enforcement inside the cascade, but runs *before* the job is
/// enqueued so the user gets an immediate, friendly 429 and no work is queued.
async fn summary_budget_gate(state: &AppState, user_id: Uuid) -> Result<(), AppError> {
    let budget = &state.settings.ai_budget;
    let since = AiBudget::today_start();

    if budget.has_per_user_llm_call_cap() {
        let calls =
            cost_ledger::user_call_count_since(state.db.pool(), user_id, "summary", since).await?;
        if calls >= i64::from(budget.llm_per_user_daily_calls) {
            return Err(AppError::RateLimited);
        }
    }

    // BYO-key users spend their own quota and are exempt from the USD budgets.
    if llm_profiles::has_enabled_user_profile(state.db.pool(), user_id).await? {
        return Ok(());
    }
    if budget.has_global_llm_cap() {
        let spent = cost_ledger::instance_spend_micro_usd_since(state.db.pool(), since).await?;
        if spent >= budget.llm_global_daily_micro_usd {
            return Err(AppError::RateLimited);
        }
    }
    if budget.has_per_user_llm_usd_cap() {
        let spent =
            cost_ledger::user_spend_micro_usd_since(state.db.pool(), user_id, since).await?;
        if spent >= budget.llm_per_user_daily_micro_usd {
            return Err(AppError::RateLimited);
        }
    }
    Ok(())
}

/// `POST /articles/{id}/summarize` — enqueue a (deduped) summary job.
pub async fn summarize(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ActionForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !state.settings.summarization_enabled {
        return Err(AppError::Validation(
            "AI summaries are not enabled for this Rusty Feed instance.".to_string(),
        ));
    }
    if !state.summarize_limiter.check(&user.id.to_string()).await {
        return Err(AppError::RateLimited);
    }
    crate::verify::deny_demo(&user)?;
    crate::verify::require_verified(&state, &user)?;
    // Enforce the AI budget/quota *before* enqueuing so a stranger cannot fan
    // out thousands of paid jobs (FND-187). BYO-key users are exempt from the
    // instance USD budget; the daily call cap applies to everyone.
    summary_budget_gate(&state, user.id).await?;
    authorize_action(&state, user.id, id).await?;
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let dedup = summary_dedup(id, article.content_revision);
    jobs::enqueue_in_lane(
        state.db.pool(),
        jobs::QueueLane::Interactive,
        "summarize_article",
        serde_json::json!({
            "article_id": id,
            "content_revision": article.content_revision,
            "user_id": user.id
        }),
        Some(&dedup),
        None,
        3,
    )
    .await?;
    Ok(action_response(is_ajax, form.next))
}

#[derive(Serialize)]
pub struct SummaryStatus {
    state: &'static str,
    message: &'static str,
}

/// `GET /articles/{id}/summary/status` — safe, live progress for a shared
/// revision-scoped summary job. Operational error details remain admin-only.
pub async fn summary_status(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<axum::Json<SummaryStatus>, AppError> {
    authorize_action(&state, user.id, id).await?;
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    if article.summarized_at.is_some() {
        return Ok(axum::Json(SummaryStatus {
            state: "ready",
            message: "AI summary ready.",
        }));
    }
    let dedup = summary_dedup(id, article.content_revision);
    let status = jobs::latest_by_dedup(state.db.pool(), &dedup).await?;
    let response = match status {
        Some(job) if job.status == "running" => SummaryStatus {
            state: "running",
            message: "Generating the AI summary now…",
        },
        Some(job) if job.status == "pending" && job.attempts > 0 => SummaryStatus {
            state: "retrying",
            message: "The first attempt failed. Retrying in the priority queue…",
        },
        Some(job) if job.status == "pending" => SummaryStatus {
            state: "queued",
            message: "Waiting for a priority worker…",
        },
        Some(job) if job.status == "dead" => SummaryStatus {
            state: "failed",
            message: "The summary could not be generated. An administrator can see the error details.",
        },
        Some(_) => SummaryStatus {
            state: "failed",
            message: "The summary finished without usable content. An administrator can see the run details.",
        },
        None => SummaryStatus {
            state: "idle",
            message: "Ready to generate an AI summary.",
        },
    };
    Ok(axum::Json(response))
}

fn summary_dedup(id: Uuid, content_revision: i64) -> String {
    format!("summarize:{id}:{content_revision}")
}

async fn reader_view(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
    preferences: &prefs::Preferences,
    requested_mode: Option<SubscriptionContentMode>,
) -> Result<ReaderView, AppError> {
    if !articles::user_can_access(state.db.pool(), user_id, id).await? {
        return Err(AppError::NotFound);
    }
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    note_article_opened(state, user_id, id, preferences.auto_mark_read).await?;

    let feed = feeds::find_by_id(state.db.pool(), article.feed_id).await?;
    let subscription = subscriptions::find(state.db.pool(), user_id, article.feed_id).await?;
    let feed_name = subscription
        .as_ref()
        .and_then(|subscription| subscription.custom_name.clone())
        .filter(|name| !name.trim().is_empty())
        .or_else(|| feed.as_ref().map(|feed| feed.name.clone()))
        .unwrap_or_default();
    let subscription_content = subscription
        .as_ref()
        .map(|subscription| SubscriptionContentSettings::from_json(subscription.settings.as_ref()))
        .unwrap_or_default();
    let canonical_url = state.settings.tracking_url_policy.clean(&article.url);
    let clean_url = safe_external_url(Some(&canonical_url));
    let content_mode = requested_mode.unwrap_or(subscription_content.mode);
    let temporary = content_mode != subscription_content.mode;
    let (content, content_notice, content_availability, stored_blocked_tracking_pixels) =
        selected_content(state, &article, content_mode, temporary).await?;
    let content_report = sanitize_article_html_with_policies(
        &content,
        &article.url,
        &state.settings.tracking_url_policy,
        &state.settings.tracking_pixel_policy,
    );
    let blocked_tracking_pixels =
        stored_blocked_tracking_pixels.saturating_add(content_report.blocked_tracking_pixels);
    let stored_summary = articles::get_summary(state.db.pool(), id).await?;
    let can_summarize = state.settings.summarization_enabled && stored_summary.is_none();
    let (summary, key_points, topics) = match stored_summary {
        Some(s) => (Some(s.summary), s.key_points, s.topics),
        None => (None, Vec::new(), Vec::new()),
    };
    let feedback_value = feedback::get(state.db.pool(), user_id, id)
        .await?
        .map(|record| record.feedback_value);

    let original_url = safe_external_url(Some(&article.original_url));
    let original_differs = original_url != clean_url;
    let prefer_original = preferences.external_link_mode == "original";
    let external_new_tab = state.settings.external_links_new_tab_required
        || preferences.external_open_mode == "new-tab";
    let (url, url_label, alternate_url, alternate_url_label) = if prefer_original {
        (
            original_url.clone().or_else(|| clean_url.clone()),
            "Open publisher URL".to_string(),
            original_differs.then(|| clean_url.clone()).flatten(),
            "Privacy-clean URL".to_string(),
        )
    } else {
        (
            clean_url.clone().or_else(|| original_url.clone()),
            "Open clean link".to_string(),
            original_differs.then(|| original_url.clone()).flatten(),
            "Publisher URL".to_string(),
        )
    };
    let live_view_url = clean_url
        .as_ref()
        .map(|_| format!("/articles/{id}/live-ticket"));
    let stored_enclosures = articles::list_enclosures(state.db.pool(), id).await?;
    let mut youtube_urls = vec![article.url.as_str(), article.original_url.as_str()];
    youtube_urls.extend(
        stored_enclosures
            .iter()
            .map(|enclosure| enclosure.url.as_str()),
    );
    let youtube_mode = subscription
        .as_ref()
        .map(|subscription| SubscriptionYouTubeMode::from_json(subscription.settings.as_ref()))
        .unwrap_or_default();
    let enclosure_settings = subscription
        .as_ref()
        .map(|subscription| {
            SubscriptionEnclosureSettings::from_json(subscription.settings.as_ref())
        })
        .unwrap_or_default();
    let enclosure_delivery_mode = match enclosure_settings.delivery {
        SubscriptionEnclosureDelivery::Inherit => preferences.enclosure_delivery_mode.as_str(),
        SubscriptionEnclosureDelivery::Proxy => "proxy",
        SubscriptionEnclosureDelivery::MixedContent => "mixed-content",
        SubscriptionEnclosureDelivery::Direct => "direct",
    };
    let enclosure_load_mode = match enclosure_settings.load {
        SubscriptionEnclosureLoad::Inherit => preferences.enclosure_load_mode.as_str(),
        SubscriptionEnclosureLoad::OnDemand => "on-demand",
        SubscriptionEnclosureLoad::Metadata => "metadata",
        SubscriptionEnclosureLoad::LinksOnly => "links-only",
    };
    let preferred_youtube_provider = if preferences.youtube_player_provider == "alternative"
        && state.settings.youtube_policy.alternative_name().is_some()
    {
        "alternative"
    } else {
        "privacy"
    };
    let (allow_youtube_player, youtube_provider) = match youtube_mode {
        SubscriptionYouTubeMode::Inherit => (
            preferences.youtube_player_mode == "privacy",
            preferred_youtube_provider,
        ),
        SubscriptionYouTubeMode::Privacy => (true, "privacy"),
        SubscriptionYouTubeMode::LinksOnly => (false, "privacy"),
    };
    let mut youtube_detection_html = content_report.html.clone();
    if let Some(feed_content) = article.feed_content.as_deref() {
        youtube_detection_html.push_str(feed_content);
    }
    if let Some(article_page_content) = article.article_page_content.as_deref() {
        youtube_detection_html.push_str(article_page_content);
    }
    let detected_youtube_videos = state
        .settings
        .youtube_policy
        .videos(youtube_urls, &youtube_detection_html);
    let video_ids: Vec<_> = detected_youtube_videos
        .iter()
        .map(|video| video.id.clone())
        .collect();
    let metadata = youtube_metadata::list(state.db.pool(), &video_ids).await?;
    let now = OffsetDateTime::now_utc();
    let durations: HashMap<_, _> = metadata
        .iter()
        .filter_map(|row| {
            (row.status == "ready" && row.expires_at > now)
                .then(|| {
                    row.duration_secs
                        .map(|duration| (row.video_id.clone(), duration))
                })
                .flatten()
        })
        .collect();
    if state.settings.youtube_duration_enabled {
        let known: HashMap<_, _> = metadata
            .into_iter()
            .map(|row| (row.video_id, row.expires_at))
            .collect();
        for video_id in &video_ids {
            if known
                .get(video_id)
                .is_none_or(|expires_at| *expires_at <= now)
            {
                youtube_metadata::reserve(state.db.pool(), video_id).await?;
                let _ = jobs::enqueue(
                    state.db.pool(),
                    "youtube_duration",
                    serde_json::json!({"video_id": video_id}),
                    Some(&format!("youtube_duration:{video_id}")),
                    None,
                    3,
                )
                .await?;
            }
        }
    }
    let youtube_videos = youtube_views(
        detected_youtube_videos,
        &article.title,
        allow_youtube_player,
        youtube_provider,
        preferences,
        &state.settings.youtube_policy,
        &durations,
    );
    let enclosures = enclosure_views(
        id,
        stored_enclosures,
        preferences,
        state.settings.media_proxy_enabled,
        enclosure_delivery_mode,
        enclosure_load_mode,
        &state.settings.media_proxy_extra_inline_mime_types,
    );
    let default_share_source = if subscription_content.mode == SubscriptionContentMode::ArticlePage
    {
        "article_page"
    } else {
        "rss"
    };
    let share =
        public_shares::controls(state, user_id, id, &article.title, default_share_source).await?;
    let (story_summary, story_sources) =
        story_view(stories::coverage_for_user(state.db.pool(), user_id, id).await?);
    Ok(ReaderView {
        id: id.to_string(),
        title: article.title,
        feed_name,
        image_url: reader_image_url(
            id,
            article.image_url.as_deref(),
            state.settings.media_proxy_enabled,
            enclosure_delivery_mode,
        ),
        enclosures,
        youtube_videos,
        url,
        url_label,
        alternate_url,
        alternate_url_label,
        live_view_url,
        content_mode: content_mode.as_str().to_string(),
        default_content_mode: subscription_content.mode.as_str().to_string(),
        content_mode_return_url: format!("/articles/{id}"),
        content_mode_panel_url: format!("/articles/{id}/panel"),
        comments_url: article
            .comments_url
            .as_deref()
            .map(|url| state.settings.tracking_url_policy.clean(url))
            .and_then(|url| safe_external_url(Some(&url)))
            .filter(|comments_url| comments_url != &canonical_url),
        published: article
            .published_at
            .map(|date| preferences.format_date(date)),
        content: present_reader_html(
            &content_report.html,
            external_new_tab,
            preferences.reading_format == "adhd",
        ),
        content_notice,
        content_pending: content_availability == ContentAvailability::Pending,
        blocked_tracking_pixels: (preferences.show_blocked_tracking_notice
            && blocked_tracking_pixels > 0)
            .then_some(blocked_tracking_pixels),
        is_read: read_articles::is_read(state.db.pool(), user_id, id).await?,
        is_bookmarked: bookmarks::is_bookmarked(state.db.pool(), user_id, id).await?,
        is_liked: feedback_value.is_some_and(|value| value > 0.0),
        is_disliked: feedback_value.is_some_and(|value| value < 0.0),
        summary,
        key_points,
        topics,
        can_summarize,
        story_summary,
        story_sources,
        external_new_tab,
        share,
    })
}

/// `GET /articles/{id}` — standalone reading view.
pub async fn read(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Query(query): Query<ContentViewQuery>,
) -> Result<Response, AppError> {
    let prefs = prefs::load(&state.db, user.id).await;
    let is_admin = user.role == "admin";
    let article = reader_view(&state, user.id, id, &prefs, query.mode()).await?;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = ArticleTemplate {
        theme: prefs.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui: prefs,
        is_admin,
        id: article.id,
        title: article.title,
        feed_name: article.feed_name,
        url: article.url,
        url_label: article.url_label,
        alternate_url: article.alternate_url,
        alternate_url_label: article.alternate_url_label,
        live_view_url: article.live_view_url,
        content_mode: article.content_mode,
        default_content_mode: article.default_content_mode,
        content_mode_return_url: article.content_mode_return_url,
        content_mode_panel_url: article.content_mode_panel_url,
        comments_url: article.comments_url,
        published: article.published,
        image_url: article.image_url,
        enclosures: article.enclosures,
        youtube_videos: article.youtube_videos,
        content: article.content,
        content_notice: article.content_notice,
        content_pending: article.content_pending,
        blocked_tracking_pixels: article.blocked_tracking_pixels,
        is_read: article.is_read,
        is_bookmarked: article.is_bookmarked,
        is_liked: article.is_liked,
        is_disliked: article.is_disliked,
        summary: article.summary,
        key_points: article.key_points,
        topics: article.topics,
        can_summarize: article.can_summarize,
        story_summary: article.story_summary,
        story_sources: article.story_sources,
        external_new_tab: article.external_new_tab,
        share: article.share,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    let mut response = (jar, body).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}

pub async fn side_panel(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Query(query): Query<ContentViewQuery>,
) -> Result<Response, AppError> {
    let prefs = prefs::load(&state.db, user.id).await;
    let article = reader_view(&state, user.id, id, &prefs, query.mode()).await?;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = ArticlePanelTemplate {
        csrf_token,
        ui: prefs,
        id: article.id,
        title: article.title,
        feed_name: article.feed_name,
        url: article.url,
        url_label: article.url_label,
        alternate_url: article.alternate_url,
        alternate_url_label: article.alternate_url_label,
        live_view_url: article.live_view_url,
        content_mode: article.content_mode,
        default_content_mode: article.default_content_mode,
        content_mode_return_url: article.content_mode_return_url,
        content_mode_panel_url: article.content_mode_panel_url,
        comments_url: article.comments_url,
        published: article.published,
        image_url: article.image_url,
        enclosures: article.enclosures,
        youtube_videos: article.youtube_videos,
        content: article.content,
        content_notice: article.content_notice,
        content_pending: article.content_pending,
        blocked_tracking_pixels: article.blocked_tracking_pixels,
        is_read: article.is_read,
        is_bookmarked: article.is_bookmarked,
        is_liked: article.is_liked,
        is_disliked: article.is_disliked,
        summary: article.summary,
        key_points: article.key_points,
        topics: article.topics,
        can_summarize: article.can_summarize,
        story_summary: article.story_summary,
        story_sources: article.story_sources,
        external_new_tab: article.external_new_tab,
        share: article.share,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, body).into_response())
}

/// `GET /articles/{id}/content` — sanitized content fragment (for inline mode).
pub async fn content_fragment(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    Query(query): Query<ContentViewQuery>,
) -> Result<Response, AppError> {
    if !articles::user_can_access(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let prefs = prefs::load(&state.db, user.id).await;
    note_article_opened(&state, user.id, id, prefs.auto_mark_read).await?;
    let settings =
        subscriptions::content_settings_for_user(state.db.pool(), user.id, article.feed_id)
            .await?
            .unwrap_or_default();
    let content_mode = query.mode().unwrap_or(settings.mode);
    let temporary = content_mode != settings.mode;
    let (content, _, availability, stored_blocked_tracking_pixels) =
        selected_content(&state, &article, content_mode, temporary).await?;
    let report = sanitize_article_html_with_policies(
        &content,
        &article.url,
        &state.settings.tracking_url_policy,
        &state.settings.tracking_pixel_policy,
    );
    let blocked_tracking_pixels =
        stored_blocked_tracking_pixels.saturating_add(report.blocked_tracking_pixels);
    let notice = if prefs.show_blocked_tracking_notice && blocked_tracking_pixels > 0 {
        format!(
            "<p class=\"content-notice privacy-notice\">Tracking images removed by Rusty Feed: {blocked_tracking_pixels}.</p>"
        )
    } else {
        String::new()
    };
    let external_new_tab =
        state.settings.external_links_new_tab_required || prefs.external_open_mode == "new-tab";
    let content = present_reader_html(
        &report.html,
        external_new_tab,
        prefs.reading_format == "adhd",
    );
    Ok((
        [("x-rusty-feed-content-status", availability.as_str())],
        Html(format!("{notice}{content}")),
    )
        .into_response())
}

pub async fn set_content_mode(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ContentModeForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    authorize_action(&state, user.id, id).await?;
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let mode = SubscriptionContentMode::parse(&form.content_mode).ok_or_else(|| {
        AppError::Validation("Choose RSS, page content, or Live View.".to_string())
    })?;
    if mode == SubscriptionContentMode::Live && safe_external_url(Some(&article.url)).is_none() {
        return Err(AppError::Validation(
            "This feed item has no public page for Live View.".to_string(),
        ));
    }
    let settings =
        subscriptions::content_settings_for_user(state.db.pool(), user.id, article.feed_id)
            .await?
            .ok_or(AppError::NotFound)?;
    subscriptions::update_content_settings(
        state.db.pool(),
        user.id,
        article.feed_id,
        SubscriptionContentSettings {
            mode,
            lifetime: settings.lifetime,
        },
    )
    .await?;
    let shared_lifetime =
        subscriptions::requested_article_page_lifetime(state.db.pool(), article.feed_id).await?;
    articles::apply_article_page_lifetime(state.db.pool(), article.feed_id, shared_lifetime)
        .await?;
    if mode == SubscriptionContentMode::ArticlePage {
        if !article_page_is_current(&article) {
            enqueue_article_page_on_access(&state, &article, false).await?;
        }
        crate::feeds::queue_page_extractions(&state, article.feed_id).await?;
    }
    Ok(action_response(is_ajax, form.next))
}

async fn authorize_action(state: &AppState, user_id: Uuid, id: Uuid) -> Result<(), AppError> {
    if articles::user_can_access(state.db.pool(), user_id, id).await? {
        Ok(())
    } else {
        Err(AppError::NotFound)
    }
}

/// Actions return `204` to our fetch client (which updates the UI in place) and
/// a redirect for plain form posts (no-JS).
fn action_response(is_ajax: bool, next: Option<String>) -> Response {
    if is_ajax {
        axum::http::StatusCode::NO_CONTENT.into_response()
    } else {
        Redirect::to(&safe_next(next)).into_response()
    }
}

/// `POST /articles/{id}/read`.
pub async fn mark_read(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ActionForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    authorize_action(&state, user.id, id).await?;
    read_articles::mark_read(state.db.pool(), user.id, id).await?;
    Ok(action_response(is_ajax, form.next))
}

/// `POST /articles/{id}/unread`.
pub async fn mark_unread(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ActionForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    authorize_action(&state, user.id, id).await?;
    read_articles::mark_unread(state.db.pool(), user.id, id).await?;
    Ok(action_response(is_ajax, form.next))
}

/// `POST /articles/{id}/bookmark`.
pub async fn bookmark(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ActionForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    authorize_action(&state, user.id, id).await?;
    bookmarks::add(state.db.pool(), user.id, id).await?;
    crate::integrations::dispatch_event(&state, user.id, id, "article.bookmarked").await?;
    Ok(action_response(is_ajax, form.next))
}

/// `POST /articles/{id}/unbookmark`.
pub async fn unbookmark(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ActionForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    authorize_action(&state, user.id, id).await?;
    bookmarks::remove(state.db.pool(), user.id, id).await?;
    Ok(action_response(is_ajax, form.next))
}

/// `POST /articles/read-all`.
pub async fn mark_all_read(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Form(form): Form<MarkAllReadForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let unfiled = form.unfiled.as_deref() == Some("1");
    let category_id = if unfiled { None } else { form.category };
    if let Some(category_id) = category_id {
        if !categories::belongs_to_user(state.db.pool(), user.id, category_id).await? {
            return Err(AppError::NotFound);
        }
    }
    let tag = normalized_tag(form.tag).map_err(AppError::Validation)?;
    read_articles::mark_all_read(
        state.db.pool(),
        user.id,
        form.feed,
        category_id,
        unfiled,
        tag.as_deref(),
    )
    .await?;
    Ok(action_response(is_ajax, form.next))
}

#[cfg(test)]
mod tests {
    use super::{
        articles_url, enclosure_mime_is_safe, format_timestamp, reader_image_url, safe_external_url,
    };
    use uuid::Uuid;

    #[test]
    fn article_urls_encode_read_filter_overrides() {
        assert_eq!(
            articles_url(None, None, false, None, None, "recent", None, false),
            "/articles"
        );
        assert_eq!(
            articles_url(None, None, false, None, Some(true), "recent", None, false),
            "/articles?unread=1"
        );
        assert_eq!(
            articles_url(None, None, false, None, Some(false), "recent", None, false),
            "/articles?unread=0"
        );
    }

    #[test]
    fn external_urls_reject_unsafe_origins_and_strip_fragments() {
        assert_eq!(
            safe_external_url(Some("https://example.com/article#section")).as_deref(),
            Some("https://example.com/article")
        );
        for url in [
            "javascript:alert(1)",
            "data:text/html,hello",
            "file:///etc/passwd",
            "https://user:password@example.com/article",
            "https://localhost/article",
            "https://reader.localhost/article",
            "https://127.0.0.1/article",
            "https://[::1]/article",
            "https://169.254.169.254/latest/meta-data",
        ] {
            assert!(
                safe_external_url(Some(url)).is_none(),
                "{url} should be rejected"
            );
        }
    }

    #[test]
    fn inline_enclosures_use_a_narrow_passive_media_allow_list() {
        assert!(enclosure_mime_is_safe(
            "audio",
            Some("audio/mpeg"),
            "https://cdn.example/play"
        ));
        assert!(enclosure_mime_is_safe(
            "video",
            None,
            "https://cdn.example/trailer.webm?token=1"
        ));
        assert!(!enclosure_mime_is_safe(
            "image",
            Some("image/svg+xml"),
            "https://cdn.example/vector.svg"
        ));
        assert!(!enclosure_mime_is_safe(
            "audio",
            None,
            "https://cdn.example/download"
        ));
    }

    #[test]
    fn header_images_follow_the_effective_media_privacy_mode() {
        let article_id = Uuid::now_v7();
        let publisher = "https://images.example.test/hero.jpg";
        assert_eq!(
            reader_image_url(article_id, Some(publisher), true, "proxy"),
            Some(format!("/articles/{article_id}/hero"))
        );
        assert_eq!(
            reader_image_url(article_id, Some(publisher), true, "mixed-content").as_deref(),
            Some(publisher)
        );
        assert_eq!(
            reader_image_url(
                article_id,
                Some("http://images.example.test/hero.jpg"),
                true,
                "mixed-content"
            ),
            Some(format!("/articles/{article_id}/hero"))
        );
        assert_eq!(
            reader_image_url(article_id, Some(publisher), true, "direct").as_deref(),
            Some(publisher)
        );
        assert!(reader_image_url(
            article_id,
            Some("http://127.0.0.1/private.jpg"),
            true,
            "proxy"
        )
        .is_none());
    }

    #[test]
    fn youtube_timestamp_labels_are_compact_and_unambiguous() {
        assert_eq!(format_timestamp(42), "Starts at 0:42");
        assert_eq!(format_timestamp(62), "Starts at 1:02");
        assert_eq!(format_timestamp(3_723), "Starts at 1:02:03");
    }
}
