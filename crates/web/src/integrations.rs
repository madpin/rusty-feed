//! Integration profiles, article delivery, signed webhooks, and history.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use rustss_db::{articles, integrations, jobs, subscriptions, User};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

struct ProviderView {
    id: String,
    name: String,
    capabilities: String,
}
struct ProfileView {
    id: String,
    name: String,
    provider: String,
    endpoint_host: String,
    credential_kind: String,
    events: String,
    tags: String,
    is_default: bool,
    paused: bool,
}
struct DeliveryView {
    event_type: String,
    status: String,
    attempts: i32,
    response_status: Option<i32>,
    error: Option<String>,
    created: String,
}

#[derive(Template)]
#[template(path = "integrations.html")]
struct IntegrationsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    providers: Vec<ProviderView>,
    profiles: Vec<ProfileView>,
    deliveries: Vec<DeliveryView>,
    notice: Option<String>,
    error: Option<String>,
    default_template: String,
}

#[derive(Template)]
#[template(path = "article_send.html")]
struct ArticleSendTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    article_id: String,
    article_title: String,
    profiles: Vec<ProfileView>,
    notice: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
pub struct ProfileForm {
    csrf_token: String,
    name: String,
    provider: String,
    endpoint_url: String,
    #[serde(default)]
    credential: String,
    credential_kind: String,
    payload_template: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    event_article_saved: Option<String>,
    #[serde(default)]
    event_article_bookmarked: Option<String>,
    #[serde(default)]
    event_new_entries: Option<String>,
    #[serde(default)]
    event_summary_ready: Option<String>,
    max_attempts: i32,
    rate_limit_per_minute: i32,
    #[serde(default)]
    is_default: Option<String>,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}
#[derive(Deserialize)]
pub struct ToggleForm {
    csrf_token: String,
    paused: bool,
}
#[derive(Deserialize)]
pub struct RotateForm {
    csrf_token: String,
    credential_kind: String,
    #[serde(default)]
    credential: String,
}
#[derive(Deserialize)]
pub struct SendForm {
    csrf_token: String,
    profile_id: String,
}

fn profile_views(rows: Vec<integrations::IntegrationProfile>) -> Vec<ProfileView> {
    rows.into_iter()
        .map(|profile| ProfileView {
            id: profile.id.to_string(),
            name: profile.name,
            provider: profile.provider,
            endpoint_host: url::Url::parse(&profile.endpoint_url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_string))
                .unwrap_or_else(|| "invalid".into()),
            credential_kind: profile.credential_kind,
            events: profile.event_types.join(", "),
            tags: profile.tags.join(", "),
            is_default: profile.is_default,
            paused: profile.paused_at.is_some() || !profile.enabled,
        })
        .collect()
}

async fn render_page(
    state: &AppState,
    user: &User,
    jar: CookieJar,
    notice: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let (profiles, deliveries) = tokio::try_join!(
        integrations::list_profiles(state.db.pool(), user.id),
        integrations::list_deliveries(state.db.pool(), user.id, 100),
    )?;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let deliveries = deliveries
        .into_iter()
        .map(|delivery| DeliveryView {
            event_type: delivery.event_type,
            status: delivery.status,
            attempts: delivery.attempt_count,
            response_status: delivery.response_status,
            error: delivery.error_summary,
            created: ui.format_date(delivery.created_at),
        })
        .collect();
    let body = IntegrationsTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui,
        is_admin: user.role == "admin",
        providers: rustss_core::integrations::PROVIDERS
            .iter()
            .map(|provider| ProviderView {
                id: provider.id.into(),
                name: provider.name.into(),
                capabilities: match (provider.saves_articles, provider.sends_notifications) {
                    (true, true) => "save + notify",
                    (true, false) => "save",
                    _ => "notify",
                }
                .into(),
            })
            .collect(),
        profiles: profile_views(profiles),
        deliveries,
        notice,
        error,
        default_template: "{{title}}\n{{url}}".to_string(),
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    Ok((jar, body).into_response())
}

pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render_page(&state, &user, jar, None, None).await
}

fn normalize_tags(value: &str) -> Result<Vec<String>, String> {
    let mut tags = Vec::new();
    for tag in value
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
    {
        if tag.chars().count() > 40 || tag.chars().any(char::is_control) {
            return Err("Tags must be at most 40 printable characters.".into());
        }
        if !tags.iter().any(|stored| stored == tag) {
            tags.push(tag.to_string());
        }
    }
    if tags.len() > 20 {
        return Err("Use at most 20 destination tags.".into());
    }
    Ok(tags)
}

pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<ProfileForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let name = form.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return render_page(
            &state,
            &user,
            jar,
            None,
            Some("Profile names must contain 1 to 80 characters.".into()),
        )
        .await;
    }
    if rustss_core::integrations::provider(&form.provider).is_none() {
        return render_page(
            &state,
            &user,
            jar,
            None,
            Some("Choose a supported destination provider.".into()),
        )
        .await;
    }
    if let Err(error) = state.fetch.validate_url(form.endpoint_url.trim()) {
        return render_page(
            &state,
            &user,
            jar,
            None,
            Some(format!("Unsafe destination URL: {error}")),
        )
        .await;
    }
    if !matches!(
        form.credential_kind.as_str(),
        "none" | "bearer" | "basic" | "hmac"
    ) {
        return render_page(
            &state,
            &user,
            jar,
            None,
            Some("Choose a valid credential mode.".into()),
        )
        .await;
    }
    if form.payload_template.is_empty() || form.payload_template.chars().count() > 8_192 {
        return render_page(
            &state,
            &user,
            jar,
            None,
            Some("Templates must contain 1 to 8,192 characters.".into()),
        )
        .await;
    }
    let tags = normalize_tags(&form.tags).map_err(AppError::Validation)?;
    let mut event_types = Vec::new();
    if form.event_article_saved.is_some() {
        event_types.push("article.saved".to_string());
    }
    if form.event_article_bookmarked.is_some() {
        event_types.push("article.bookmarked".to_string());
    }
    if form.event_new_entries.is_some() {
        event_types.push("feed.new_entries".to_string());
    }
    if form.event_summary_ready.is_some() {
        event_types.push("summary.ready".to_string());
    }
    if event_types.is_empty() {
        event_types.push("article.saved".into());
    }
    let encrypted = if form.credential_kind == "none" || form.credential.is_empty() {
        None
    } else {
        Some(
            state
                .integration_credentials
                .encrypt_str(&form.credential)
                .map_err(|_| AppError::Internal(anyhow::anyhow!("credential encryption failed")))?,
        )
    };
    integrations::create_profile(
        state.db.pool(),
        user.id,
        name,
        &form.provider,
        form.endpoint_url.trim(),
        encrypted.as_deref(),
        &form.credential_kind,
        &form.payload_template,
        &tags,
        &event_types,
        form.max_attempts,
        form.rate_limit_per_minute,
        form.is_default.is_some(),
    )
    .await?;
    render_page(
        &state,
        &user,
        jar,
        Some("Integration profile created. Credential values will not be shown again.".into()),
        None,
    )
    .await
}

async fn enqueue(
    state: &AppState,
    profile: &integrations::IntegrationProfile,
    article_id: Option<Uuid>,
    event_type: &str,
) -> Result<Uuid, AppError> {
    let delivery =
        integrations::queue_delivery(state.db.pool(), profile, article_id, event_type).await?;
    jobs::enqueue(
        state.db.pool(),
        "integration_delivery",
        serde_json::json!({"delivery_id": delivery.id}),
        Some(&format!("integration:{}", delivery.id)),
        None,
        profile.max_attempts,
    )
    .await?;
    Ok(delivery.id)
}

/// Queue every enabled personal destination subscribed to an event.
///
/// Delivery is personal automation state while the referenced article remains
/// shared, so this does not duplicate acquisition or enrichment work.
pub async fn dispatch_event(
    state: &AppState,
    user_id: Uuid,
    article_id: Uuid,
    event_type: &str,
) -> Result<usize, AppError> {
    let profiles = integrations::profiles_for_event(state.db.pool(), user_id, event_type).await?;
    let mut queued = 0;
    for profile in profiles {
        enqueue(state, &profile, Some(article_id), event_type).await?;
        queued += 1;
    }
    Ok(queued)
}

pub async fn test_profile(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let profile = integrations::find_owned(state.db.pool(), id, user.id)
        .await?
        .ok_or(AppError::NotFound)?;
    enqueue(&state, &profile, None, "test").await?;
    render_page(
        &state,
        &user,
        jar,
        Some("Test delivery queued.".into()),
        None,
    )
    .await
}

pub async fn toggle_profile(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ToggleForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !integrations::set_paused(state.db.pool(), id, user.id, form.paused).await? {
        return Err(AppError::NotFound);
    }
    render_page(
        &state,
        &user,
        jar,
        Some(
            if form.paused {
                "Profile paused."
            } else {
                "Profile resumed."
            }
            .into(),
        ),
        None,
    )
    .await
}

pub async fn rotate_credential(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<RotateForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !matches!(
        form.credential_kind.as_str(),
        "none" | "bearer" | "basic" | "hmac"
    ) {
        return render_page(
            &state,
            &user,
            jar,
            None,
            Some("Choose a valid credential mode.".into()),
        )
        .await;
    }
    if form.credential_kind != "none" && form.credential.is_empty() {
        return render_page(
            &state,
            &user,
            jar,
            None,
            Some("Enter the replacement credential.".into()),
        )
        .await;
    }
    let encrypted = if form.credential_kind == "none" {
        None
    } else {
        Some(
            state
                .integration_credentials
                .encrypt_str(&form.credential)
                .map_err(|_| AppError::Internal(anyhow::anyhow!("credential encryption failed")))?,
        )
    };
    if !integrations::rotate_credential(
        state.db.pool(),
        id,
        user.id,
        encrypted.as_deref(),
        &form.credential_kind,
    )
    .await?
    {
        return Err(AppError::NotFound);
    }
    render_page(
        &state,
        &user,
        jar,
        Some("Credential rotated. The replacement value will not be shown again.".into()),
        None,
    )
    .await
}

pub async fn delete_profile(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !integrations::delete_profile(state.db.pool(), id, user.id).await? {
        return Err(AppError::NotFound);
    }
    render_page(
        &state,
        &user,
        jar,
        Some("Profile and its delivery history deleted.".into()),
        None,
    )
    .await
}

async fn authorized_article(
    state: &AppState,
    user_id: Uuid,
    article_id: Uuid,
) -> Result<rustss_db::Article, AppError> {
    let article = articles::find_by_id(state.db.pool(), article_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !subscriptions::is_subscribed(state.db.pool(), user_id, article.feed_id).await? {
        return Err(AppError::NotFound);
    }
    Ok(article)
}

async fn render_send(
    state: &AppState,
    user: &User,
    jar: CookieJar,
    article: &rustss_db::Article,
    notice: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let profiles = integrations::list_profiles(state.db.pool(), user.id)
        .await?
        .into_iter()
        .filter(|profile| profile.enabled && profile.paused_at.is_none())
        .collect();
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = ArticleSendTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui,
        is_admin: user.role == "admin",
        article_id: article.id.to_string(),
        article_title: article.title.clone(),
        profiles: profile_views(profiles),
        notice,
        error,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    Ok((jar, body).into_response())
}

pub async fn article_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let article = authorized_article(&state, user.id, id).await?;
    render_send(&state, &user, jar, &article, None, None).await
}

pub async fn article_send(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<SendForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let article = authorized_article(&state, user.id, id).await?;
    let profile = if form.profile_id.trim().is_empty() {
        integrations::default_profile(state.db.pool(), user.id).await?
    } else if let Ok(profile_id) = Uuid::parse_str(&form.profile_id) {
        integrations::find_owned(state.db.pool(), profile_id, user.id).await?
    } else {
        None
    }
    .filter(|profile| profile.enabled && profile.paused_at.is_none())
    .ok_or_else(|| AppError::Validation("Choose an active destination profile.".into()))?;
    enqueue(&state, &profile, Some(id), "article.saved").await?;
    render_send(
        &state,
        &user,
        jar,
        &article,
        Some(format!("Delivery to {} was queued.", profile.name)),
        None,
    )
    .await
}

pub async fn status(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let delivery = integrations::find_delivery(state.db.pool(), id)
        .await?
        .filter(|delivery| delivery.user_id == user.id)
        .ok_or(AppError::NotFound)?;
    Ok(axum::Json(serde_json::json!({"data": {"id": delivery.id, "status": delivery.status, "attempts": delivery.attempt_count, "error": delivery.error_summary}})).into_response())
}
