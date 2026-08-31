//! Email address verification: a stateless, signed (AEAD-encrypted) token
//! carried in a link, plus the verify + resend handlers and a gate helper for
//! privileged actions (`docs/PUBLIC_LAUNCH_PLAN.md` §4.5).

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use rustss_core::crypto::Cryptor;
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

/// How long a verification link stays valid.
const TOKEN_TTL_SECONDS: i64 = 48 * 60 * 60;

/// Mint a signed verification token for `user_id` (valid for [`TOKEN_TTL_SECONDS`]).
pub fn issue_token(cryptor: &Cryptor, user_id: Uuid) -> Result<String, AppError> {
    let exp = OffsetDateTime::now_utc().unix_timestamp() + TOKEN_TTL_SECONDS;
    cryptor
        .encrypt_str(&format!("{user_id}|{exp}"))
        .map_err(|_| AppError::Internal(anyhow::anyhow!("failed to mint verification token")))
}

/// Validate a token and return the user id it verifies, if still valid.
fn parse_token(cryptor: &Cryptor, token: &str) -> Option<Uuid> {
    let plain = cryptor.decrypt_str(token).ok()?;
    let (id, exp) = plain.split_once('|')?;
    let exp: i64 = exp.parse().ok()?;
    if OffsetDateTime::now_utc().unix_timestamp() > exp {
        return None;
    }
    Uuid::parse_str(id).ok()
}

/// Build the absolute verification link for an email body.
pub fn verification_link(base_url: &str, token: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(token.as_bytes()).collect();
    format!("{}/verify?token={encoded}", base_url.trim_end_matches('/'))
}

/// Send a verification email best-effort. A send failure is logged but never
/// propagated, so an SMTP outage cannot break registration.
pub async fn send_verification_email(state: &AppState, user_id: Uuid, email: &str) {
    let Ok(token) = issue_token(&state.email_verification_tokens, user_id) else {
        return;
    };
    let link = verification_link(&state.settings.base_url, &token);
    let subject = "Verify your RustSS email address";
    let body = format!(
        "Welcome to RustSS!\n\nPlease confirm this email address by opening the link below \
         (valid for 48 hours):\n\n{link}\n\nIf you did not create this account, you can ignore \
         this message."
    );
    if let Err(error) = state.mailer.send(email, subject, &body).await {
        tracing::warn!(%user_id, error = %error, "failed to send verification email");
    }
}

/// Whether `user` must verify their email before privileged actions. Only local
/// (password) accounts are gated; provider-authenticated accounts are trusted.
pub fn needs_verification(state: &AppState, user: &rustss_db::User) -> bool {
    state.settings.email_verification_required
        && user.email_verified_at.is_none()
        && user.password_hash.is_some()
}

/// Reject a privileged action for an unverified local account (FND-191).
pub fn require_verified(state: &AppState, user: &rustss_db::User) -> Result<(), AppError> {
    if needs_verification(state, user) {
        return Err(AppError::Validation(
            "Please verify your email address first. Check your inbox, or resend the link from Settings."
                .to_string(),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct VerifyQuery {
    #[serde(default)]
    token: String,
}

/// `GET /verify?token=...` — confirm an email address.
pub async fn verify(
    State(state): State<AppState>,
    Query(query): Query<VerifyQuery>,
) -> Result<Response, AppError> {
    let Some(user_id) = parse_token(&state.email_verification_tokens, query.token.trim()) else {
        return Ok(Redirect::to("/login?verify=invalid").into_response());
    };
    rustss_db::users::mark_email_verified(state.db.pool(), user_id).await?;
    Ok(Redirect::to("/login?verify=ok").into_response())
}

#[derive(Deserialize)]
pub struct ResendForm {
    csrf_token: String,
}

/// `POST /verify/resend` — re-send the current user's verification email.
pub async fn resend(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<ResendForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    // A cheap abuse guard reusing the auth limiter, keyed per user.
    if !state
        .auth_limiter
        .check(&format!("verify-resend:{}", user.id))
        .await
    {
        return Err(AppError::RateLimited);
    }
    if user.email_verified_at.is_none() {
        send_verification_email(&state, user.id, &user.email).await;
    }
    Ok(Redirect::to("/settings/preferences?verify=sent").into_response())
}
