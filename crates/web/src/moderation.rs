//! Content moderation: the feed blocklist enforcement helper, user abuse
//! reports, and admin takedown/block actions with an audit trail
//! (docs/PUBLIC_LAUNCH_PLAN.md §4.6, §5, FND-192).

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use rustss_core::blocklist::BlockKind;
use rustss_db::{abuse_reports, articles, feed_blocklist, feeds, moderation, User};

use crate::auth::csrf;
use crate::auth::extractor::{AdminUser, AuthUser};
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

/// Reject a feed URL that is on the blocklist. Called immediately after
/// `validate_url` at every add path so blocked feeds cannot be subscribed to.
pub async fn assert_feed_allowed(state: &AppState, normalized_url: &str) -> Result<(), AppError> {
    if let Some(reason) = state.blocklist().await?.blocked_reason(normalized_url) {
        return Err(AppError::Validation(format!(
            "That feed is blocked by an administrator: {reason}"
        )));
    }
    Ok(())
}

// ── User abuse reports ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ReportForm {
    csrf_token: String,
    #[serde(default)]
    reason: String,
}

fn clean_reason(raw: &str) -> Result<String, AppError> {
    let reason = raw.trim();
    if reason.is_empty() || reason.chars().count() > 500 {
        return Err(AppError::Validation(
            "Please describe the problem in 1–500 characters.".to_string(),
        ));
    }
    Ok(reason.to_string())
}

/// `POST /articles/{id}/report` — flag a shared article for admin review.
pub async fn report_article(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ReportForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !state
        .feed_add_limiter
        .check(&format!("report:{}", user.id))
        .await
    {
        return Err(AppError::RateLimited);
    }
    let reason = clean_reason(&form.reason)?;
    if !articles::user_can_access(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    abuse_reports::create(
        state.db.pool(),
        user.id,
        "article",
        id,
        Some(&article.url),
        &reason,
    )
    .await?;
    Ok(Redirect::to(&format!("/articles/{id}?reported=1")).into_response())
}

/// `POST /feeds/{id}/report` — flag a shared feed for admin review.
pub async fn report_feed(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ReportForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !state
        .feed_add_limiter
        .check(&format!("report:{}", user.id))
        .await
    {
        return Err(AppError::RateLimited);
    }
    let reason = clean_reason(&form.reason)?;
    let feed = feeds::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    abuse_reports::create(
        state.db.pool(),
        user.id,
        "feed",
        id,
        Some(&feed.url),
        &reason,
    )
    .await?;
    Ok(Redirect::to("/feeds?reported=1").into_response())
}

// ── Admin: blocklist management ───────────────────────────────────────────────

#[derive(Template)]
#[template(path = "admin_blocklist.html")]
struct BlocklistTemplate {
    theme: String,
    user_email: String,
    ui: prefs::Preferences,
    is_admin: bool,
    admin_readonly: bool,
    csrf_token: String,
    error: Option<String>,
    notice: Option<String>,
    entries: Vec<BlocklistEntryRow>,
    reports: Vec<AbuseReportRow>,
    audit: Vec<AuditRow>,
}

struct BlocklistEntryRow {
    id: String,
    kind: String,
    pattern: String,
    reason: String,
    source: String,
}

struct AbuseReportRow {
    id: String,
    target_kind: String,
    target_id: String,
    target_url: String,
    reason: String,
    status: String,
    reporter: String,
}

struct AuditRow {
    actor: String,
    action: String,
    target: String,
    reason: String,
    when: String,
}

async fn render_blocklist(
    state: &AppState,
    user: &User,
    jar: CookieJar,
    error: Option<String>,
    notice: Option<String>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let entries = feed_blocklist::list(state.db.pool())
        .await?
        .into_iter()
        .map(|row| BlocklistEntryRow {
            id: row.id.to_string(),
            kind: row.kind,
            pattern: row.pattern,
            reason: row.reason.unwrap_or_default(),
            source: row.source,
        })
        .collect();
    let reports = abuse_reports::list_active(state.db.pool(), 200)
        .await?
        .into_iter()
        .map(|row| AbuseReportRow {
            id: row.id.to_string(),
            target_kind: row.target_kind,
            target_id: row.target_id.to_string(),
            target_url: row.target_url.unwrap_or_default(),
            reason: row.reason,
            status: row.status,
            reporter: row
                .reporter_email
                .unwrap_or_else(|| "(deleted)".to_string()),
        })
        .collect();
    let audit = moderation::recent(state.db.pool(), 50)
        .await?
        .into_iter()
        .map(|row| AuditRow {
            actor: row.actor_email.unwrap_or_else(|| "(system)".to_string()),
            action: row.action,
            target: row.target.unwrap_or_default(),
            reason: row.reason.unwrap_or_default(),
            when: row.created_at.date().to_string(),
        })
        .collect();
    let body = BlocklistTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        ui,
        is_admin: true,
        admin_readonly: false,
        csrf_token,
        error,
        notice,
        entries,
        reports,
        audit,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, Html(body)).into_response())
}

/// `GET /admin/feeds/blocklist` — the blocklist + abuse queue + audit.
pub async fn blocklist_page(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render_blocklist(&state, &user, jar, None, None).await
}

#[derive(Deserialize)]
pub struct BlocklistAddForm {
    csrf_token: String,
    kind: String,
    pattern: String,
    #[serde(default)]
    reason: String,
}

/// `POST /admin/feeds/blocklist` — add a domain/URL block and disable matches.
pub async fn blocklist_add(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Form(form): Form<BlocklistAddForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let Some(kind) = BlockKind::parse(form.kind.trim()) else {
        return render_blocklist(
            &state,
            &user,
            jar,
            Some("Choose domain or URL.".into()),
            None,
        )
        .await;
    };
    let pattern = match kind {
        BlockKind::Domain => rustss_core::blocklist::normalize_host(&form.pattern),
        BlockKind::Url => form.pattern.trim().to_string(),
    };
    if pattern.is_empty() || pattern.chars().count() > 400 {
        return render_blocklist(
            &state,
            &user,
            jar,
            Some("Enter a valid pattern.".into()),
            None,
        )
        .await;
    }
    let reason = form.reason.trim();
    let reason = (!reason.is_empty()).then_some(reason);
    feed_blocklist::add(
        state.db.pool(),
        kind.as_str(),
        &pattern,
        reason,
        "admin",
        Some(user.id),
    )
    .await?;
    // Disable already-subscribed feeds that now match.
    let compiled = feed_blocklist::compiled(state.db.pool()).await?;
    let mut disabled = 0u32;
    for (feed_id, url) in feed_blocklist::all_feed_urls(state.db.pool()).await? {
        if compiled.is_blocked(&url) && feeds::set_blocked(state.db.pool(), feed_id, reason).await?
        {
            disabled += 1;
        }
    }
    moderation::record(
        state.db.pool(),
        Some(user.id),
        "blocklist.add",
        Some(kind.as_str()),
        Some(&pattern),
        reason,
        json!({ "feeds_disabled": disabled }),
    )
    .await?;
    state.invalidate_blocklist();
    render_blocklist(
        &state,
        &user,
        jar,
        None,
        Some(format!(
            "Blocked “{pattern}”. {disabled} matching shared feed(s) disabled."
        )),
    )
    .await
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

/// `POST /admin/feeds/blocklist/{id}/delete` — remove a blocklist entry.
pub async fn blocklist_remove(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let entry = feed_blocklist::find_by_id(state.db.pool(), id).await?;
    if feed_blocklist::remove(state.db.pool(), id).await? {
        if let Some(entry) = entry {
            moderation::record(
                state.db.pool(),
                Some(user.id),
                "blocklist.remove",
                Some(&entry.kind),
                Some(&entry.pattern),
                None,
                json!({}),
            )
            .await?;
        }
        state.invalidate_blocklist();
    }
    render_blocklist(
        &state,
        &user,
        jar,
        None,
        Some("Removed the blocklist entry.".into()),
    )
    .await
}

// ── Admin: takedown / block actions ───────────────────────────────────────────

/// `POST /admin/feeds/{id}/block` — block a shared feed by its domain and
/// disable it everywhere.
pub async fn block_feed(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let feed = feeds::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let host = url::Url::parse(&feed.url)
        .ok()
        .and_then(|u| u.host_str().map(rustss_core::blocklist::normalize_host))
        .unwrap_or_default();
    let reason = "blocked by an administrator";
    if !host.is_empty() {
        feed_blocklist::add(
            state.db.pool(),
            "domain",
            &host,
            Some(reason),
            "admin",
            Some(user.id),
        )
        .await?;
        state.invalidate_blocklist();
    }
    feeds::set_blocked(state.db.pool(), id, Some(reason)).await?;
    moderation::record(
        state.db.pool(),
        Some(user.id),
        "feed.block",
        Some("feed"),
        Some(&feed.url),
        Some(reason),
        json!({ "feed_id": id, "domain": host }),
    )
    .await?;
    Ok((jar, Redirect::to("/admin/feeds")).into_response())
}

/// `POST /admin/articles/{id}/takedown` — delete a shared article everywhere and
/// block its URL so it cannot re-ingest.
pub async fn takedown_article(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let article = articles::find_by_id(state.db.pool(), id)
        .await?
        .ok_or(AppError::NotFound)?;
    let reason = "taken down by an administrator";
    feed_blocklist::add(
        state.db.pool(),
        "url",
        &article.url,
        Some(reason),
        "admin",
        Some(user.id),
    )
    .await?;
    state.invalidate_blocklist();
    articles::delete_by_id(state.db.pool(), id).await?;
    moderation::record(
        state.db.pool(),
        Some(user.id),
        "article.takedown",
        Some("article"),
        Some(&article.url),
        Some(reason),
        json!({ "article_id": id, "title": article.title }),
    )
    .await?;
    Ok((jar, Redirect::to("/admin/feeds/blocklist")).into_response())
}

#[derive(Deserialize)]
pub struct ResolveForm {
    csrf_token: String,
    status: String,
}

/// `POST /admin/abuse/{id}/resolve` — resolve or dismiss an abuse report.
pub async fn resolve_report(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ResolveForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let status = match form.status.as_str() {
        "resolved" | "dismissed" | "reviewing" => form.status.as_str(),
        _ => return Err(AppError::Validation("Invalid status.".into())),
    };
    abuse_reports::set_status(state.db.pool(), id, user.id, status).await?;
    moderation::record(
        state.db.pool(),
        Some(user.id),
        "report.resolve",
        Some("report"),
        Some(&id.to_string()),
        Some(status),
        json!({}),
    )
    .await?;
    Ok((jar, Redirect::to("/admin/feeds/blocklist")).into_response())
}
