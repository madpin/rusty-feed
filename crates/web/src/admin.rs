use askama::Template;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use rustss_core::feed_content::FeedContentSettings;
use rustss_db::{agent_runs, articles, catalog, cost_ledger, feeds, jobs, users};
use rustss_feeds::{enqueue_fetch_feed, normalize_url};

use crate::auth::csrf;
use crate::auth::extractor::AdminUser;
use crate::catalog_view::{content_size, frequency};
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

const ADMIN_LIST_LIMIT: i64 = 500;

struct CostRow {
    provider: String,
    model: String,
    kind: String,
    calls: i64,
    input_tokens: i64,
    output_tokens: i64,
    cost_usd: String,
}

struct FeedErrorRow {
    feed_id: String,
    feed_name: String,
    occurred_at: String,
    error_type: String,
    summary: String,
    http_status: String,
    health_status: String,
    consecutive_failures: i32,
    resolved: bool,
}

struct AgentRunRow {
    agent_name: String,
    status: String,
    started_at: String,
    duration: String,
    stats: String,
    has_error: bool,
}

struct DashboardStats {
    users: i64,
    feeds: i64,
    articles: i64,
    catalog_entries: i64,
    active_jobs: i64,
}

struct UserRow {
    id: String,
    email: String,
    name: String,
    role: String,
    created: String,
    can_change: bool,
}

struct FeedRow {
    id: String,
    name: String,
    url: String,
    health: String,
    check_cadence: String,
    content_mode: String,
    subscriber_count: i64,
    article_count: i64,
    last_post: String,
}

struct CatalogFields<'a> {
    title: &'a str,
    description: &'a str,
    category: &'a str,
    country: &'a str,
    language: &'a str,
    featured: bool,
    sort_order: i32,
    published: bool,
}

struct CatalogRow {
    id: String,
    feed_id: String,
    title: String,
    description: String,
    category: String,
    country: String,
    language: String,
    featured: bool,
    sort_order: i32,
    published: bool,
    url: String,
    health: String,
    article_count: i64,
    average_content: String,
    last_post: String,
    frequency: String,
}

#[derive(Template)]
#[template(path = "admin_dashboard.html")]
struct DashboardTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    stats: DashboardStats,
}

#[derive(Template)]
#[template(path = "admin_users.html")]
struct UsersTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    users: Vec<UserRow>,
    message: Option<String>,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "admin_feeds.html")]
struct FeedsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    feeds: Vec<FeedRow>,
}

#[derive(Template)]
#[template(path = "admin_catalog.html")]
struct CatalogTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    entries: Vec<CatalogRow>,
    message: Option<String>,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "admin_costs.html")]
struct CostsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    rows: Vec<CostRow>,
    total_usd: String,
}

#[derive(Template)]
#[template(path = "admin_logs.html")]
struct LogsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    feed_errors: Vec<FeedErrorRow>,
    agent_runs: Vec<AgentRunRow>,
}

#[derive(Deserialize)]
pub struct RoleForm {
    csrf_token: String,
    role: String,
}

#[derive(Deserialize)]
pub struct CatalogCreateForm {
    csrf_token: String,
    url: String,
    title: String,
    #[serde(default)]
    description: String,
    category: String,
    country: String,
    language: String,
    #[serde(default)]
    featured: Option<String>,
    #[serde(default)]
    sort_order: Option<i32>,
    #[serde(default)]
    published: Option<String>,
}

#[derive(Deserialize)]
pub struct CatalogUpdateForm {
    csrf_token: String,
    title: String,
    #[serde(default)]
    description: String,
    category: String,
    country: String,
    language: String,
    #[serde(default)]
    featured: Option<String>,
    #[serde(default)]
    sort_order: Option<i32>,
    #[serde(default)]
    published: Option<String>,
}

fn micro_to_usd(micro: i64) -> String {
    format!("${:.4}", micro as f64 / 1_000_000.0)
}

fn numeric_run_stats(value: Option<serde_json::Value>) -> String {
    const KEYS: &[&str] = &[
        "due",
        "enqueued",
        "created",
        "updated",
        "skipped",
        "extract_jobs",
        "embed_jobs",
        "summarize_jobs",
        "decayed",
        "pruned",
    ];
    let Some(serde_json::Value::Object(values)) = value else {
        return "—".to_string();
    };
    let values: Vec<_> = KEYS
        .iter()
        .filter_map(|key| {
            values
                .get(*key)
                .and_then(serde_json::Value::as_i64)
                .map(|value| format!("{key}: {value}"))
        })
        .collect();
    if values.is_empty() {
        "—".to_string()
    } else {
        values.join(", ")
    }
}

fn feed_error_summary(error_type: &str, http_status: Option<i32>) -> String {
    match error_type {
        "http" => http_status
            .map(|status| format!("HTTP {status}"))
            .unwrap_or_else(|| "HTTP request failed".to_string()),
        "network" => "Network request failed".to_string(),
        "invalid_url" => "Feed URL rejected".to_string(),
        "unsupported_scheme" => "Feed URL scheme unsupported".to_string(),
        "response_too_large" => "Response exceeded the size limit".to_string(),
        "parse" => "Feed response could not be parsed".to_string(),
        "database" => "Feed state update failed".to_string(),
        _ => "Feed refresh failed".to_string(),
    }
}

fn feed_check_cadence(
    consecutive_failures: i32,
    failure_started_at: Option<time::OffsetDateTime>,
    configured_minutes: i32,
) -> String {
    if consecutive_failures < 3 {
        return format!("Every {configured_minutes} min");
    }
    if failure_started_at.is_some_and(|started_at| {
        started_at <= time::OffsetDateTime::now_utc() - time::Duration::days(14)
    }) {
        "Weekly while failing".to_string()
    } else {
        "Daily while failing".to_string()
    }
}

fn catalog_row(entry: catalog::AdminCatalogEntry) -> CatalogRow {
    CatalogRow {
        id: entry.id.to_string(),
        feed_id: entry.feed_id.to_string(),
        title: entry.title,
        description: entry.description.unwrap_or_default(),
        category: entry.category,
        country: entry.country,
        language: entry.language,
        featured: entry.featured,
        sort_order: entry.sort_order,
        published: entry.published,
        url: entry.url,
        health: entry.health_status,
        article_count: entry.article_count,
        average_content: content_size(entry.average_rss_content_length),
        last_post: entry
            .last_post_at
            .map(|date| date.date().to_string())
            .unwrap_or_else(|| "No dated posts yet".to_string()),
        frequency: frequency(entry.average_interval_hours),
    }
}

fn render<T: Template>(template: T) -> Result<Html<String>, AppError> {
    template
        .render()
        .map(Html)
        .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))
}

async fn render_users(
    state: &AppState,
    user: &rustss_db::User,
    jar: CookieJar,
    message: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let users = users::list(state.db.pool(), ADMIN_LIST_LIMIT)
        .await?
        .into_iter()
        .map(|candidate| UserRow {
            id: candidate.id.to_string(),
            email: candidate.email,
            name: candidate.name.unwrap_or_else(|| "—".to_string()),
            role: candidate.role,
            created: candidate.created_at.date().to_string(),
            can_change: candidate.id != user.id,
        })
        .collect();
    Ok((
        jar,
        render(UsersTemplate {
            theme: ui.theme.clone(),
            user_email: user.email.clone(),
            csrf_token,
            ui,
            is_admin: true,
            users,
            message,
            error,
        })?,
    )
        .into_response())
}

async fn render_catalog(
    state: &AppState,
    user: &rustss_db::User,
    jar: CookieJar,
    message: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let entries = catalog::list_admin(state.db.pool())
        .await?
        .into_iter()
        .map(catalog_row)
        .collect();
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    Ok((
        jar,
        render(CatalogTemplate {
            theme: ui.theme.clone(),
            user_email: user.email.clone(),
            csrf_token,
            ui,
            is_admin: true,
            entries,
            message,
            error,
        })?,
    )
        .into_response())
}

fn catalog_draft<'a>(fields: CatalogFields<'a>) -> Result<catalog::CatalogDraft<'a>, String> {
    fn required<'a>(value: &'a str, label: &str, limit: usize) -> Result<&'a str, String> {
        let value = value.trim();
        if value.is_empty() || value.len() > limit {
            return Err(format!("{label} must be between 1 and {limit} characters."));
        }
        Ok(value)
    }
    let description = (!fields.description.trim().is_empty()).then_some(fields.description.trim());
    if description.is_some_and(|value| value.len() > 2_000) {
        return Err("Description must be at most 2,000 characters.".to_string());
    }
    Ok(catalog::CatalogDraft {
        title: required(fields.title, "Title", 200)?,
        description,
        category: required(fields.category, "Category", 80)?,
        country: required(fields.country, "Country", 80)?,
        language: required(fields.language, "Language", 35)?,
        featured: fields.featured,
        sort_order: fields.sort_order.clamp(-10_000, 10_000),
        published: fields.published,
    })
}

pub async fn dashboard(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let (user_count, feed_count, article_count, catalog_entries, active_jobs) = tokio::try_join!(
        users::count(state.db.pool()),
        feeds::count(state.db.pool()),
        articles::count(state.db.pool()),
        catalog::count(state.db.pool()),
        jobs::count_active(state.db.pool()),
    )?;
    let ui = prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    Ok((
        jar,
        render(DashboardTemplate {
            theme: ui.theme.clone(),
            user_email: user.email,
            csrf_token,
            ui,
            is_admin: true,
            stats: DashboardStats {
                users: user_count,
                feeds: feed_count,
                articles: article_count,
                catalog_entries,
                active_jobs,
            },
        })?,
    )
        .into_response())
}

pub async fn users_page(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render_users(&state, &user, jar, None, None).await
}

pub async fn update_role(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<RoleForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let message = match users::change_role(state.db.pool(), user.id, id, form.role.trim()).await? {
        users::RoleChange::Changed => {
            return Ok((jar, Redirect::to("/admin/users")).into_response())
        }
        users::RoleChange::TargetNotFound => "That user no longer exists.",
        users::RoleChange::InvalidRole => "Choose admin or user.",
        users::RoleChange::CannotDemoteSelf => "You cannot demote your own administrator account.",
        users::RoleChange::LastAdmin => "Keep at least one administrator account.",
    };
    render_users(&state, &user, jar, None, Some(message.to_string())).await
}

pub async fn feeds_page(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let feeds = feeds::list_admin(state.db.pool(), ADMIN_LIST_LIMIT)
        .await?
        .into_iter()
        .map(|feed| FeedRow {
            id: feed.id.to_string(),
            name: feed.name,
            url: feed.url,
            health: feed.health_status,
            check_cadence: feed_check_cadence(
                feed.consecutive_failures,
                feed.failure_started_at,
                feed.fetch_interval_minutes,
            ),
            content_mode: FeedContentSettings::from_json(feed.extraction_settings.as_ref())
                .mode
                .label()
                .to_string(),
            subscriber_count: feed.subscriber_count,
            article_count: feed.article_count,
            last_post: feed
                .last_post_at
                .map(|date| date.date().to_string())
                .unwrap_or_else(|| "No dated posts yet".to_string()),
        })
        .collect();
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    Ok((
        jar,
        render(FeedsTemplate {
            theme: ui.theme.clone(),
            user_email: user.email,
            csrf_token,
            ui,
            is_admin: true,
            feeds,
        })?,
    )
        .into_response())
}

pub async fn catalog_page(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render_catalog(&state, &user, jar, None, None).await
}

pub async fn create_catalog_entry(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Form(form): Form<CatalogCreateForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let draft = match catalog_draft(CatalogFields {
        title: &form.title,
        description: &form.description,
        category: &form.category,
        country: &form.country,
        language: &form.language,
        featured: form.featured.is_some(),
        sort_order: form.sort_order.unwrap_or_default(),
        published: form.published.is_some(),
    }) {
        Ok(draft) => draft,
        Err(error) => return render_catalog(&state, &user, jar, None, Some(error)).await,
    };
    let url = match normalize_url(&form.url) {
        Ok(url) => url,
        Err(error) => {
            return render_catalog(&state, &user, jar, None, Some(error.to_string())).await
        }
    };
    if let Err(error) = state.fetch.validate_url(&url) {
        return render_catalog(&state, &user, jar, None, Some(error.to_string())).await;
    }
    let (feed, _) = feeds::upsert_by_url(state.db.pool(), &url, draft.title, None).await?;
    catalog::upsert(state.db.pool(), feed.id, draft).await?;
    let queued = enqueue_fetch_feed(&state.db, feed.id).await?.is_some();
    let message = if queued {
        "Catalog entry saved and its first refresh was queued."
    } else {
        "Catalog entry saved; a refresh is already queued."
    };
    render_catalog(&state, &user, jar, Some(message.to_string()), None).await
}

pub async fn update_catalog_entry(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CatalogUpdateForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let draft = match catalog_draft(CatalogFields {
        title: &form.title,
        description: &form.description,
        category: &form.category,
        country: &form.country,
        language: &form.language,
        featured: form.featured.is_some(),
        sort_order: form.sort_order.unwrap_or_default(),
        published: form.published.is_some(),
    }) {
        Ok(draft) => draft,
        Err(error) => return render_catalog(&state, &user, jar, None, Some(error)).await,
    };
    if !catalog::update(state.db.pool(), id, draft).await? {
        return render_catalog(
            &state,
            &user,
            jar,
            None,
            Some("That catalog entry no longer exists.".to_string()),
        )
        .await;
    }
    render_catalog(
        &state,
        &user,
        jar,
        Some("Catalog entry updated.".to_string()),
        None,
    )
    .await
}

pub async fn logs(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let (feed_errors, agent_runs) = tokio::try_join!(
        feeds::list_recent_errors(state.db.pool(), ADMIN_LIST_LIMIT),
        agent_runs::list_recent(state.db.pool(), ADMIN_LIST_LIMIT),
    )?;
    let ui = prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = render(LogsTemplate {
        theme: ui.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui,
        is_admin: true,
        feed_errors: feed_errors
            .into_iter()
            .map(|error| {
                let summary = feed_error_summary(&error.error_type, error.http_status);
                FeedErrorRow {
                    feed_id: error.feed_id.to_string(),
                    feed_name: error.feed_name,
                    occurred_at: error.occurred_at.to_string(),
                    error_type: error.error_type,
                    summary,
                    http_status: error
                        .http_status
                        .map(|status| status.to_string())
                        .unwrap_or_else(|| "—".to_string()),
                    health_status: error.health_status,
                    consecutive_failures: error.consecutive_failures,
                    resolved: error.resolved,
                }
            })
            .collect(),
        agent_runs: agent_runs
            .into_iter()
            .map(|run| AgentRunRow {
                agent_name: run.agent_name,
                status: run.status,
                started_at: run.started_at.to_string(),
                duration: run
                    .duration_ms
                    .map(|duration| format!("{duration} ms"))
                    .unwrap_or_else(|| "Running".to_string()),
                stats: numeric_run_stats(run.stats),
                has_error: run.has_error,
            })
            .collect(),
    })?;
    let mut response = (jar, body).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub async fn costs(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let rows = cost_ledger::by_model(state.db.pool())
        .await?
        .into_iter()
        .map(|row| CostRow {
            provider: row.provider,
            model: row.model,
            kind: row.kind,
            calls: row.calls,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cost_usd: micro_to_usd(row.cost_micro_usd),
        })
        .collect();
    let total_usd = micro_to_usd(cost_ledger::total_micro_usd(state.db.pool()).await?);
    let ui = prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    Ok((
        jar,
        render(CostsTemplate {
            theme: ui.theme.clone(),
            user_email: user.email,
            csrf_token,
            ui,
            is_admin: true,
            rows,
            total_usd,
        })?,
    )
        .into_response())
}
