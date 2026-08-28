//! API-token management UI (create / list / revoke). Tokens are shown in full
//! exactly once, at creation; only their hashes are stored.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use rustss_db::api_tokens;

use crate::auth::extractor::AuthUser;
use crate::auth::{csrf, token};
use crate::error::AppError;
use crate::state::AppState;

struct TokenView {
    id: String,
    name: String,
    created: String,
    revoked: bool,
}

#[derive(Template)]
#[template(path = "tokens.html")]
struct TokensTemplate {
    csrf_token: String,
    new_token: Option<String>,
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
    jar: CookieJar,
    new_token: Option<String>,
) -> Result<Response, AppError> {
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let tokens = api_tokens::list_for_user(state.db.pool(), user_id)
        .await?
        .into_iter()
        .map(|t| TokenView {
            id: t.id.to_string(),
            name: t.name,
            created: t.created_at.date().to_string(),
            revoked: t.revoked_at.is_some(),
        })
        .collect();

    let body = TokensTemplate {
        csrf_token,
        new_token,
        tokens,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, body).into_response())
}

/// `GET /settings/tokens`.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render_page(&state, user.id, jar, None).await
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
        return render_page(&state, user.id, jar, None).await;
    }
    let generated = token::generate();
    api_tokens::create(state.db.pool(), user.id, name, &generated.hash, &[]).await?;
    render_page(&state, user.id, jar, Some(generated.raw)).await
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
    api_tokens::revoke(state.db.pool(), id, user.id).await?;
    Ok((jar, Redirect::to("/settings/tokens")).into_response())
}
