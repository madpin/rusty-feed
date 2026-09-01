use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use rusty_feed_core::feed_content::FeedContentSettings;
use rusty_feed_db::{
    agent_runs, api_audit, articles, catalog, cost_ledger, feeds, invitations, jobs, operations,
    public_shares, users,
};
use rusty_feed_feeds::{enqueue_fetch_feed, normalize_url};

use crate::auth::extractor::{AdminReadUser, AdminUser};
use crate::auth::{csrf, token};
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
    tone: String,
    started_at: String,
    duration: String,
    stats: String,
    error: String,
}

struct QueueStats {
    pending_due: i64,
    pending_scheduled: i64,
    running: i64,
    stalled: i64,
    finished_last_hour: i64,
    dead: i64,
    workers_online: usize,
    worker_capacity: i64,
    background_capacity: i64,
    interactive_capacity: i64,
    background_pending: i64,
    background_running: i64,
    interactive_pending: i64,
    interactive_running: i64,
}

struct QueueKindRow {
    lane: String,
    kind: String,
    pending_due: i64,
    pending_scheduled: i64,
    running: i64,
    stalled: i64,
    dead: i64,
}

struct QueueItemRow {
    lane: String,
    kind: String,
    target: String,
    status: String,
    tone: String,
    attempts: String,
    created_at: String,
    run_at: String,
    worker: String,
    updated_at: String,
    error: String,
}

struct WorkerRow {
    id: String,
    concurrency: i32,
    background_concurrency: i32,
    interactive_concurrency: i32,
    scheduler: String,
    started_at: String,
    heartbeat_at: String,
}

struct DashboardStats {
    users: i64,
    feeds: i64,
    articles: i64,
    catalog_entries: i64,
    pending_jobs: i64,
    running_jobs: i64,
    stalled_jobs: i64,
    dead_jobs: i64,
    workers_online: usize,
    worker_capacity: i64,
    active_public_shares: i64,
    database_size: String,
    article_storage: String,
    embedding_storage: String,
    media_storage: String,
    process_memory: String,
}

struct PublicShareEventRow {
    action: String,
    user_email: String,
    article_title: String,
    policy: String,
    occurred_at: String,
}

struct ApiAuditRow {
    operation: String,
    user: String,
    token: String,
    occurred_at: String,
}

struct UserRow {
    id: String,
    email: String,
    name: String,
    role: String,
    created: String,
    last_seen: String,
    can_change: bool,
}

struct InvitationRow {
    id: String,
    email: String,
    created: String,
    expires: String,
    status: String,
    can_revoke: bool,
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
    last_checked: String,
}

struct FeedAdminSummary {
    total: i64,
    healthy: i64,
    warning: i64,
    error: i64,
    disabled: i64,
    subscribers: i64,
    articles: i64,
}

struct CatalogSummary {
    total: i64,
    published: i64,
    drafts: i64,
    featured: i64,
    posts: i64,
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
    youtube_embeds_enabled: bool,
    youtube_source_host_count: usize,
    youtube_alternative_player: String,
    pwa_name: String,
    pwa_short_name: String,
    pwa_theme_color: String,
    pwa_background_color: String,
    public_article_sharing_enabled: bool,
    public_share_default_lifetime_days: u16,
    public_share_max_lifetime_days: u16,
    public_share_allow_infinite: bool,
    outbound_user_agent: String,
    outbound_protocol: String,
    reverse_proxy_auth_enabled: bool,
    reverse_proxy_trusted_networks: usize,
    reverse_proxy_auto_provision: bool,
    reverse_proxy_allowed_domains: usize,
    reverse_proxy_admin_mappings: usize,
    reverse_proxy_logout_configured: bool,
    media_proxy_enabled: bool,
    media_proxy_max_bytes: usize,
    media_proxy_cache_budget_bytes: usize,
    media_proxy_cache_ttl_hours: u64,
    media_proxy_extra_mime_types: usize,
    tracking_url_cleanup_enabled: bool,
    tracking_url_rule_count: usize,
    tracking_url_exception_count: usize,
    api_enabled: bool,
    greader_api_enabled: bool,
    fever_api_enabled: bool,
    api_audit_events: Vec<ApiAuditRow>,
    public_share_events: Vec<PublicShareEventRow>,
}

#[derive(Template)]
#[template(path = "admin_users.html")]
struct UsersTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    admin_readonly: bool,
    users: Vec<UserRow>,
    invitations: Vec<InvitationRow>,
    invitation_link: Option<String>,
    message: Option<String>,
    error: Option<String>,
}

/// Mask an email for read-only admin views: keep the first character and domain.
fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) => {
            let first = local.chars().next().unwrap_or('*');
            format!("{first}***@{domain}")
        }
        None => "***".to_string(),
    }
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
    summary: FeedAdminSummary,
    filter_health: String,
    filter_query: String,
    total_matching: i64,
    showing_from: i64,
    showing_to: i64,
    page: i64,
    total_pages: i64,
    has_prev: bool,
    has_next: bool,
    prev_url: String,
    next_url: String,
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
    summary: CatalogSummary,
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
    admin_readonly: bool,
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
    queue: QueueStats,
    queue_by_kind: Vec<QueueKindRow>,
    active_jobs: Vec<QueueItemRow>,
    recent_jobs: Vec<QueueItemRow>,
    workers: Vec<WorkerRow>,
}

#[derive(Deserialize)]
pub struct RoleForm {
    csrf_token: String,
    role: String,
}

#[derive(Deserialize, Default)]
pub struct FeedsQuery {
    #[serde(default)]
    health: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    page: Option<i64>,
}

#[derive(Deserialize)]
pub struct InvitationForm {
    csrf_token: String,
    #[serde(default)]
    email: String,
    #[serde(default = "default_invitation_lifetime")]
    lifetime_days: i64,
}

fn default_invitation_lifetime() -> i64 {
    7
}

#[derive(Deserialize)]
pub struct CsrfOnlyForm {
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct FeedStatusForm {
    csrf_token: String,
    status: String,
}

#[derive(Deserialize)]
pub struct MaintenanceForm {
    csrf_token: String,
    action: String,
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

/// Best-effort resident set size of the current process, in bytes. Reads
/// `/proc/self/status` on Linux (the container runtime); returns `None`
/// elsewhere (e.g. macOS development hosts).
fn process_rss_bytes() -> Option<i64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kib: i64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kib * 1024);
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn bytes_label(bytes: i64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes.max(0) as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn safe_run_stats(value: Option<serde_json::Value>) -> String {
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
    let mut output: Vec<_> = KEYS
        .iter()
        .filter_map(|key| {
            values
                .get(*key)
                .and_then(serde_json::Value::as_i64)
                .map(|value| format!("{key}: {value}"))
        })
        .collect();
    if let Some(status) = values.get("status").and_then(serde_json::Value::as_str) {
        let status = match status {
            "success" => Some("content: ready"),
            "failed" => Some("content: RSS fallback"),
            "disallowed" => Some("content: blocked by robots policy"),
            _ => None,
        };
        if let Some(status) = status {
            output.push(status.to_string());
        }
    }
    if let Some(extractor) = values
        .get("extractor")
        .and_then(serde_json::Value::as_str)
        .filter(|extractor| matches!(*extractor, "readability" | "chromium"))
    {
        output.push(format!("extractor: {extractor}"));
    }
    if let Some(skipped) = values
        .get("skipped")
        .and_then(serde_json::Value::as_str)
        .filter(|reason| {
            matches!(
                *reason,
                "article gone"
                    | "feed gone"
                    | "stale content revision"
                    | "no subscriber requests public-page content"
                    | "public-page content is current"
                    | "feed is paused"
                    | "feed is blocked"
                    | "over AI budget"
                    | "no LLM configured"
            )
        })
    {
        output.push(format!("skipped: {skipped}"));
    }
    if output.is_empty() {
        "—".to_string()
    } else {
        output.join(", ")
    }
}

fn agent_run_tone(
    status: &str,
    stats: Option<&serde_json::Value>,
    has_error: bool,
) -> &'static str {
    if has_error || status == "failed" {
        return "error";
    }
    if stats
        .and_then(|stats| stats.get("status"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|status| matches!(status, "failed" | "disallowed"))
    {
        return "warning";
    }
    "healthy"
}

fn queue_item(item: jobs::QueueItemSummary) -> QueueItemRow {
    let status = if item.status == "running" && !item.worker_online {
        "stalled".to_string()
    } else if item.status == "pending" && item.run_at > time::OffsetDateTime::now_utc() {
        "scheduled".to_string()
    } else {
        item.status
    };
    let tone = match status.as_str() {
        "done" => "healthy",
        "dead" => "error",
        "stalled" => "error",
        "running" => "warning",
        _ => "",
    };
    QueueItemRow {
        lane: item.queue_name,
        kind: item.kind,
        target: item.target.unwrap_or_else(|| "—".to_string()),
        status,
        tone: tone.to_string(),
        attempts: format!("{} / {}", item.attempts, item.max_attempts),
        created_at: item.created_at.to_string(),
        run_at: item.run_at.to_string(),
        worker: item.locked_by.unwrap_or_else(|| "—".to_string()),
        updated_at: item.updated_at.to_string(),
        error: item.last_error.unwrap_or_else(|| "—".to_string()),
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
    user: &rusty_feed_db::User,
    jar: CookieJar,
    message: Option<String>,
    error: Option<String>,
    invitation_link: Option<String>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    // Read-only admins (e.g. the demo "see as admin") must not see other users'
    // PII, so mask emails and hide mutation controls (§7.6 privacy).
    let admin_readonly = user.role == "admin_readonly";
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let users = users::list(state.db.pool(), ADMIN_LIST_LIMIT)
        .await?
        .into_iter()
        .map(|candidate| UserRow {
            id: candidate.id.to_string(),
            email: if admin_readonly {
                mask_email(&candidate.email)
            } else {
                candidate.email
            },
            name: if admin_readonly {
                "—".to_string()
            } else {
                candidate.name.unwrap_or_else(|| "—".to_string())
            },
            role: candidate.role,
            created: candidate.created_at.date().to_string(),
            last_seen: candidate
                .last_seen_at
                .map(|seen| seen.date().to_string())
                .unwrap_or_else(|| "—".to_string()),
            can_change: !admin_readonly && candidate.id != user.id,
        })
        .collect();
    let now = time::OffsetDateTime::now_utc();
    let invitations = invitations::list(state.db.pool(), 100)
        .await?
        .into_iter()
        .map(|invitation| {
            let (status, can_revoke) = if invitation.accepted_at.is_some() {
                ("Accepted".to_string(), false)
            } else if invitation.revoked_at.is_some() {
                ("Revoked".to_string(), false)
            } else if invitation.expires_at <= now {
                ("Expired".to_string(), false)
            } else {
                ("Available".to_string(), true)
            };
            let email = invitation
                .email
                .map(|value| {
                    if admin_readonly {
                        mask_email(&value)
                    } else {
                        value
                    }
                })
                .unwrap_or_else(|| "Anyone with the link".to_string());
            InvitationRow {
                id: invitation.id.to_string(),
                email,
                created: invitation.created_at.date().to_string(),
                expires: invitation.expires_at.date().to_string(),
                status,
                can_revoke,
            }
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
            admin_readonly,
            users,
            invitations,
            invitation_link,
            message,
            error,
        })?,
    )
        .into_response())
}

async fn render_catalog(
    state: &AppState,
    user: &rusty_feed_db::User,
    jar: CookieJar,
    message: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let entries: Vec<CatalogRow> = catalog::list_admin(state.db.pool())
        .await?
        .into_iter()
        .map(catalog_row)
        .collect();
    let summary = CatalogSummary {
        total: entries.len() as i64,
        published: entries.iter().filter(|entry| entry.published).count() as i64,
        drafts: entries.iter().filter(|entry| !entry.published).count() as i64,
        featured: entries.iter().filter(|entry| entry.featured).count() as i64,
        posts: entries.iter().map(|entry| entry.article_count).sum(),
    };
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
            summary,
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
    AdminReadUser(user): AdminReadUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let (
        user_count,
        feed_count,
        article_count,
        catalog_entries,
        queue,
        workers,
        active_public_shares,
        public_share_events,
        api_audit_events,
        storage,
    ) = tokio::try_join!(
        users::count(state.db.pool()),
        feeds::count(state.db.pool()),
        articles::count(state.db.pool()),
        catalog::count(state.db.pool()),
        jobs::queue_totals(state.db.pool()),
        jobs::list_online_workers(state.db.pool()),
        public_shares::active_count(state.db.pool()),
        public_shares::recent_events(state.db.pool(), 20),
        api_audit::recent(state.db.pool(), 20),
        operations::storage_stats(state.db.pool()),
    )?;
    let worker_capacity = workers
        .iter()
        .map(|worker| i64::from(worker.concurrency))
        .sum();
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
            youtube_embeds_enabled: state.settings.youtube_policy.enabled(),
            youtube_source_host_count: state.settings.youtube_policy.source_host_count(),
            youtube_alternative_player: state
                .settings
                .youtube_policy
                .alternative_name()
                .map_or_else(|| "Not configured".to_string(), str::to_string),
            pwa_name: state.settings.pwa_name.clone(),
            pwa_short_name: state.settings.pwa_short_name.clone(),
            pwa_theme_color: state.settings.pwa_theme_color.clone(),
            pwa_background_color: state.settings.pwa_background_color.clone(),
            public_article_sharing_enabled: state.settings.public_article_sharing_enabled,
            public_share_default_lifetime_days: state.settings.public_share_default_lifetime_days,
            public_share_max_lifetime_days: state.settings.public_share_max_lifetime_days,
            public_share_allow_infinite: state.settings.public_share_allow_infinite,
            outbound_user_agent: state.settings.outbound_user_agent.clone(),
            outbound_protocol: if state.settings.outbound_http1_only {
                "HTTP/1.1 only".to_string()
            } else {
                "HTTP/2 with HTTP/1.1 fallback".to_string()
            },
            reverse_proxy_auth_enabled: state.settings.reverse_proxy_auth.enabled,
            reverse_proxy_trusted_networks: state.settings.reverse_proxy_auth.trusted_cidrs.len(),
            reverse_proxy_auto_provision: state.settings.reverse_proxy_auth.auto_provision,
            reverse_proxy_allowed_domains: state.settings.reverse_proxy_auth.allowed_domains.len(),
            reverse_proxy_admin_mappings: state.settings.reverse_proxy_auth.admin_emails.len(),
            reverse_proxy_logout_configured: state.settings.reverse_proxy_auth.logout_url.is_some(),
            media_proxy_enabled: state.settings.media_proxy_enabled,
            media_proxy_max_bytes: state.settings.media_proxy_max_bytes,
            media_proxy_cache_budget_bytes: state.settings.media_proxy_cache_budget_bytes,
            media_proxy_cache_ttl_hours: state.settings.media_proxy_cache_ttl.as_secs() / 3_600,
            media_proxy_extra_mime_types: state.settings.media_proxy_extra_inline_mime_types.len(),
            tracking_url_cleanup_enabled: state.settings.tracking_url_policy.enabled(),
            tracking_url_rule_count: state.settings.tracking_url_policy.parameter_rule_count(),
            tracking_url_exception_count: state
                .settings
                .tracking_url_policy
                .domain_exception_count(),
            api_enabled: state.settings.api_enabled,
            greader_api_enabled: state.settings.greader_api_enabled,
            fever_api_enabled: state.settings.fever_api_enabled,
            api_audit_events: api_audit_events
                .into_iter()
                .map(|event| ApiAuditRow {
                    operation: event.operation,
                    user: event.user_email,
                    token: event
                        .token_name
                        .unwrap_or_else(|| "Revoked token".to_string()),
                    occurred_at: event.created_at.to_string(),
                })
                .collect(),
            public_share_events: public_share_events
                .into_iter()
                .map(|event| PublicShareEventRow {
                    action: if event.action == "revoked" {
                        "Revoked".to_string()
                    } else {
                        "Created or rotated".to_string()
                    },
                    user_email: event.user_email,
                    article_title: event.article_title,
                    policy: event.content_source.map_or_else(
                        || "Link disabled".to_string(),
                        |source| {
                            let lifetime = event.expires_at.map_or_else(
                                || "infinite".to_string(),
                                |expires| format!("until {}", expires.date()),
                            );
                            let images = if event.include_images.unwrap_or(false) {
                                "external images on"
                            } else {
                                "external images off"
                            };
                            format!("{source}, {lifetime}, {images}")
                        },
                    ),
                    occurred_at: event.created_at.to_string(),
                })
                .collect(),
            stats: DashboardStats {
                users: user_count,
                feeds: feed_count,
                articles: article_count,
                catalog_entries,
                pending_jobs: queue.pending_due + queue.pending_scheduled,
                running_jobs: queue.running,
                stalled_jobs: queue.stalled,
                dead_jobs: queue.dead,
                workers_online: workers.len(),
                worker_capacity,
                active_public_shares,
                database_size: bytes_label(storage.database_bytes),
                article_storage: bytes_label(storage.articles_bytes),
                embedding_storage: bytes_label(storage.embeddings_bytes),
                media_storage: bytes_label(storage.media_cache_bytes),
                process_memory: process_rss_bytes()
                    .map(bytes_label)
                    .unwrap_or_else(|| "Unavailable on this platform".to_string()),
            },
        })?,
    )
        .into_response())
}

pub async fn maintenance(
    State(state): State<AppState>,
    AdminUser(_user): AdminUser,
    jar: CookieJar,
    Form(form): Form<MaintenanceForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    match form.action.as_str() {
        "refresh_due" => {
            jobs::enqueue(
                state.db.pool(),
                "feed_refresh",
                serde_json::json!({}),
                Some("feed_refresh"),
                None,
                3,
            )
            .await?;
        }
        "cleanup" => {
            jobs::enqueue(
                state.db.pool(),
                "content_cleanup",
                serde_json::json!({}),
                Some("content_cleanup"),
                None,
                3,
            )
            .await?;
        }
        "reclaim_stalled" => {
            jobs::reclaim_stale(state.db.pool(), time::Duration::minutes(10)).await?;
        }
        "retry_dead" => {
            jobs::retry_dead(state.db.pool(), 100).await?;
        }
        _ => {
            return Err(AppError::Validation(
                "Unknown maintenance action.".to_string(),
            ))
        }
    }
    Ok((jar, Redirect::to("/admin/logs")).into_response())
}

pub async fn users_page(
    State(state): State<AppState>,
    AdminReadUser(user): AdminReadUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render_users(&state, &user, jar, None, None, None).await
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
    render_users(&state, &user, jar, None, Some(message.to_string()), None).await
}

pub async fn create_invitation(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Form(form): Form<InvitationForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let email = form.email.trim();
    if !email.is_empty() && (!email.contains('@') || email.len() > 320) {
        return render_users(
            &state,
            &user,
            jar,
            None,
            Some("Enter a valid email address or leave it blank.".to_string()),
            None,
        )
        .await;
    }
    if !(1..=90).contains(&form.lifetime_days) {
        return render_users(
            &state,
            &user,
            jar,
            None,
            Some("Invitation lifetime must be between 1 and 90 days.".to_string()),
            None,
        )
        .await;
    }
    let generated = token::generate();
    invitations::create(
        state.db.pool(),
        user.id,
        &generated.hash,
        (!email.is_empty()).then_some(email),
        form.lifetime_days,
    )
    .await?;
    let link = format!(
        "{}/register?invite={}",
        state.settings.base_url.trim_end_matches('/'),
        generated.raw
    );
    render_users(
        &state,
        &user,
        jar,
        Some("Invitation created. Copy the link now; Rusty Feed stores only its hash.".to_string()),
        None,
        Some(link),
    )
    .await
}

pub async fn revoke_invitation(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfOnlyForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let message = if invitations::revoke(state.db.pool(), id).await? {
        "Invitation revoked."
    } else {
        "That invitation is no longer available."
    };
    render_users(&state, &user, jar, Some(message.to_string()), None, None).await
}

const ADMIN_FEEDS_PER_PAGE: i64 = 25;

pub async fn feeds_page(
    State(state): State<AppState>,
    AdminReadUser(user): AdminReadUser,
    jar: CookieJar,
    Query(query): Query<FeedsQuery>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;

    let health = query
        .health
        .as_deref()
        .map(str::trim)
        .filter(|value| matches!(*value, "healthy" | "warning" | "error" | "disabled"))
        .map(str::to_string);
    let search = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let summary = feeds::admin_summary(state.db.pool()).await?;
    let total_matching =
        feeds::count_admin_filtered(state.db.pool(), health.as_deref(), search.as_deref()).await?;
    let total_pages = (total_matching + ADMIN_FEEDS_PER_PAGE - 1) / ADMIN_FEEDS_PER_PAGE;
    let page = query.page.unwrap_or(1).max(1).min(total_pages.max(1));
    let offset = (page - 1) * ADMIN_FEEDS_PER_PAGE;

    let feeds: Vec<FeedRow> = feeds::list_admin_filtered(
        state.db.pool(),
        health.as_deref(),
        search.as_deref(),
        ADMIN_FEEDS_PER_PAGE,
        offset,
    )
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
            .unwrap_or_else(|| "—".to_string()),
        last_checked: feed
            .last_fetched_at
            .map(|date| date.date().to_string())
            .unwrap_or_else(|| "Never".to_string()),
    })
    .collect();

    let showing_from = if feeds.is_empty() { 0 } else { offset + 1 };
    let showing_to = offset + feeds.len() as i64;
    let filter_health = health.clone().unwrap_or_default();
    let filter_query = search.clone().unwrap_or_default();
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
            summary: FeedAdminSummary {
                total: summary.total,
                healthy: summary.healthy,
                warning: summary.warning,
                error: summary.error,
                disabled: summary.disabled,
                subscribers: summary.subscribers,
                articles: summary.articles,
            },
            prev_url: admin_feeds_url(&filter_health, &filter_query, page - 1),
            next_url: admin_feeds_url(&filter_health, &filter_query, page + 1),
            filter_health,
            filter_query,
            total_matching,
            showing_from,
            showing_to,
            page,
            total_pages: total_pages.max(1),
            has_prev: page > 1,
            has_next: page < total_pages,
        })?,
    )
        .into_response())
}

fn admin_feeds_url(health: &str, query: &str, page: i64) -> String {
    let mut params: Vec<String> = Vec::new();
    if !health.is_empty() {
        params.push(format!("health={}", urlencoding_encode(health)));
    }
    if !query.is_empty() {
        params.push(format!("q={}", urlencoding_encode(query)));
    }
    if page > 1 {
        params.push(format!("page={page}"));
    }
    if params.is_empty() {
        "/admin/feeds".to_string()
    } else {
        format!("/admin/feeds?{}", params.join("&"))
    }
}

fn urlencoding_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

pub async fn set_feed_status(
    State(state): State<AppState>,
    AdminUser(_user): AdminUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<FeedStatusForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let paused = match form.status.as_str() {
        "paused" => true,
        "active" => false,
        _ => return Err(AppError::Validation("Choose active or paused.".into())),
    };
    if !feeds::set_paused(state.db.pool(), id, paused).await? {
        return Err(AppError::NotFound);
    }
    if !paused {
        let _ = enqueue_fetch_feed(&state.db, id).await?;
    }
    Ok((jar, Redirect::to("/admin/feeds")).into_response())
}

pub async fn catalog_page(
    State(state): State<AppState>,
    AdminReadUser(user): AdminReadUser,
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
    if let Err(crate::error::AppError::Validation(message)) =
        crate::moderation::assert_feed_allowed(&state, &url).await
    {
        return render_catalog(&state, &user, jar, None, Some(message)).await;
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
    AdminReadUser(user): AdminReadUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let (feed_errors, agent_runs, queue, queue_by_kind, active_jobs, recent_jobs, workers) = tokio::try_join!(
        feeds::list_recent_errors(state.db.pool(), ADMIN_LIST_LIMIT),
        agent_runs::list_recent(state.db.pool(), ADMIN_LIST_LIMIT),
        jobs::queue_totals(state.db.pool()),
        jobs::queue_by_kind(state.db.pool()),
        jobs::list_active(state.db.pool(), 100),
        jobs::list_recent_finished(state.db.pool(), 100),
        jobs::list_online_workers(state.db.pool()),
    )?;
    let ui = prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = render(LogsTemplate {
        theme: ui.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui,
        is_admin: true,
        queue: QueueStats {
            pending_due: queue.pending_due,
            pending_scheduled: queue.pending_scheduled,
            running: queue.running,
            stalled: queue.stalled,
            finished_last_hour: queue.finished_last_hour,
            dead: queue.dead,
            workers_online: workers.len(),
            worker_capacity: workers
                .iter()
                .map(|worker| i64::from(worker.concurrency))
                .sum(),
            background_capacity: workers
                .iter()
                .map(|worker| i64::from(worker.concurrency - worker.interactive_concurrency))
                .sum(),
            interactive_capacity: workers
                .iter()
                .map(|worker| i64::from(worker.interactive_concurrency))
                .sum(),
            background_pending: queue.background_pending,
            background_running: queue.background_running,
            interactive_pending: queue.interactive_pending,
            interactive_running: queue.interactive_running,
        },
        queue_by_kind: queue_by_kind
            .into_iter()
            .map(|row| QueueKindRow {
                lane: row.queue_name,
                kind: row.kind,
                pending_due: row.pending_due,
                pending_scheduled: row.pending_scheduled,
                running: row.running,
                stalled: row.stalled,
                dead: row.dead,
            })
            .collect(),
        active_jobs: active_jobs.into_iter().map(queue_item).collect(),
        recent_jobs: recent_jobs.into_iter().map(queue_item).collect(),
        workers: workers
            .into_iter()
            .map(|worker| WorkerRow {
                id: worker.worker_id,
                concurrency: worker.concurrency,
                background_concurrency: worker.concurrency - worker.interactive_concurrency,
                interactive_concurrency: worker.interactive_concurrency,
                scheduler: if worker.scheduler_enabled {
                    "On".to_string()
                } else {
                    "Off".to_string()
                },
                started_at: worker.started_at.to_string(),
                heartbeat_at: worker.last_heartbeat_at.to_string(),
            })
            .collect(),
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
            .map(|run| {
                let tone = agent_run_tone(&run.status, run.stats.as_ref(), run.has_error);
                let display_status = if tone == "warning" && run.status == "success" {
                    "fallback"
                } else {
                    &run.status
                };
                AgentRunRow {
                    agent_name: run.agent_name,
                    status: display_status.to_string(),
                    tone: tone.to_string(),
                    started_at: run.started_at.to_string(),
                    duration: run
                        .duration_ms
                        .map(|duration| format!("{duration} ms"))
                        .unwrap_or_else(|| "Running".to_string()),
                    stats: safe_run_stats(run.stats),
                    error: run.error_message.unwrap_or_else(|| "—".to_string()),
                }
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
    AdminReadUser(user): AdminReadUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    // Read-only admins see the aggregate total only, never the per-model
    // breakdown (§7.6 privacy).
    let admin_readonly = user.role == "admin_readonly";
    let rows = if admin_readonly {
        Vec::new()
    } else {
        cost_ledger::by_model(state.db.pool())
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
            .collect()
    };
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
            admin_readonly,
            rows,
            total_usd,
        })?,
    )
        .into_response())
}

#[derive(Template)]
#[template(path = "admin_activity.html")]
struct ActivityTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    admin_readonly: bool,
    open_reports: i64,
    signups: Vec<ActivityUserRow>,
    locked: Vec<ActivityUserRow>,
    ai_users: Vec<AiUsageRow>,
}

struct ActivityUserRow {
    email: String,
    detail: String,
    tags: String,
}

struct AiUsageRow {
    email: String,
    calls: i64,
    cost_usd: String,
}

/// `GET /admin/activity` — per-user abuse/activity signals (§10.1): recent
/// signups, locked accounts, top AI usage, and the open-report count. PII is
/// masked and per-user cost is hidden for read-only admins (§7.6).
pub async fn activity(
    State(state): State<AppState>,
    AdminReadUser(user): AdminReadUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let admin_readonly = user.role == "admin_readonly";
    let ui = prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let mask = |email: String| {
        if admin_readonly {
            mask_email(&email)
        } else {
            email
        }
    };
    let day_ago = time::OffsetDateTime::now_utc() - time::Duration::days(1);
    let (signups_raw, locked_raw, ai_raw, open_reports) = tokio::try_join!(
        users::recent_signups(state.db.pool(), 20),
        users::locked_accounts(state.db.pool(), 20),
        cost_ledger::usage_by_user_since(state.db.pool(), day_ago, 20),
        rusty_feed_db::abuse_reports::count_open(state.db.pool()),
    )?;
    let signups = signups_raw
        .into_iter()
        .map(|u| ActivityUserRow {
            email: mask(u.email),
            detail: u.created_at.date().to_string(),
            tags: {
                let mut tags = Vec::new();
                if u.is_demo {
                    tags.push("demo");
                }
                if u.email_verified_at.is_none() && u.password_hash.is_some() {
                    tags.push("unverified");
                }
                if u.role != "user" {
                    tags.push("staff");
                }
                tags.join(", ")
            },
        })
        .collect();
    let locked = locked_raw
        .into_iter()
        .map(|u| ActivityUserRow {
            email: mask(u.email),
            detail: u
                .locked_until
                .map(|t| t.date().to_string())
                .unwrap_or_default(),
            tags: format!("{} failed logins", u.failed_login_count),
        })
        .collect();
    let ai_users = ai_raw
        .into_iter()
        .map(|row| AiUsageRow {
            email: mask(row.email.unwrap_or_else(|| "(deleted)".to_string())),
            calls: row.calls,
            cost_usd: if admin_readonly {
                "—".to_string()
            } else {
                micro_to_usd(row.cost_micro_usd)
            },
        })
        .collect();
    Ok((
        jar,
        render(ActivityTemplate {
            theme: ui.theme.clone(),
            user_email: user.email,
            csrf_token,
            ui,
            is_admin: true,
            admin_readonly,
            open_reports,
            signups,
            locked,
            ai_users,
        })?,
    )
        .into_response())
}
