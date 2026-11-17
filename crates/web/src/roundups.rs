//! User-triggered, shared feed/folder Roundups plus personal schedules/history.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use rusty_feed_core::html::markdown_to_sanitized_html;
use rusty_feed_core::roundup::{
    select_candidates, RoundupKey, RoundupPeriod, RoundupTiming, ROUNDUP_DEFAULT_LOCAL_HOUR,
    ROUNDUP_LLM_CALL_WEIGHT, ROUNDUP_MIN_ARTICLES,
};
use rusty_feed_db::{
    categories, feeds, jobs, llm_profiles, roundup_runs, roundup_schedules, roundups, subscriptions,
};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

#[derive(Template)]
#[template(path = "roundup.html")]
struct RoundupTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    scope_label: String,
    back_url: String,
    action: String,
    schedule_url: String,
    period: String,
    period_label: String,
    result: Option<RoundupView>,
    notice: Option<String>,
    show_ai_settings_link: bool,
    status_url: Option<String>,
    status_message: Option<String>,
}

struct RoundupView {
    headline: String,
    body_html: String,
    sources: Vec<SourceView>,
    created: String,
    model: String,
}

struct SourceView {
    number: usize,
    title: String,
    feed_title: String,
    internal_url: String,
    original_url: Option<String>,
    published: Option<String>,
}

struct ScopeDetails {
    label: String,
    back_url: String,
}

#[derive(Deserialize)]
pub struct RoundupForm {
    csrf_token: String,
    period: String,
}

#[derive(Deserialize, Default)]
pub struct RoundupQuery {
    #[serde(default)]
    period: Option<String>,
    #[serde(default)]
    input_hash: Option<String>,
    #[serde(default)]
    model_version: Option<String>,
    #[serde(default)]
    notice: Option<String>,
}

#[derive(Serialize)]
pub struct RoundupStatus {
    state: &'static str,
    message: &'static str,
}

enum Preparation {
    Ready(RoundupKey, Vec<Uuid>),
    TooFew,
    NoProvider,
}

async fn scope_details(
    state: &AppState,
    user_id: Uuid,
    scope: roundups::RoundupScope,
) -> Result<ScopeDetails, AppError> {
    match scope {
        roundups::RoundupScope::Feed(feed_id) => {
            if subscriptions::find(state.db.pool(), user_id, feed_id)
                .await?
                .is_none()
            {
                return Err(AppError::NotFound);
            }
            let feed = feeds::find_by_id(state.db.pool(), feed_id)
                .await?
                .ok_or(AppError::NotFound)?;
            Ok(ScopeDetails {
                label: feed.name,
                back_url: format!("/articles?feed={feed_id}&unread=0"),
            })
        }
        roundups::RoundupScope::Folder(folder_id) => {
            let folder = categories::list_for_user(state.db.pool(), user_id)
                .await?
                .into_iter()
                .find(|folder| folder.id == folder_id)
                .ok_or(AppError::NotFound)?;
            Ok(ScopeDetails {
                label: format!("Folder: {}", folder.name),
                back_url: format!("/articles?category={folder_id}&unread=0"),
            })
        }
    }
}

async fn selected_model(state: &AppState, user_id: Uuid) -> Result<Option<String>, AppError> {
    if !state.settings.summarization_enabled {
        return Ok(None);
    }
    if let Some(profile) = llm_profiles::resolve(state.db.pool(), Some(user_id))
        .await?
        .into_iter()
        .next()
    {
        return Ok(Some(profile.model));
    }
    Ok(state
        .settings
        .env_llm
        .as_ref()
        .map(|provider| provider.model.clone()))
}

async fn prepare(
    state: &AppState,
    user_id: Uuid,
    scope: roundups::RoundupScope,
    period: RoundupPeriod,
) -> Result<Preparation, AppError> {
    let candidates = roundups::candidates_for_user(state.db.pool(), user_id, scope, period).await?;
    let selected = select_candidates(candidates, period, OffsetDateTime::now_utc());
    if selected.len() < ROUNDUP_MIN_ARTICLES {
        return Ok(Preparation::TooFew);
    }
    let Some(model_version) = selected_model(state, user_id).await? else {
        return Ok(Preparation::NoProvider);
    };
    let ids: Vec<_> = selected.iter().map(|candidate| candidate.id).collect();
    Ok(Preparation::Ready(
        RoundupKey::new(&ids, period, &model_version),
        ids,
    ))
}

fn period(value: Option<&str>) -> Result<RoundupPeriod, AppError> {
    RoundupPeriod::parse(value.unwrap_or("day"))
        .ok_or_else(|| AppError::Validation("Choose either Day or Week.".to_string()))
}

fn key_from_query(
    query: &RoundupQuery,
    period: RoundupPeriod,
) -> Result<Option<RoundupKey>, AppError> {
    let (Some(input_hash), Some(model_version)) = (&query.input_hash, &query.model_version) else {
        return Ok(None);
    };
    if input_hash.len() != 64
        || !input_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        || model_version.trim().is_empty()
        || model_version.chars().count() > 200
    {
        return Err(AppError::Validation(
            "This Roundup link is invalid. Choose Day or Week to create a new one.".to_string(),
        ));
    }
    Ok(Some(RoundupKey {
        input_hash: input_hash.to_ascii_lowercase(),
        period,
        model_version: model_version.clone(),
    }))
}

fn roundup_url(
    scope: roundups::RoundupScope,
    period: RoundupPeriod,
    key: Option<&RoundupKey>,
    notice: Option<&str>,
) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("period", period.as_str());
    if let Some(key) = key {
        query.append_pair("input_hash", &key.input_hash);
        query.append_pair("model_version", &key.model_version);
    }
    if let Some(notice) = notice {
        query.append_pair("notice", notice);
    }
    format!(
        "/roundups/{}/{}?{}",
        scope.as_str(),
        scope.id(),
        query.finish()
    )
}

fn notice_for(code: Option<&str>, period: RoundupPeriod) -> (Option<String>, bool) {
    match code {
        Some("too-few") => (
            Some(format!(
                "A Roundup needs at least {ROUNDUP_MIN_ARTICLES} articles from the last {}. Try the {} option or check back after new articles arrive.",
                if period == RoundupPeriod::Day {
                    "24 hours"
                } else {
                    "7 days"
                },
                if period == RoundupPeriod::Day { "Week" } else { "Day" }
            )),
            false,
        ),
        Some("unavailable") => (
            Some(
                "No AI provider is configured. Add one under Account → AI providers or ask an administrator."
                    .to_string(),
            ),
            true,
        ),
        Some("budget") => (
            Some("You've reached today's AI call limit or the shared AI budget is used up. Try again tomorrow, or add your own API key for instance-budget exemptions.".to_string()),
            true,
        ),
        Some("rate-limited") => (
            Some("Too many AI requests in a short time. Wait a minute, then try again.".to_string()),
            false,
        ),
        _ => (None, false),
    }
}

/// `POST /roundups/{kind}/{id}` — select shared inputs, enforce all gates, and
/// enqueue one deduplicated job on the interactive lane.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path((kind, id)): Path<(String, Uuid)>,
    Form(form): Form<RoundupForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let period = period(Some(&form.period))?;
    let scope = roundups::RoundupScope::parse(&kind, id).ok_or(AppError::NotFound)?;
    start_roundup(&state, &user, scope, period).await
}

async fn start_roundup(
    state: &AppState,
    user: &rusty_feed_db::User,
    scope: roundups::RoundupScope,
    period: RoundupPeriod,
) -> Result<Response, AppError> {
    if !state.summarize_limiter.check(&user.id.to_string()).await {
        return Ok(
            Redirect::to(&roundup_url(scope, period, None, Some("rate-limited"))).into_response(),
        );
    }
    crate::verify::deny_demo(user)?;
    crate::verify::require_verified(state, user)?;
    scope_details(state, user.id, scope).await?;

    let (key, article_ids) = match prepare(state, user.id, scope, period).await? {
        Preparation::Ready(key, ids) => (key, ids),
        Preparation::TooFew => {
            record_manual(
                state,
                user.id,
                scope,
                period,
                "skipped",
                None,
                None,
                Some("Fewer than 3 articles were available in this window."),
            )
            .await?;
            return Ok(
                Redirect::to(&roundup_url(scope, period, None, Some("too-few"))).into_response(),
            );
        }
        Preparation::NoProvider => {
            record_manual(
                state,
                user.id,
                scope,
                period,
                "skipped",
                None,
                None,
                Some("No AI provider is configured for this Roundup."),
            )
            .await?;
            return Ok(
                Redirect::to(&roundup_url(scope, period, None, Some("unavailable")))
                    .into_response(),
            );
        }
    };
    let destination = roundup_url(scope, period, Some(&key), None);
    if let Some(stored) = roundups::find_by_key(state.db.pool(), &key).await? {
        record_manual(
            state,
            user.id,
            scope,
            period,
            "ready",
            Some(&key),
            Some(stored.id),
            None,
        )
        .await?;
        return Ok(Redirect::to(&destination).into_response());
    }
    if jobs::latest_by_dedup(state.db.pool(), &key.dedup_key())
        .await?
        .is_some_and(|job| matches!(job.status.as_str(), "pending" | "running"))
    {
        record_manual(
            state,
            user.id,
            scope,
            period,
            "queued",
            Some(&key),
            None,
            None,
        )
        .await?;
        return Ok(Redirect::to(&destination).into_response());
    }
    match crate::reader::llm_budget_gate(state, user.id, ROUNDUP_LLM_CALL_WEIGHT).await {
        Ok(()) => {}
        Err(AppError::RateLimited) => {
            record_manual(
                state,
                user.id,
                scope,
                period,
                "skipped",
                Some(&key),
                None,
                Some("Today's AI budget was used up; no generation was started."),
            )
            .await?;
            return Ok(
                Redirect::to(&roundup_url(scope, period, Some(&key), Some("budget")))
                    .into_response(),
            );
        }
        Err(error) => return Err(error),
    }

    jobs::enqueue_in_lane(
        state.db.pool(),
        jobs::QueueLane::Interactive,
        "roundup",
        serde_json::json!({
            "input_hash": key.input_hash,
            "period": key.period.as_str(),
            "model_version": key.model_version,
            "article_ids": article_ids,
            "user_id": user.id,
        }),
        Some(&key.dedup_key()),
        None,
        3,
    )
    .await?;
    record_manual(
        state,
        user.id,
        scope,
        period,
        "queued",
        Some(&key),
        None,
        None,
    )
    .await?;
    Ok(Redirect::to(&destination).into_response())
}

#[allow(clippy::too_many_arguments)]
async fn record_manual(
    state: &AppState,
    user_id: Uuid,
    scope: roundups::RoundupScope,
    period: RoundupPeriod,
    status: &str,
    key: Option<&RoundupKey>,
    roundup_id: Option<Uuid>,
    reason: Option<&str>,
) -> Result<(), AppError> {
    roundup_runs::record(
        state.db.pool(),
        roundup_runs::NewRoundupRun {
            user_id,
            schedule_id: None,
            scope,
            period,
            status,
            key,
            roundup_id,
            skip_reason: reason,
            scheduled_for: None,
        },
    )
    .await?;
    Ok(())
}

/// `GET /roundups/{kind}/{id}` — render a cached result or live job progress.
pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path((kind, id)): Path<(String, Uuid)>,
    Query(query): Query<RoundupQuery>,
) -> Result<Response, AppError> {
    let period = period(query.period.as_deref())?;
    let scope = roundups::RoundupScope::parse(&kind, id).ok_or(AppError::NotFound)?;
    let details = scope_details(&state, user.id, scope).await?;
    let key = key_from_query(&query, period)?;
    let (mut notice, mut show_ai_settings_link) = notice_for(query.notice.as_deref(), period);
    let mut key = key;
    if key.is_none() && notice.is_none() {
        match prepare(&state, user.id, scope, period).await? {
            Preparation::Ready(prepared, _) => key = Some(prepared),
            Preparation::TooFew => {
                (notice, show_ai_settings_link) = notice_for(Some("too-few"), period);
            }
            Preparation::NoProvider => {
                (notice, show_ai_settings_link) = notice_for(Some("unavailable"), period);
            }
        }
    }

    let stored = match &key {
        Some(key) => roundups::find_by_key(state.db.pool(), key).await?,
        None => None,
    };
    let ui = prefs::load(&state.db, user.id).await;
    let result = match stored {
        Some(stored) => {
            if let Some(key) = &key {
                roundup_runs::mark_ready_for_key(state.db.pool(), user.id, key, stored.id).await?;
            }
            let sources = roundups::sources_by_ids(state.db.pool(), &stored.source_ids)
                .await?
                .into_iter()
                .map(|source| SourceView {
                    number: source.position as usize,
                    internal_url: format!("/articles/{}", source.id),
                    original_url: crate::reader::safe_external_url(Some(&source.url)),
                    published: source.published_at.map(|value| ui.format_date(value)),
                    title: source.title,
                    feed_title: source.feed_title,
                })
                .collect();
            Some(RoundupView {
                headline: stored.headline,
                body_html: markdown_to_sanitized_html(&stored.body_markdown),
                sources,
                created: ui.format_date(stored.created_at),
                model: stored.model,
            })
        }
        None => None,
    };
    let job = match (&key, &result) {
        (Some(key), None) => jobs::latest_by_dedup(state.db.pool(), &key.dedup_key()).await?,
        _ => None,
    };
    let (state_name, status_message) = progress(job);
    let status_url = if matches!(state_name, "queued" | "running" | "retrying") {
        key.as_ref()
            .map(|key| roundup_status_url(scope, key, period))
    } else {
        None
    };
    let status_message = if key.is_some() && result.is_none() {
        Some(status_message.to_string())
    } else {
        None
    };
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = RoundupTemplate {
        theme: ui.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui,
        is_admin: user.role == "admin" || user.role == "admin_readonly",
        scope_label: details.label,
        back_url: details.back_url,
        action: format!("/roundups/{}/{}", scope.as_str(), scope.id()),
        schedule_url: format!(
            "/roundups?scope={}:{}&period={}&repeat=1#new-roundup",
            scope.as_str(),
            scope.id(),
            period.as_str(),
        ),
        period: period.as_str().to_string(),
        period_label: if period == RoundupPeriod::Day {
            "Last 24 hours".to_string()
        } else {
            "Last 7 days".to_string()
        },
        result,
        notice,
        show_ai_settings_link,
        status_url,
        status_message,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    let mut response = (jar, body).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}

fn roundup_status_url(
    scope: roundups::RoundupScope,
    key: &RoundupKey,
    period: RoundupPeriod,
) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("period", period.as_str())
        .append_pair("input_hash", &key.input_hash)
        .append_pair("model_version", &key.model_version)
        .finish();
    format!("/roundups/{}/{}/status?{query}", scope.as_str(), scope.id())
}

fn progress(job: Option<jobs::JobStatus>) -> (&'static str, &'static str) {
    match job {
        Some(job) if job.status == "running" => ("running", "Writing your Roundup now…"),
        Some(job) if job.status == "pending" && job.attempts > 0 => (
            "retrying",
            "The first attempt failed. Trying again shortly…",
        ),
        Some(job) if job.status == "pending" => ("queued", "Waiting for its turn…"),
        Some(job) if job.status == "dead" || job.status == "failed" => (
            "failed",
            "The Roundup could not be generated. Choose Day or Week to try again. An administrator can see the error details.",
        ),
        Some(_) => (
            "failed",
            "The Roundup finished without usable content. Choose Day or Week to try again.",
        ),
        None => (
            "idle",
            "No Roundup has been generated for this exact set yet. Choose Day or Week to begin.",
        ),
    }
}

/// `GET /roundups/{kind}/{id}/status` — safe live progress without payloads or
/// provider error details.
pub async fn status(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kind, id)): Path<(String, Uuid)>,
    Query(query): Query<RoundupQuery>,
) -> Result<axum::Json<RoundupStatus>, AppError> {
    let period = period(query.period.as_deref())?;
    let scope = roundups::RoundupScope::parse(&kind, id).ok_or(AppError::NotFound)?;
    scope_details(&state, user.id, scope).await?;
    let key = key_from_query(&query, period)?
        .ok_or_else(|| AppError::Validation("The Roundup status key is missing.".to_string()))?;
    if roundups::find_by_key(state.db.pool(), &key)
        .await?
        .is_some()
    {
        return Ok(axum::Json(RoundupStatus {
            state: "ready",
            message: "Roundup ready.",
        }));
    }
    let job = jobs::latest_by_dedup(state.db.pool(), &key.dedup_key()).await?;
    let (state, message) = progress(job);
    Ok(axum::Json(RoundupStatus { state, message }))
}

#[derive(Template)]
#[template(path = "roundups.html")]
struct RoundupsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    form: ManagerFormView,
    feed_options: Vec<ScopeOption>,
    folder_options: Vec<ScopeOption>,
    weekday_options: Vec<WeekdayOption>,
    schedules: Vec<ScheduleView>,
    history: Vec<HistoryView>,
    schedule_count: i64,
    schedule_limit: u32,
    notice: Option<String>,
    error: Option<String>,
    smtp_enabled: bool,
}

struct ManagerFormView {
    action: String,
    heading: String,
    scope: String,
    period: String,
    local_time: String,
    repeat: bool,
    email_delivery: bool,
    submit_label: String,
    editing: bool,
}

struct ScopeOption {
    value: String,
    label: String,
    selected: bool,
}

struct WeekdayOption {
    value: u8,
    label: &'static str,
    selected: bool,
}

struct ScheduleView {
    scope_label: String,
    cadence: String,
    next_run: String,
    last_run: String,
    status: String,
    reason: Option<String>,
    email_label: &'static str,
    enabled: bool,
    toggle_label: &'static str,
    edit_url: String,
    toggle_url: String,
    delete_url: String,
}

struct HistoryView {
    title: String,
    scope_label: String,
    period_label: &'static str,
    created: String,
    status: &'static str,
    reason: Option<String>,
    url: Option<String>,
    repeating: bool,
}

#[derive(Default, Deserialize)]
pub struct RoundupsQuery {
    scope: Option<String>,
    period: Option<String>,
    repeat: Option<String>,
    edit: Option<Uuid>,
    notice: Option<String>,
}

#[derive(Deserialize)]
pub struct RoundupManagerForm {
    csrf_token: String,
    scope: String,
    period: String,
    local_time: Option<String>,
    weekday: Option<u8>,
    repeat: Option<String>,
    email_delivery: Option<String>,
}

#[derive(Deserialize)]
pub struct ScheduleActionForm {
    csrf_token: String,
}

/// `GET /roundups` — create one-off or repeating Roundups and inspect personal
/// schedules/history without exposing shared generation internals.
pub async fn index(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Query(query): Query<RoundupsQuery>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let editing = match query.edit {
        Some(id) => Some(
            roundup_schedules::find_for_user(state.db.pool(), user.id, id)
                .await?
                .ok_or(AppError::NotFound)?,
        ),
        None => None,
    };
    let form = editing.as_ref().map_or_else(
        || ManagerFormView {
            action: "/roundups".to_string(),
            heading: "New Roundup".to_string(),
            scope: query.scope.clone().unwrap_or_default(),
            period: query
                .period
                .as_deref()
                .and_then(RoundupPeriod::parse)
                .unwrap_or(RoundupPeriod::Day)
                .as_str()
                .to_string(),
            local_time: format!("{ROUNDUP_DEFAULT_LOCAL_HOUR:02}:00"),
            repeat: query
                .repeat
                .as_deref()
                .is_some_and(|value| matches!(value, "1" | "true" | "on" | "yes")),
            email_delivery: false,
            submit_label: "Create roundup".to_string(),
            editing: false,
        },
        |schedule| ManagerFormView {
            action: format!("/roundups/schedules/{}", schedule.id),
            heading: "Edit repeating Roundup".to_string(),
            scope: format!("{}:{}", schedule.scope_kind, schedule.scope_id),
            period: schedule.period.clone(),
            local_time: format!("{:02}:00", schedule.local_hour),
            repeat: true,
            email_delivery: schedule.email_delivery,
            submit_label: "Save schedule".to_string(),
            editing: true,
        },
    );
    let (notice, error) = manager_notice(
        query.notice.as_deref(),
        state.settings.roundup_schedule_limit,
    );
    render_roundups(&state, &user, jar, ui, form, notice, error).await
}

fn manager_notice(code: Option<&str>, limit: u32) -> (Option<String>, Option<String>) {
    let notice = match code {
        Some("created") => Some("Your repeating Roundup is scheduled.".to_string()),
        Some("updated") => Some("Schedule changes saved.".to_string()),
        Some("deleted") => {
            Some("The schedule was deleted. Past Roundups remain in History.".to_string())
        }
        Some("enabled") => Some("The schedule is active again.".to_string()),
        Some("disabled") => Some("The schedule is paused. No new Roundups will start.".to_string()),
        _ => None,
    };
    let error = match code {
        Some("cap") => Some(format!(
            "You already have {limit} schedules. Edit or delete one before adding another."
        )),
        Some("conflict") => {
            Some("That Feed or Folder already has a repeating Roundup for this period.".to_string())
        }
        _ => None,
    };
    (notice, error)
}

async fn render_roundups(
    state: &AppState,
    user: &rusty_feed_db::User,
    jar: CookieJar,
    ui: prefs::Preferences,
    mut form: ManagerFormView,
    notice: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let subscriptions = subscriptions::list_for_user(state.db.pool(), user.id).await?;
    let custom_names: HashMap<_, _> = subscriptions
        .into_iter()
        .map(|subscription| (subscription.feed_id, subscription.custom_name))
        .collect();
    let mut labels = HashMap::new();
    let mut feed_options = Vec::new();
    for feed in feeds::list_for_user(state.db.pool(), user.id).await? {
        let value = format!("feed:{}", feed.id);
        let label = custom_names
            .get(&feed.id)
            .and_then(|name| name.clone())
            .unwrap_or(feed.name);
        labels.insert(value.clone(), label.clone());
        feed_options.push(ScopeOption {
            selected: form.scope == value,
            value,
            label,
        });
    }
    let folders = categories::list_for_user(state.db.pool(), user.id).await?;
    let mut folder_options = Vec::new();
    for folder in folders {
        let value = format!("folder:{}", folder.id);
        labels.insert(value.clone(), format!("Folder: {}", folder.name));
        folder_options.push(ScopeOption {
            selected: form.scope == value,
            value,
            label: folder.name,
        });
    }
    if !labels.contains_key(&form.scope) {
        form.scope = feed_options
            .first()
            .map(|option| option.value.clone())
            .or_else(|| folder_options.first().map(|option| option.value.clone()))
            .unwrap_or_default();
        for option in feed_options.iter_mut().chain(folder_options.iter_mut()) {
            option.selected = option.value == form.scope;
        }
    }

    let schedules = roundup_schedules::list_for_user(state.db.pool(), user.id).await?;
    let schedule_count = schedules.len() as i64;
    let schedule_views = schedules
        .iter()
        .map(|schedule| {
            let timing = schedule.timing();
            let period = schedule.roundup_period().unwrap_or(RoundupPeriod::Day);
            let cadence = match (period, timing.as_ref().and_then(|timing| timing.weekday)) {
                (RoundupPeriod::Week, Some(weekday)) => format!(
                    "Every {} at {:02}:00 · {}",
                    weekday_name(weekday),
                    schedule.local_hour,
                    schedule.timezone,
                ),
                _ => format!(
                    "Every day at {:02}:00 · {}",
                    schedule.local_hour, schedule.timezone,
                ),
            };
            let next_run = if !schedule.enabled {
                "Paused".to_string()
            } else {
                schedule
                    .next_run()
                    .map_or("Not available".to_string(), |next| {
                        rusty_feed_core::i18n::format_date_time(next, &schedule.timezone)
                    })
            };
            let last_run = schedule.last_run_at.map_or("Never".to_string(), |last| {
                rusty_feed_core::i18n::format_date_time(last, &schedule.timezone)
            });
            ScheduleView {
                scope_label: labels
                    .get(&format!("{}:{}", schedule.scope_kind, schedule.scope_id))
                    .cloned()
                    .unwrap_or_else(|| "Unavailable scope".to_string()),
                cadence,
                next_run,
                last_run,
                status: schedule
                    .last_status
                    .as_deref()
                    .map_or("Not run yet", status_label)
                    .to_string(),
                reason: schedule.last_skip_reason.clone(),
                email_label: if schedule.email_delivery {
                    "Email on"
                } else {
                    "Email off"
                },
                enabled: schedule.enabled,
                toggle_label: if schedule.enabled { "Pause" } else { "Resume" },
                edit_url: format!("/roundups?edit={}#new-roundup", schedule.id),
                toggle_url: format!("/roundups/schedules/{}/toggle", schedule.id),
                delete_url: format!("/roundups/schedules/{}/delete", schedule.id),
            }
        })
        .collect();
    let history = roundup_runs::list_for_user(state.db.pool(), user.id, 30)
        .await?
        .into_iter()
        .map(|run| {
            let period = run.roundup_period().unwrap_or(RoundupPeriod::Day);
            let url = run.key().and_then(|key| {
                run.scope()
                    .map(|scope| roundup_url(scope, period, Some(&key), None))
            });
            HistoryView {
                title: run.roundup_headline.clone().unwrap_or_else(|| {
                    if period == RoundupPeriod::Day {
                        "Day Roundup".to_string()
                    } else {
                        "Week Roundup".to_string()
                    }
                }),
                scope_label: labels
                    .get(&format!("{}:{}", run.scope_kind, run.scope_id))
                    .cloned()
                    .unwrap_or_else(|| "Unavailable scope".to_string()),
                period_label: if period == RoundupPeriod::Day {
                    "Day"
                } else {
                    "Week"
                },
                created: rusty_feed_core::i18n::format_date_time(run.created_at, &ui.timezone),
                status: status_label(&run.status),
                reason: run.skip_reason,
                url,
                repeating: run.schedule_id.is_some(),
            }
        })
        .collect();
    let selected_weekday = editing_weekday(&form, &schedules);
    let weekday_options = weekday_options(selected_weekday);
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = RoundupsTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui,
        is_admin: user.role == "admin" || user.role == "admin_readonly",
        form,
        feed_options,
        folder_options,
        weekday_options,
        schedules: schedule_views,
        history,
        schedule_count,
        schedule_limit: state.settings.roundup_schedule_limit,
        notice,
        error,
        smtp_enabled: state.mailer.is_enabled(),
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    let mut response = (jar, body).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}

fn editing_weekday(form: &ManagerFormView, schedules: &[roundup_schedules::RoundupSchedule]) -> u8 {
    if form.editing {
        if let Some(schedule) = schedules
            .iter()
            .find(|schedule| form.action.ends_with(&schedule.id.to_string()))
        {
            if let Some(day) = schedule.weekday.and_then(|day| u8::try_from(day).ok()) {
                return day;
            }
        }
    }
    1
}

fn weekday_options(selected: u8) -> Vec<WeekdayOption> {
    [
        time::Weekday::Monday,
        time::Weekday::Tuesday,
        time::Weekday::Wednesday,
        time::Weekday::Thursday,
        time::Weekday::Friday,
        time::Weekday::Saturday,
        time::Weekday::Sunday,
    ]
    .into_iter()
    .map(|weekday| WeekdayOption {
        value: weekday.number_from_monday(),
        label: weekday_name(weekday),
        selected: weekday.number_from_monday() == selected,
    })
    .collect()
}

fn weekday_name(weekday: time::Weekday) -> &'static str {
    match weekday {
        time::Weekday::Monday => "Monday",
        time::Weekday::Tuesday => "Tuesday",
        time::Weekday::Wednesday => "Wednesday",
        time::Weekday::Thursday => "Thursday",
        time::Weekday::Friday => "Friday",
        time::Weekday::Saturday => "Saturday",
        time::Weekday::Sunday => "Sunday",
    }
}

fn status_label(status: &str) -> &'static str {
    match status {
        "ready" => "Ready",
        "skipped" => "Skipped",
        "failed" => "Failed",
        "queued" => "Waiting",
        "pending" => "Waiting",
        _ => "Waiting",
    }
}

fn parse_scope(value: &str) -> Result<roundups::RoundupScope, AppError> {
    let (kind, id) = value
        .split_once(':')
        .ok_or_else(|| AppError::Validation("Choose a Feed or Folder.".to_string()))?;
    let id = Uuid::parse_str(id)
        .map_err(|_| AppError::Validation("Choose a valid Feed or Folder.".to_string()))?;
    roundups::RoundupScope::parse(kind, id)
        .ok_or_else(|| AppError::Validation("Choose a Feed or Folder.".to_string()))
}

fn parse_local_hour(value: Option<&str>) -> Result<u8, AppError> {
    let value = value.unwrap_or("07:00");
    let (hour, minute) = value
        .split_once(':')
        .ok_or_else(|| AppError::Validation("Choose a whole hour, such as 07:00.".to_string()))?;
    if hour.len() != 2
        || minute.len() != 2
        || minute != "00"
        || !hour.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(AppError::Validation(
            "Choose a whole hour, such as 07:00.".to_string(),
        ));
    }
    hour.parse::<u8>()
        .ok()
        .filter(|hour| *hour <= 23)
        .ok_or_else(|| AppError::Validation("Choose an hour from 00:00 to 23:00.".to_string()))
}

fn schedule_timing(
    form: &RoundupManagerForm,
    timezone: String,
) -> Result<(roundups::RoundupScope, RoundupTiming), AppError> {
    let scope = parse_scope(&form.scope)?;
    let period = period(Some(&form.period))?;
    let local_hour = if period == RoundupPeriod::Day {
        parse_local_hour(form.local_time.as_deref())?
    } else {
        ROUNDUP_DEFAULT_LOCAL_HOUR
    };
    let weekday = if period == RoundupPeriod::Week {
        Some(
            rusty_feed_core::roundup::roundup_weekday(form.weekday.unwrap_or(1)).ok_or_else(
                || AppError::Validation("Choose a weekday for the weekly Roundup.".to_string()),
            )?,
        )
    } else {
        None
    };
    let timing = RoundupTiming {
        period,
        local_hour,
        weekday,
        timezone,
    };
    if !timing.is_valid() {
        return Err(AppError::Validation(
            "The schedule timing is not valid.".to_string(),
        ));
    }
    Ok((scope, timing))
}

/// `POST /roundups` — generate once or create a personal schedule.
pub async fn create_managed(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<RoundupManagerForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    crate::verify::deny_demo(&user)?;
    crate::verify::require_verified(&state, &user)?;
    let ui = prefs::load(&state.db, user.id).await;
    let (scope, timing) = schedule_timing(&form, ui.timezone)?;
    scope_details(&state, user.id, scope).await?;
    if form.repeat.is_none() {
        return start_roundup(&state, &user, scope, timing.period).await;
    }
    let outcome = roundup_schedules::create(
        state.db.pool(),
        roundup_schedules::NewRoundupSchedule {
            user_id: user.id,
            scope,
            timing: &timing,
            email_delivery: form.email_delivery.is_some(),
            enabled: true,
        },
        state.settings.roundup_schedule_limit,
    )
    .await?;
    let code = match outcome {
        roundup_schedules::CreateScheduleOutcome::Created(_) => "created",
        roundup_schedules::CreateScheduleOutcome::CapReached => "cap",
        roundup_schedules::CreateScheduleOutcome::Conflict => "conflict",
    };
    Ok(Redirect::to(&format!("/roundups?notice={code}#schedules")).into_response())
}

/// `POST /roundups/schedules/{id}` — replace one owned schedule.
pub async fn update_schedule(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<RoundupManagerForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    crate::verify::deny_demo(&user)?;
    crate::verify::require_verified(&state, &user)?;
    let existing = roundup_schedules::find_for_user(state.db.pool(), user.id, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let ui = prefs::load(&state.db, user.id).await;
    let (scope, timing) = schedule_timing(&form, ui.timezone)?;
    scope_details(&state, user.id, scope).await?;
    if roundup_schedules::list_for_user(state.db.pool(), user.id)
        .await?
        .into_iter()
        .any(|schedule| {
            schedule.id != id
                && schedule.scope_kind == scope.as_str()
                && schedule.scope_id == scope.id()
                && schedule.period == timing.period.as_str()
        })
    {
        return Ok(Redirect::to("/roundups?notice=conflict#schedules").into_response());
    }
    let updated = roundup_schedules::update(
        state.db.pool(),
        user.id,
        id,
        roundup_schedules::NewRoundupSchedule {
            user_id: user.id,
            scope,
            timing: &timing,
            email_delivery: form.email_delivery.is_some(),
            enabled: existing.enabled,
        },
    )
    .await?;
    if updated.is_none() {
        return Err(AppError::NotFound);
    }
    Ok(Redirect::to("/roundups?notice=updated#schedules").into_response())
}

/// `POST /roundups/schedules/{id}/toggle` — pause or resume one owned schedule.
pub async fn toggle_schedule(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ScheduleActionForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    crate::verify::deny_demo(&user)?;
    let schedule = roundup_schedules::find_for_user(state.db.pool(), user.id, id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !roundup_schedules::set_enabled(state.db.pool(), user.id, id, !schedule.enabled).await? {
        return Err(AppError::NotFound);
    }
    let notice = if schedule.enabled {
        "disabled"
    } else {
        "enabled"
    };
    Ok(Redirect::to(&format!("/roundups?notice={notice}#schedules")).into_response())
}

/// `POST /roundups/schedules/{id}/delete` — delete only personal scheduling;
/// existing shared content remains available in History.
pub async fn delete_schedule(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ScheduleActionForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    crate::verify::deny_demo(&user)?;
    if !roundup_schedules::delete(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    Ok(Redirect::to("/roundups?notice=deleted#schedules").into_response())
}
