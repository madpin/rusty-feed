//! Managed AI-provider configuration with user → admin → environment fallback.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use rusty_feed_db::{llm_profiles, User};

use crate::auth::csrf;
use crate::auth::{AdminReadUser, AdminUser, AuthUser};
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

struct ProfileView {
    id: String,
    name: String,
    provider: String,
    model: String,
    base_url: String,
    enabled: bool,
    priority: i32,
    has_key: bool,
}

/// Read-only view of the environment-configured fallback provider.
struct EnvProviderView {
    provider: String,
    model: String,
    base_url: String,
    has_key: bool,
    /// Whether the current scope can create a profile of this provider type
    /// (users may only create OpenAI-compatible profiles).
    copyable: bool,
}

#[derive(Template)]
#[template(path = "ai_settings.html")]
struct AiSettingsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    managing_instance: bool,
    profiles: Vec<ProfileView>,
    env_provider: Option<EnvProviderView>,
    auto_summary_enabled: bool,
    auto_summary_min_chars: u32,
    notice: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateForm {
    csrf_token: String,
    name: String,
    provider: String,
    model: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    api_key: String,
    #[serde(default = "default_priority")]
    priority: i32,
}

fn default_priority() -> i32 {
    100
}

#[derive(Deserialize)]
pub struct ToggleForm {
    csrf_token: String,
    enabled: bool,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct SummaryPolicyForm {
    csrf_token: String,
    #[serde(default)]
    enabled: bool,
    min_content_chars: u32,
}

async fn render(
    state: &AppState,
    user: &User,
    jar: CookieJar,
    managing_instance: bool,
    notice: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let (owner, scope) = if managing_instance {
        (None, "admin")
    } else {
        (Some(user.id), "user")
    };
    let profiles = llm_profiles::list_for_owner(state.db.pool(), owner, scope)
        .await?
        .into_iter()
        .map(|profile| ProfileView {
            id: profile.id.to_string(),
            name: profile.name,
            provider: profile.provider,
            model: profile.model,
            base_url: profile.base_url,
            enabled: profile.enabled,
            priority: profile.priority,
            has_key: profile.api_key_ciphertext.is_some(),
        })
        .collect();
    let env_provider = state.settings.env_llm.as_ref().map(|env| EnvProviderView {
        provider: env.provider.clone(),
        model: env.model.clone(),
        base_url: env.base_url.clone(),
        has_key: env.has_key,
        copyable: managing_instance || env.provider == "openai",
    });
    let summary_policy = rusty_feed_db::summary_policy::get_or_init(
        state.db.pool(),
        state.settings.auto_summarize_default,
        state.settings.auto_summarize_min_chars_default,
    )
    .await?;
    let body = AiSettingsTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui,
        is_admin: user.role == "admin",
        managing_instance,
        profiles,
        env_provider,
        auto_summary_enabled: summary_policy.enabled,
        auto_summary_min_chars: summary_policy.min_content_chars,
        notice,
        error,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    Ok((jar, body).into_response())
}

pub async fn user_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render(&state, &user, jar, false, None, None).await
}

pub async fn admin_page(
    State(state): State<AppState>,
    AdminReadUser(user): AdminReadUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render(&state, &user, jar, true, None, None).await
}

pub async fn admin_summary_policy(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Form(form): Form<SummaryPolicyForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if form.min_content_chars > rusty_feed_db::summary_policy::MAX_MIN_CONTENT_CHARS {
        return render(
            &state,
            &user,
            jar,
            true,
            None,
            Some("The minimum must be between 0 and 1,000,000 characters.".to_string()),
        )
        .await;
    }
    rusty_feed_db::summary_policy::update(state.db.pool(), form.enabled, form.min_content_chars)
        .await?;
    render(
        &state,
        &user,
        jar,
        true,
        Some("Automatic summary policy saved. New feed ingestion uses it immediately.".to_string()),
        None,
    )
    .await
}

pub async fn admin_summary_backfill(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let policy = rusty_feed_db::summary_policy::get_or_init(
        state.db.pool(),
        state.settings.auto_summarize_default,
        state.settings.auto_summarize_min_chars_default,
    )
    .await?;
    if !policy.enabled {
        return render(
            &state,
            &user,
            jar,
            true,
            None,
            Some("Enable automatic summaries before starting a backfill.".to_string()),
        )
        .await;
    }
    let queued = rusty_feed_db::jobs::enqueue(
        state.db.pool(),
        "summary_backfill",
        serde_json::json!({ "feed_id": null, "ready_only": false, "after": null }),
        Some("summary-backfill:all:false:start"),
        None,
        3,
    )
    .await?
    .is_some();
    render(
        &state,
        &user,
        jar,
        true,
        Some(if queued {
            "Eligible existing articles were queued for bounded background backfill.".to_string()
        } else {
            "An automatic summary backfill is already queued or running.".to_string()
        }),
        None,
    )
    .await
}

fn validate(form: &CreateForm, managing_instance: bool) -> Result<(&str, String), String> {
    let provider = form.provider.trim();
    if !matches!(provider, "openai" | "ollama") || (!managing_instance && provider != "openai") {
        return Err("Choose an available provider.".to_string());
    }
    if form.name.trim().is_empty()
        || form.name.chars().count() > 80
        || form.model.trim().is_empty()
        || form.model.chars().count() > 160
        || !(0..=10_000).contains(&form.priority)
    {
        return Err("Enter a name, model, and priority between 0 and 10000.".to_string());
    }
    if provider == "openai" && form.api_key.trim().is_empty() {
        return Err("OpenAI profiles require an API key.".to_string());
    }
    let base_url = if managing_instance && !form.base_url.trim().is_empty() {
        form.base_url.trim().trim_end_matches('/').to_string()
    } else if provider == "ollama" {
        "http://127.0.0.1:11434".to_string()
    } else {
        "https://api.openai.com".to_string()
    };
    let parsed =
        url::Url::parse(&base_url).map_err(|_| "Enter a valid provider URL.".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || (!managing_instance && base_url != "https://api.openai.com")
    {
        return Err("That provider URL is not allowed for this profile.".to_string());
    }
    Ok((provider, base_url))
}

async fn create(
    state: AppState,
    user: User,
    jar: CookieJar,
    form: CreateForm,
    managing_instance: bool,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let (provider, base_url) = match validate(&form, managing_instance) {
        Ok(value) => value,
        Err(error) => {
            return render(&state, &user, jar, managing_instance, None, Some(error)).await
        }
    };
    let encrypted_key = if form.api_key.trim().is_empty() {
        None
    } else {
        Some(
            state
                .llm_credentials
                .encrypt_str(form.api_key.trim())
                .map_err(|error| {
                    AppError::Internal(anyhow::anyhow!("AI credential encryption failed: {error}"))
                })?,
        )
    };
    llm_profiles::create(
        state.db.pool(),
        (!managing_instance).then_some(user.id),
        if managing_instance { "admin" } else { "user" },
        form.name.trim(),
        provider,
        form.model.trim(),
        &base_url,
        encrypted_key.as_deref(),
        true,
        form.priority,
    )
    .await?;
    render(
        &state,
        &user,
        jar,
        managing_instance,
        Some(
            "AI provider saved. New summaries use the updated provider order immediately."
                .to_string(),
        ),
        None,
    )
    .await
}

pub async fn user_create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<CreateForm>,
) -> Result<Response, AppError> {
    crate::verify::deny_demo(&user)?;
    create(state, user, jar, form, false).await
}

pub async fn admin_create(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Form(form): Form<CreateForm>,
) -> Result<Response, AppError> {
    create(state, user, jar, form, true).await
}

async fn toggle(
    state: AppState,
    user: User,
    jar: CookieJar,
    id: Uuid,
    form: ToggleForm,
    managing_instance: bool,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    llm_profiles::set_enabled(
        state.db.pool(),
        id,
        (!managing_instance).then_some(user.id),
        if managing_instance { "admin" } else { "user" },
        form.enabled,
    )
    .await?;
    Ok(Redirect::to(if managing_instance {
        "/admin/ai"
    } else {
        "/settings/ai"
    })
    .into_response())
}

pub async fn user_toggle(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ToggleForm>,
) -> Result<Response, AppError> {
    toggle(state, user, jar, id, form, false).await
}

pub async fn admin_toggle(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<ToggleForm>,
) -> Result<Response, AppError> {
    toggle(state, user, jar, id, form, true).await
}

async fn delete(
    state: AppState,
    user: User,
    jar: CookieJar,
    id: Uuid,
    form: CsrfForm,
    managing_instance: bool,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    llm_profiles::delete(
        state.db.pool(),
        id,
        (!managing_instance).then_some(user.id),
        if managing_instance { "admin" } else { "user" },
    )
    .await?;
    Ok(Redirect::to(if managing_instance {
        "/admin/ai"
    } else {
        "/settings/ai"
    })
    .into_response())
}

pub async fn user_delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    delete(state, user, jar, id, form, false).await
}

pub async fn admin_delete(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    delete(state, user, jar, id, form, true).await
}
