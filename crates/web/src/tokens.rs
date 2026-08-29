//! API-token management UI (create / list / revoke). Tokens are shown in full
//! exactly once, at creation; only their hashes are stored.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use rustss_db::api_tokens;

use crate::auth::extractor::AuthUser;
use crate::auth::{csrf, token};
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

struct TokenView {
    id: String,
    name: String,
    created: String,
    last_used: Option<String>,
    revoked: bool,
}

#[derive(Template)]
#[template(path = "tokens.html")]
struct TokensTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    new_token: Option<String>,
    notice: Option<String>,
    error: Option<String>,
    tokens: Vec<TokenView>,
}

#[derive(Deserialize)]
pub struct CreateForm {
    name: String,
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

async fn render_page(
    state: &AppState,
    user_id: Uuid,
    user_email: String,
    jar: CookieJar,
    new_token: Option<String>,
    notice: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user_id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let tokens = api_tokens::list_for_user(state.db.pool(), user_id)
        .await?
        .into_iter()
        .map(|t| TokenView {
            id: t.id.to_string(),
            name: t.name,
            created: t.created_at.date().to_string(),
            last_used: t.last_used_at.map(|time| time.date().to_string()),
            revoked: t.revoked_at.is_some(),
        })
        .collect();

    let reveal = new_token.is_some();
    let body = TokensTemplate {
        theme: ui.theme.clone(),
        user_email,
        csrf_token,
        ui,
        new_token,
        notice,
        error,
        tokens,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    let mut response = (jar, body).into_response();
    if reveal {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    Ok(response)
}

/// `GET /settings/tokens`.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render_page(&state, user.id, user.email, jar, None, None, None).await
}

/// `POST /settings/tokens` — create a token and show it once.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<CreateForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let name = form.name.trim();
    if name.is_empty() {
        return render_page(
            &state,
            user.id,
            user.email,
            jar,
            None,
            None,
            Some("Enter a name for this token.".to_string()),
        )
        .await;
    }
    let generated = token::generate();
    api_tokens::create(state.db.pool(), user.id, name, &generated.hash, &[]).await?;
    render_page(
        &state,
        user.id,
        user.email,
        jar,
        Some(generated.raw),
        Some("Token created.".to_string()),
        None,
    )
    .await
}

/// `POST /settings/tokens/{id}/revoke`.
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
    let revoked = api_tokens::revoke(state.db.pool(), id, user.id).await?;
    let (notice, error) = if revoked {
        (Some("Token revoked.".to_string()), None)
    } else {
        (
            None,
            Some("That token is already revoked or no longer exists.".to_string()),
        )
    };
    render_page(&state, user.id, user.email, jar, None, notice, error).await
}
