//! Request extractors for authentication and authorization.
//!
//! - [`MaybeUser`] — the current user, or `None` (never rejects).
//! - [`AuthUser`] — requires a logged-in user (401 otherwise).
//! - [`AdminUser`] — requires an admin (401/403 otherwise).

use std::convert::Infallible;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::cookie::CookieJar;

use rustss_db::User;

use crate::auth::session::SESSION_COOKIE;
use crate::auth::token;
use crate::error::AppError;
use crate::state::AppState;

/// The current user, if the request carries a valid session.
pub struct MaybeUser(pub Option<User>);

/// A required, authenticated user.
pub struct AuthUser(pub User);

/// A required user with the `admin` role.
pub struct AdminUser(pub User);

async fn load_user(state: &AppState, parts: &Parts) -> Option<User> {
    let jar = CookieJar::from_headers(&parts.headers);
    let raw = jar.get(SESSION_COOKIE)?.value().to_string();
    let hash = token::hash(&raw);

    let session = rustss_db::sessions::find_valid(state.db.pool(), &hash)
        .await
        .ok()
        .flatten()?;
    let user = rustss_db::users::find_by_id(state.db.pool(), session.user_id)
        .await
        .ok()
        .flatten()?;

    // Best-effort: refresh last_used_at (ignore errors).
    let _ = rustss_db::sessions::touch(state.db.pool(), session.id).await;
    Some(user)
}

impl FromRequestParts<AppState> for MaybeUser {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Infallible> {
        Ok(MaybeUser(load_user(state, parts).await))
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        load_user(state, parts)
            .await
            .map(AuthUser)
            .ok_or(AppError::Unauthorized)
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let user = load_user(state, parts)
            .await
            .ok_or(AppError::Unauthorized)?;
        if user.role == "admin" {
            Ok(AdminUser(user))
        } else {
            Err(AppError::Forbidden)
        }
    }
}
