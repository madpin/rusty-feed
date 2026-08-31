//! Versioned personal CSS and opt-in administrator-managed JavaScript.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;
use uuid::Uuid;

use rustss_db::{ui_customizations, User};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

struct VersionView {
    id: String,
    version: i32,
    created: String,
    active: bool,
}

#[derive(Template)]
#[template(path = "customization.html")]
struct CustomizationTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    css_enabled: bool,
    javascript_enabled: bool,
    css: String,
    javascript: String,
    css_versions: Vec<VersionView>,
    javascript_versions: Vec<VersionView>,
    safe_mode: bool,
    notice: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
pub struct AssetForm {
    csrf_token: String,
    content: String,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

fn safe_mode(jar: &CookieJar) -> bool {
    jar.get("rustss_safe_mode")
        .is_some_and(|cookie| cookie.value() == "1")
}

fn versions(
    rows: Vec<ui_customizations::UiCustomization>,
    ui: &prefs::Preferences,
) -> Vec<VersionView> {
    rows.into_iter()
        .map(|row| VersionView {
            id: row.id.to_string(),
            version: row.version,
            created: ui.format_date(row.created_at),
            active: row.enabled,
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
    let (css, javascript, css_history, javascript_history) = tokio::try_join!(
        ui_customizations::current_css(state.db.pool(), user.id),
        ui_customizations::current_javascript(state.db.pool()),
        ui_customizations::history(state.db.pool(), Some(user.id), "css", 10),
        ui_customizations::history(state.db.pool(), None, "javascript", 10),
    )?;
    let is_safe = safe_mode(&jar);
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let css_versions = versions(css_history, &ui);
    let javascript_versions = versions(javascript_history, &ui);
    let body = CustomizationTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui,
        is_admin: user.role == "admin",
        css_enabled: state.settings.custom_css_enabled,
        javascript_enabled: state.settings.custom_javascript_enabled,
        css: css.map(|asset| asset.content).unwrap_or_default(),
        javascript: javascript.map(|asset| asset.content).unwrap_or_default(),
        css_versions,
        javascript_versions,
        safe_mode: is_safe,
        notice,
        error,
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

fn validate_css(content: &str, max_bytes: usize) -> Result<(), String> {
    if content.len() > max_bytes {
        return Err(format!("CSS may contain at most {max_bytes} bytes."));
    }
    let lower = content.to_ascii_lowercase();
    if [
        "@import",
        "url(",
        "expression(",
        "behavior:",
        "-moz-binding",
    ]
    .iter()
    .any(|token| lower.contains(token))
    {
        return Err("Personal CSS cannot import or request external resources.".into());
    }
    Ok(())
}

fn validate_javascript(content: &str, max_bytes: usize) -> Result<(), String> {
    if content.len() > max_bytes {
        return Err(format!("JavaScript may contain at most {max_bytes} bytes."));
    }
    if content.contains('\0') {
        return Err("JavaScript cannot contain NUL bytes.".into());
    }
    Ok(())
}

pub async fn save_css(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<AssetForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !state.settings.custom_css_enabled {
        return Err(AppError::NotFound);
    }
    if let Err(error) = validate_css(&form.content, state.settings.ui_customization_max_bytes) {
        return render_page(&state, &user, jar, None, Some(error)).await;
    }
    ui_customizations::create_version(
        state.db.pool(),
        Some(user.id),
        "css",
        &form.content,
        user.id,
    )
    .await?;
    render_page(
        &state,
        &user,
        jar,
        Some("Personal CSS version saved.".into()),
        None,
    )
    .await
}

pub async fn save_javascript(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<AssetForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) || user.role != "admin" {
        return Err(AppError::Forbidden);
    }
    if !state.settings.custom_javascript_enabled {
        return Err(AppError::NotFound);
    }
    if let Err(error) =
        validate_javascript(&form.content, state.settings.ui_customization_max_bytes)
    {
        return render_page(&state, &user, jar, None, Some(error)).await;
    }
    ui_customizations::create_version(state.db.pool(), None, "javascript", &form.content, user.id)
        .await?;
    render_page(
        &state,
        &user,
        jar,
        Some("Instance script version saved.".into()),
        None,
    )
    .await
}

pub async fn reset_css(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    ui_customizations::disable(state.db.pool(), Some(user.id), "css").await?;
    render_page(&state, &user, jar, Some("Personal CSS reset.".into()), None).await
}

pub async fn reset_javascript(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) || user.role != "admin" {
        return Err(AppError::Forbidden);
    }
    ui_customizations::disable(state.db.pool(), None, "javascript").await?;
    render_page(
        &state,
        &user,
        jar,
        Some("Instance script disabled.".into()),
        None,
    )
    .await
}

pub async fn restore_css(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    ui_customizations::restore(state.db.pool(), id, Some(user.id), "css", user.id)
        .await?
        .ok_or(AppError::NotFound)?;
    render_page(
        &state,
        &user,
        jar,
        Some("CSS version restored as a new version.".into()),
        None,
    )
    .await
}

pub async fn restore_javascript(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) || user.role != "admin" {
        return Err(AppError::Forbidden);
    }
    ui_customizations::restore(state.db.pool(), id, None, "javascript", user.id)
        .await?
        .ok_or(AppError::NotFound)?;
    render_page(
        &state,
        &user,
        jar,
        Some("Script version restored as a new version.".into()),
        None,
    )
    .await
}

pub async fn css_asset(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let content = if state.settings.custom_css_enabled && !safe_mode(&jar) {
        ui_customizations::current_css(state.db.pool(), user.id)
            .await?
            .map(|asset| asset.content)
            .unwrap_or_default()
    } else {
        String::new()
    };
    Ok((
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        content,
    )
        .into_response())
}

pub async fn javascript_asset(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let content =
        if state.settings.custom_javascript_enabled && ui.custom_script_enabled && !safe_mode(&jar)
        {
            ui_customizations::current_javascript(state.db.pool())
                .await?
                .map(|asset| asset.content)
                .unwrap_or_default()
        } else {
            String::new()
        };
    let mut response = (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        content,
    )
        .into_response();
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

pub async fn enter_safe_mode(State(state): State<AppState>, jar: CookieJar) -> Response {
    let mut cookie = Cookie::new("rustss_safe_mode", "1");
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_secure(state.settings.cookie_secure);
    (jar.add(cookie), Redirect::to("/settings/customization")).into_response()
}

pub async fn leave_safe_mode(State(state): State<AppState>, jar: CookieJar) -> Response {
    let mut cookie = Cookie::new("rustss_safe_mode", "");
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_secure(state.settings.cookie_secure);
    cookie.make_removal();
    (jar.add(cookie), Redirect::to("/settings/customization")).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_blocks_network_and_legacy_active_constructs() {
        assert!(validate_css(".article { color: rebeccapurple }", 1_024).is_ok());
        assert!(validate_css("@import 'https://tracker.test/x.css';", 1_024).is_err());
        assert!(validate_css("body{background:url(https://tracker.test/a)}", 1_024).is_err());
    }
}
