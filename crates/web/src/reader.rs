//! The reading experience: article list, per-article actions (read/bookmark),
//! and article reading (side panel, standalone page, and inline fragment).

use std::collections::{BTreeMap, HashMap, HashSet};

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use rustss_core::feed_content::{FeedContentMode, FeedContentSettings};
use rustss_core::html::sanitize_article_html;
use rustss_db::{
    articles, bookmarks, categories, feed_icons, feedback, feeds, jobs, patterns, read_articles,
    subscriptions,
};
use rustss_search::personalize;
use rustss_search::{extract_keywords, relevance_score, ExplicitFeedback};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::extract::Ajax;
use crate::prefs;
use crate::state::AppState;

/// Candidate window scored when sorting by relevance.
const RELEVANCE_CANDIDATES: i64 = 120;
/// Default implicit bounce threshold (ARCH §11.2).
const BOUNCE_THRESHOLD: f64 = 0.25;

struct ArticleView {
    id: String,
    title: String,
    excerpt: Option<String>,
    image_url: Option<String>,
    feed_name: String,
    published: Option<String>,
    is_read: bool,
    is_bookmarked: bool,
    is_liked: bool,
    is_disliked: bool,
    score: Option<i64>,
}

struct FeedNav {
    id: String,
    name: String,
    initial: String,
    has_icon: bool,
    active: bool,
    href: String,
}

struct CategoryNav {
    name: String,
    indent: String,
    feed_count: i64,
    active: bool,
    href: String,
}

struct TagNav {
    name: String,
    feed_count: i64,
    active: bool,
    href: String,
}

struct CategoryNavContext<'a> {
    selected: Option<Uuid>,
    tag: Option<&'a str>,
    unread: bool,
    sort: &'a str,
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
    feeds: Vec<FeedNav>,
    categories: Vec<CategoryNav>,
    tags: Vec<TagNav>,
    recent_url: String,
    unread_url: String,
    relevance_url: String,
    scope_label: String,
    feed_filter: Option<String>,
    category_filter: Option<String>,
    tag_filter: Option<String>,
    unread_only: bool,
    reading_mode: String,
    side_panel: bool,
    sort: String,
    back_url: String,
    has_prev: bool,
    has_next: bool,
    prev_url: String,
    next_url: String,
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
    url: String,
    published: Option<String>,
    image_url: Option<String>,
    content: String, // sanitized HTML
    content_notice: Option<String>,
    is_read: bool,
    is_bookmarked: bool,
    is_liked: bool,
    is_disliked: bool,
    summary: Option<String>,
    key_points: Vec<String>,
    topics: Vec<String>,
    can_summarize: bool,
}

#[derive(Template)]
#[template(path = "article_panel.html")]
struct ArticlePanelTemplate {
    csrf_token: String,
    id: String,
    title: String,
    feed_name: String,
    url: String,
    published: Option<String>,
    image_url: Option<String>,
    content: String,
    content_notice: Option<String>,
    is_read: bool,
    is_bookmarked: bool,
    is_liked: bool,
    is_disliked: bool,
    summary: Option<String>,
    key_points: Vec<String>,
    topics: Vec<String>,
    can_summarize: bool,
}

struct ReaderView {
    id: String,
    title: String,
    feed_name: String,
    url: String,
    published: Option<String>,
    image_url: Option<String>,
    content: String,
    content_notice: Option<String>,
    is_read: bool,
    is_bookmarked: bool,
    is_liked: bool,
    is_disliked: bool,
    summary: Option<String>,
    key_points: Vec<String>,
    topics: Vec<String>,
    can_summarize: bool,
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
    /// `recent` (default) or `relevance`.
    #[serde(default)]
    sort: Option<String>,
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
}

/// Only allow same-origin relative redirects (avoid open-redirects).
fn safe_next(next: Option<String>) -> String {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") => n,
        _ => "/articles".to_string(),
    }
}

fn safe_media_url(value: Option<&str>) -> Option<String> {
    let value = value?;
    let url = url::Url::parse(value).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
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
    let tag = tag.trim().to_lowercase();
    if tag.is_empty() || tag.chars().count() > 40 || tag.chars().any(char::is_control) {
        return Err("Tag filters must be between 1 and 40 printable characters.".to_string());
    }
    Ok(Some(tag))
}

fn articles_url(
    feed_id: Option<Uuid>,
    category_id: Option<Uuid>,
    tag: Option<&str>,
    unread: bool,
    sort: &str,
    offset: Option<i64>,
) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    if unread {
        query.append_pair("unread", "1");
    }
    if let Some(feed_id) = feed_id {
        query.append_pair("feed", &feed_id.to_string());
    }
    if let Some(category_id) = category_id {
        query.append_pair("category", &category_id.to_string());
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
    let query = query.finish();
    if query.is_empty() {
        "/articles".to_string()
    } else {
        format!("/articles?{query}")
    }
}

fn append_category_nav(
    parent_id: Option<Uuid>,
    depth: usize,
    children: &mut BTreeMap<Option<Uuid>, Vec<categories::CategoryWithFeedCount>>,
    seen: &mut HashSet<Uuid>,
    output: &mut Vec<CategoryNav>,
    context: &CategoryNavContext<'_>,
) {
    let Some(categories) = children.remove(&parent_id) else {
        return;
    };
    for category in categories {
        if !seen.insert(category.id) {
            continue;
        }
        output.push(CategoryNav {
            name: category.name,
            indent: "↳ ".repeat(depth),
            feed_count: category.feed_count,
            active: Some(category.id) == context.selected,
            href: articles_url(
                None,
                Some(category.id),
                context.tag,
                context.unread,
                context.sort,
                None,
            ),
        });
        append_category_nav(
            Some(category.id),
            depth + 1,
            children,
            seen,
            output,
            context,
        );
    }
}

fn category_scope_ids(
    categories: &[categories::CategoryWithFeedCount],
    selected: Option<Uuid>,
) -> HashSet<Uuid> {
    let Some(selected) = selected else {
        return HashSet::new();
    };
    let mut children = HashMap::<Uuid, Vec<Uuid>>::new();
    for category in categories {
        if let Some(parent_id) = category.parent_id {
            children.entry(parent_id).or_default().push(category.id);
        }
    }
    let mut scope = HashSet::from([selected]);
    let mut frontier = vec![selected];
    while let Some(parent_id) = frontier.pop() {
        for child_id in children.get(&parent_id).into_iter().flatten() {
            if scope.insert(*child_id) {
                frontier.push(*child_id);
            }
        }
    }
    scope
}

fn category_nav(
    categories: Vec<categories::CategoryWithFeedCount>,
    selected: Option<Uuid>,
    tag: Option<&str>,
    unread: bool,
    sort: &str,
) -> Vec<CategoryNav> {
    let mut children = BTreeMap::<Option<Uuid>, Vec<_>>::new();
    for category in categories {
        children
            .entry(category.parent_id)
            .or_default()
            .push(category);
    }
    let context = CategoryNavContext {
        selected,
        tag,
        unread,
        sort,
    };
    let mut output = Vec::new();
    let mut seen = HashSet::new();
    append_category_nav(None, 0, &mut children, &mut seen, &mut output, &context);
    for category in children.into_values().flatten() {
        if seen.insert(category.id) {
            output.push(CategoryNav {
                name: category.name,
                indent: String::new(),
                feed_count: category.feed_count,
                active: Some(category.id) == context.selected,
                href: articles_url(
                    None,
                    Some(category.id),
                    context.tag,
                    context.unread,
                    context.sort,
                    None,
                ),
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
    let unread_param = q.unread.as_deref() == Some("1");
    let unread_only = unread_param || !prefs.show_read_articles;
    let offset = q.offset.unwrap_or(0).max(0);
    let feed_id = q.feed;
    let category_id = q.category;
    if let Some(category_id) = category_id {
        if !categories::belongs_to_user(state.db.pool(), user.id, category_id).await? {
            return Err(AppError::NotFound);
        }
    }
    let tag = normalized_tag(q.tag).map_err(AppError::Validation)?;
    let sort = if q.sort.as_deref() == Some("relevance") {
        "relevance"
    } else {
        "recent"
    };

    let filter = articles::LibraryFilter {
        feed_id,
        category_id,
        tag: tag.as_deref(),
    };
    let (article_views, has_next) = if sort == "relevance" {
        relevance_page(&state, user.id, filter, unread_only, per_page, offset).await?
    } else {
        recency_page(&state, user.id, filter, unread_only, per_page, offset).await?
    };

    let (feed_records, category_records, tag_records, organizations) = tokio::try_join!(
        feeds::list_for_user(state.db.pool(), user.id),
        categories::list_with_feed_counts(state.db.pool(), user.id),
        categories::list_tags(state.db.pool(), user.id),
        subscriptions::list_organization(state.db.pool(), user.id),
    )?;
    let category_name = category_id.and_then(|category_id| {
        category_records
            .iter()
            .find(|category| category.id == category_id)
            .map(|category| category.name.as_str())
    });
    let category_scope = category_scope_ids(&category_records, category_id);
    let scope_label = match (category_name, tag.as_deref()) {
        (Some(category), Some(tag)) => format!("{category} · #{tag}"),
        (Some(category), None) => format!("Folder: {category}"),
        (None, Some(tag)) => format!("Tag: #{tag}"),
        (None, None) if unread_only => "Unread articles".to_string(),
        (None, None) if sort == "relevance" => "For you".to_string(),
        (None, None) => "All articles".to_string(),
    };
    let organizations: HashMap<Uuid, subscriptions::FeedOrganization> = organizations
        .into_iter()
        .map(|organization| (organization.feed_id, organization))
        .collect();
    let feed_ids: Vec<Uuid> = feed_records.iter().map(|feed| feed.id).collect();
    let icon_ids = feed_icons::available_for_feeds(state.db.pool(), &feed_ids).await?;
    let feeds = feed_records
        .into_iter()
        .filter_map(|feed| {
            let organization = organizations.get(&feed.id);
            let in_category = category_id.is_none_or(|_| {
                organization.is_some_and(|organization| {
                    organization
                        .category_ids
                        .iter()
                        .any(|category_id| category_scope.contains(category_id))
                })
            });
            let has_tag = tag.as_ref().is_none_or(|tag| {
                organization
                    .is_some_and(|organization| organization.tags.iter().any(|value| value == tag))
            });
            if !in_category || !has_tag {
                return None;
            }
            let id = feed.id;
            Some(FeedNav {
                id: id.to_string(),
                initial: feed_initial(&feed.name),
                has_icon: icon_ids.contains(&id),
                active: Some(id) == feed_id,
                href: articles_url(
                    Some(id),
                    category_id,
                    tag.as_deref(),
                    unread_param,
                    sort,
                    None,
                ),
                name: feed.name,
            })
        })
        .collect();
    let categories = category_nav(
        category_records,
        category_id,
        tag.as_deref(),
        unread_param,
        sort,
    );
    let tags = tag_records
        .into_iter()
        .map(|tag_record| {
            let href = articles_url(
                None,
                category_id,
                Some(&tag_record.name),
                unread_param,
                sort,
                None,
            );
            TagNav {
                active: tag.as_deref() == Some(tag_record.name.as_str()),
                name: tag_record.name,
                feed_count: tag_record.feed_count,
                href,
            }
        })
        .collect();
    let side_panel = prefs.reading_mode == "side-panel" && !article_views.is_empty();
    let back_url = articles_url(
        feed_id,
        category_id,
        tag.as_deref(),
        unread_param,
        sort,
        Some(offset),
    );
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);

    let body = ArticlesTemplate {
        theme: prefs.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui: prefs.clone(),
        is_admin,
        articles: article_views,
        feeds,
        categories,
        tags,
        recent_url: articles_url(None, category_id, tag.as_deref(), false, "recent", None),
        unread_url: articles_url(None, category_id, tag.as_deref(), true, "recent", None),
        relevance_url: articles_url(None, category_id, tag.as_deref(), false, "relevance", None),
        scope_label,
        feed_filter: feed_id.map(|feed_id| feed_id.to_string()),
        category_filter: category_id.map(|category_id| category_id.to_string()),
        tag_filter: tag.clone(),
        unread_only,
        reading_mode: prefs.reading_mode,
        side_panel,
        sort: sort.to_string(),
        back_url,
        has_prev: offset > 0,
        has_next,
        prev_url: articles_url(
            feed_id,
            category_id,
            tag.as_deref(),
            unread_param,
            sort,
            Some((offset - per_page).max(0)),
        ),
        next_url: articles_url(
            feed_id,
            category_id,
            tag.as_deref(),
            unread_param,
            sort,
            Some(offset + per_page),
        ),
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;

    Ok((jar, body).into_response())
}

/// Recency-ordered page (newest first), fetching one extra row for `has_next`.
async fn recency_page(
    state: &AppState,
    user_id: Uuid,
    filter: articles::LibraryFilter<'_>,
    unread_only: bool,
    per_page: i64,
    offset: i64,
) -> Result<(Vec<ArticleView>, bool), AppError> {
    let mut items = articles::list_for_user(
        state.db.pool(),
        user_id,
        filter,
        unread_only,
        per_page + 1,
        offset,
    )
    .await?;
    let has_next = items.len() as i64 > per_page;
    items.truncate(per_page as usize);
    let views = items
        .into_iter()
        .map(|a| ArticleView {
            id: a.id.to_string(),
            title: a.title,
            excerpt: a.excerpt,
            image_url: safe_media_url(a.image_url.as_deref()),
            feed_name: a.feed_name,
            published: a.published_at.map(|d| d.date().to_string()),
            is_read: a.is_read,
            is_bookmarked: a.is_bookmarked,
            is_liked: a.feedback_value.is_some_and(|value| value > 0.0),
            is_disliked: a.feedback_value.is_some_and(|value| value < 0.0),
            score: None,
        })
        .collect();
    Ok((views, has_next))
}

/// Relevance-ranked page: score a recent candidate window against the user's
/// patterns (one batched weights lookup), sort by score, then paginate.
async fn relevance_page(
    state: &AppState,
    user_id: Uuid,
    filter: articles::LibraryFilter<'_>,
    unread_only: bool,
    per_page: i64,
    offset: i64,
) -> Result<(Vec<ArticleView>, bool), AppError> {
    let candidates = articles::list_for_user(
        state.db.pool(),
        user_id,
        filter,
        unread_only,
        RELEVANCE_CANDIDATES,
        0,
    )
    .await?;

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
            title: a.title,
            excerpt: a.excerpt,
            image_url: safe_media_url(a.image_url.as_deref()),
            feed_name: a.feed_name,
            published: a.published_at.map(|d| d.date().to_string()),
            is_read: a.is_read,
            is_bookmarked: a.is_bookmarked,
            is_liked: a.feedback_value.is_some_and(|value| value > 0.0),
            is_disliked: a.feedback_value.is_some_and(|value| value < 0.0),
            score: Some((score.clamp(0.0, 1.0) * 100.0).round() as i64),
        })
        .collect();
    Ok((views, has_next))
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
fn estimate_reading_seconds(content: &str) -> f64 {
    let words = content.split_whitespace().count().max(1) as f64;
    words / 200.0 * 60.0
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
    authorize_action(&state, user.id, id).await?;
    let dedup = format!("summarize:{id}");
    jobs::enqueue(
        state.db.pool(),
        "summarize_article",
        serde_json::json!({ "article_id": id, "user_id": user.id }),
        Some(&dedup),
        None,
        3,
    )
    .await?;
    Ok(action_response(is_ajax, form.next))
}

async fn reader_view(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
    auto_mark_read: bool,
) -> Result<ReaderView, AppError> {
    if !articles::user_can_access(state.db.pool(), user_id, id).await? {
        return Err(AppError::NotFound);
    }
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    if auto_mark_read {
        read_articles::mark_read(state.db.pool(), user_id, id).await?;
    }

    let feed = feeds::find_by_id(state.db.pool(), article.feed_id).await?;
    let feed_name = feed
        .as_ref()
        .map(|feed| feed.name.clone())
        .unwrap_or_default();
    let content_mode = FeedContentSettings::from_json(
        feed.as_ref()
            .and_then(|feed| feed.extraction_settings.as_ref()),
    )
    .mode;
    let content_notice = match (
        content_mode,
        article.content_source.as_str(),
        article.extraction_status.as_str(),
    ) {
        (FeedContentMode::ArticlePage, "feed", "none") => Some(
            "Full article content is queued for this source. Showing feed-provided content until extraction finishes.".to_string(),
        ),
        (FeedContentMode::ArticlePage, "feed", "failed") => Some(
            "Full article content could not be downloaded. Showing the feed-provided fallback.".to_string(),
        ),
        _ => None,
    };
    let stored_summary = articles::get_summary(state.db.pool(), id).await?;
    let can_summarize = state.settings.summarization_enabled && stored_summary.is_none();
    let (summary, key_points, topics) = match stored_summary {
        Some(s) => (Some(s.summary), s.key_points, s.topics),
        None => (None, Vec::new(), Vec::new()),
    };
    let feedback_value = feedback::get(state.db.pool(), user_id, id)
        .await?
        .map(|record| record.feedback_value);

    Ok(ReaderView {
        id: id.to_string(),
        title: article.title,
        feed_name,
        image_url: safe_media_url(article.image_url.as_deref()),
        url: article.url.clone(),
        published: article.published_at.map(|d| d.date().to_string()),
        content: sanitize_article_html(&article.content, &article.url),
        content_notice,
        is_read: read_articles::is_read(state.db.pool(), user_id, id).await?,
        is_bookmarked: bookmarks::is_bookmarked(state.db.pool(), user_id, id).await?,
        is_liked: feedback_value.is_some_and(|value| value > 0.0),
        is_disliked: feedback_value.is_some_and(|value| value < 0.0),
        summary,
        key_points,
        topics,
        can_summarize,
    })
}

/// `GET /articles/{id}` — standalone reading view.
pub async fn read(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let prefs = prefs::load(&state.db, user.id).await;
    let is_admin = user.role == "admin";
    let article = reader_view(&state, user.id, id, prefs.auto_mark_read).await?;
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
        published: article.published,
        image_url: article.image_url,
        content: article.content,
        content_notice: article.content_notice,
        is_read: article.is_read,
        is_bookmarked: article.is_bookmarked,
        is_liked: article.is_liked,
        is_disliked: article.is_disliked,
        summary: article.summary,
        key_points: article.key_points,
        topics: article.topics,
        can_summarize: article.can_summarize,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, body).into_response())
}

pub async fn side_panel(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let prefs = prefs::load(&state.db, user.id).await;
    let article = reader_view(&state, user.id, id, prefs.auto_mark_read).await?;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = ArticlePanelTemplate {
        csrf_token,
        id: article.id,
        title: article.title,
        feed_name: article.feed_name,
        url: article.url,
        published: article.published,
        image_url: article.image_url,
        content: article.content,
        content_notice: article.content_notice,
        is_read: article.is_read,
        is_bookmarked: article.is_bookmarked,
        is_liked: article.is_liked,
        is_disliked: article.is_disliked,
        summary: article.summary,
        key_points: article.key_points,
        topics: article.topics,
        can_summarize: article.can_summarize,
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
) -> Result<Response, AppError> {
    if !articles::user_can_access(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let prefs = prefs::load(&state.db, user.id).await;
    if prefs.auto_mark_read {
        read_articles::mark_read(state.db.pool(), user.id, id).await?;
    }
    Ok(Html(sanitize_article_html(&article.content, &article.url)).into_response())
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
    if let Some(category_id) = form.category {
        if !categories::belongs_to_user(state.db.pool(), user.id, category_id).await? {
            return Err(AppError::NotFound);
        }
    }
    let tag = normalized_tag(form.tag).map_err(AppError::Validation)?;
    read_articles::mark_all_read(
        state.db.pool(),
        user.id,
        form.feed,
        form.category,
        tag.as_deref(),
    )
    .await?;
    Ok(action_response(is_ajax, form.next))
}
