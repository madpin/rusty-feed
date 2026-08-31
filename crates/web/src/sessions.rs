//! Personal session controls. Only opaque-token hashes are compared; session
//! bearer values and internal hashes are never rendered.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use rustss_db::User;

use crate::auth::extractor::AuthUser;
use crate::auth::{csrf, session, token};
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

struct SessionView {
    id: String,
    created: String,
    last_used: String,
    expires: String,
    current: bool,
}

#[derive(Template)]
#[template(path = "sessions.html")]
struct SessionsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    sessions: Vec<SessionView>,
    notice: Option<String>,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

async fn render_page(
    state: &AppState,
    user: &User,
    jar: CookieJar,
    notice: Option<String>,
) -> Result<Response, AppError> {
    let current_hash = jar
        .get(session::SESSION_COOKIE)
        .map(|cookie| token::hash(cookie.value()));
    let sessions = rustss_db::sessions::list_for_user(state.db.pool(), user.id)
        .await?
        .into_iter()
        .map(|entry| SessionView {
            id: entry.id.to_string(),
            created: entry.created_at.to_string(),
            last_used: entry.last_used_at.to_string(),
            expires: entry.expires_at.date().to_string(),
            current: current_hash.as_deref() == Some(entry.token_hash.as_str()),
        })
        .collect();
    let ui = prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = SessionsTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui,
        is_admin: user.role == "admin",
        sessions,
        notice,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    Ok((jar, body).into_response())
}

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render_page(&state, &user, jar, None).await
}

pub async fn revoke(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let revoked = rustss_db::sessions::delete_for_user_by_id(state.db.pool(), user.id, id).await?;
    render_page(
        &state,
        &user,
        jar,
        Some(if revoked {
            "Session revoked.".to_string()
        } else {
            "That session is no longer active.".to_string()
        }),
    )
    .await
}

pub async fn revoke_others(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let current_hash = jar
        .get(session::SESSION_COOKIE)
        .map(|cookie| token::hash(cookie.value()))
        .ok_or(AppError::Unauthorized)?;
    let removed =
        rustss_db::sessions::delete_other_sessions(state.db.pool(), user.id, &current_hash).await?;
    render_page(
        &state,
        &user,
        jar,
        Some(format!("Signed out {removed} other session(s).")),
    )
    .await
}
