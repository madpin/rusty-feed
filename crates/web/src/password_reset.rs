//! Forgot-password flow. The response never says whether an email exists.
//! Tokens are single-use, expire in one hour, and only their hash is stored.

use askama::Template;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use time::{Duration, OffsetDateTime};

use rusty_feed_db::{password_resets, users};

use crate::auth::csrf;
use crate::auth::password::{hash_password, validate_strength};
use crate::auth::token;
use crate::error::AppError;
use crate::extract::ClientIp;
use crate::state::AppState;

const RESET_TTL: Duration = Duration::hours(1);

#[derive(Template)]
#[template(path = "forgot_password.html")]
struct ForgotTemplate {
    csrf_token: String,
    error: Option<String>,
    mail_enabled: bool,
    sent: bool,
}

#[derive(Template)]
#[template(path = "reset_password.html")]
struct ResetTemplate {
    csrf_token: String,
    token: String,
    error: Option<String>,
    min_password_length: usize,
}

#[derive(Deserialize)]
pub(crate) struct EmailForm {
    csrf_token: String,
    email: String,
}

#[derive(Deserialize)]
pub(crate) struct ResetForm {
    csrf_token: String,
    token: String,
    password: String,
}

#[derive(Deserialize)]
pub(crate) struct ResetQuery {
    #[serde(default)]
    token: String,
}

pub async fn forgot_page(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render_forgot(&state, jar, None, false)
}

pub async fn forgot_submit(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    jar: CookieJar,
    Form(form): Form<EmailForm>,
) -> Result<Response, AppError> {
    if !state.auth_limiter.check(&ip).await {
        return Err(AppError::RateLimited);
    }
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !state.mailer.is_enabled() {
        return render_forgot(&state, jar, None, false);
    }
    let email = form.email.trim();
    if email_is_address(email) {
        if let Some(user) = users::find_by_email(state.db.pool(), email).await? {
            if user.password_hash.is_some() && !user.is_demo {
                let generated = token::generate();
                let expires = OffsetDateTime::now_utc() + RESET_TTL;
                password_resets::issue(state.db.pool(), user.id, &generated.hash, expires).await?;
                let link = format!(
                    "{}/reset-password?token={}",
                    state.settings.base_url.trim_end_matches('/'),
                    generated.raw
                );
                let body = format!(
                    "A password reset was requested for your Rusty Feed account.\n\n\
                     Open this link within one hour to choose a new password:\n\n{link}\n\n\
                     If you did not ask for this, you can ignore the message. The link works once."
                );
                if let Err(error) = state
                    .mailer
                    .send(email, "Reset your Rusty Feed password", &body)
                    .await
                {
                    tracing::warn!(error = %error, "password reset email was not sent");
                }
            }
        }
    }
    render_forgot(&state, jar, None, true)
}

pub async fn reset_page(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<ResetQuery>,
) -> Result<Response, AppError> {
    if !token_shape(&query.token) {
        return render_reset(
            &state,
            jar,
            String::new(),
            Some("This reset link is invalid or has expired.".into()),
        );
    }
    render_reset(&state, jar, query.token, None)
}

pub async fn reset_submit(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    jar: CookieJar,
    Form(form): Form<ResetForm>,
) -> Result<Response, AppError> {
    if !state.auth_limiter.check(&ip).await {
        return Err(AppError::RateLimited);
    }
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !token_shape(&form.token) {
        return render_reset(
            &state,
            jar,
            String::new(),
            Some("This reset link is invalid or has expired.".into()),
        );
    }
    if let Err(message) = validate_strength(&form.password, state.settings.min_password_length) {
        return render_reset(&state, jar, form.token, Some(message));
    }
    let password_hash = hash_password(&form.password).map_err(|_| {
        AppError::Internal(anyhow::anyhow!("could not hash the replacement password"))
    })?;
    let consumed =
        password_resets::consume(state.db.pool(), &token::hash(&form.token), &password_hash)
            .await?;
    if consumed.is_none() {
        return render_reset(
            &state,
            jar,
            String::new(),
            Some("This reset link is invalid or has expired.".into()),
        );
    }
    Ok(Redirect::to("/login?reset=1").into_response())
}

fn render_forgot(
    state: &AppState,
    jar: CookieJar,
    error: Option<String>,
    sent: bool,
) -> Result<Response, AppError> {
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = ForgotTemplate {
        csrf_token,
        error,
        mail_enabled: state.mailer.is_enabled(),
        sent,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    Ok((jar, body).into_response())
}

fn render_reset(
    state: &AppState,
    jar: CookieJar,
    token: String,
    error: Option<String>,
) -> Result<Response, AppError> {
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = ResetTemplate {
        csrf_token,
        token,
        error,
        min_password_length: state.settings.min_password_length,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    Ok((jar, body).into_response())
}

fn email_is_address(email: &str) -> bool {
    email.len() <= 320 && email.contains('@') && !email.contains(['\r', '\n', ' '])
}

fn token_shape(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}
