//! Request extractors for authentication and authorization.
//!
//! - [`MaybeUser`] — the current user, or `None` (never rejects).
//! - [`AuthUser`] — requires a logged-in user (401 otherwise).
//! - [`AdminUser`] — requires an admin (401/403 otherwise).

use std::convert::Infallible;
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::HeaderName;
use axum_extra::extract::cookie::CookieJar;

use rustss_db::{users, User};

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

async fn load_session_user(state: &AppState, parts: &Parts) -> Option<User> {
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

async fn load_proxy_user(state: &AppState, parts: &Parts) -> Result<Option<User>, AppError> {
    let policy = &state.settings.reverse_proxy_auth;
    if !policy.enabled {
        return Ok(None);
    }
    let Some(peer) = parts.extensions.get::<ConnectInfo<SocketAddr>>() else {
        return Ok(None);
    };
    if !policy
        .trusted_cidrs
        .iter()
        .any(|network| network.contains(&peer.0.ip()))
    {
        return Ok(None);
    }
    let user_header = HeaderName::from_bytes(policy.user_header.as_bytes())
        .map_err(|_| AppError::Internal(anyhow::anyhow!("invalid configured proxy header")))?;
    let Some(raw_email) = parts
        .headers
        .get(user_header)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(None);
    };
    let email = raw_email.trim().to_ascii_lowercase();
    let Some((local, domain)) = email.rsplit_once('@') else {
        return Ok(None);
    };
    if local.is_empty()
        || email.len() > 320
        || email.chars().any(char::is_control)
        || (!policy.allowed_domains.is_empty()
            && !policy
                .allowed_domains
                .iter()
                .any(|allowed| domain == allowed))
    {
        return Ok(None);
    }
    if let Some(user) = users::find_by_email(state.db.pool(), &email).await? {
        return Ok(Some(user));
    }
    if !policy.auto_provision {
        return Ok(None);
    }
    let name_header = HeaderName::from_bytes(policy.name_header.as_bytes())
        .map_err(|_| AppError::Internal(anyhow::anyhow!("invalid configured proxy name header")))?;
    let name = parts
        .headers
        .get(name_header)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| {
            !value.is_empty() && value.len() <= 160 && !value.chars().any(char::is_control)
        });
    let role = if policy.admin_emails.iter().any(|admin| admin == &email) {
        "admin"
    } else {
        "user"
    };
    match users::create_with_default_feeds(state.db.pool(), &email, name, None, role, None).await {
        Ok(user) => Ok(Some(user)),
        Err(create_error) => {
            // Concurrent requests for a newly-provisioned identity race safely
            // on the unique email and then converge on the shared user row.
            if let Some(user) = users::find_by_email(state.db.pool(), &email).await? {
                Ok(Some(user))
            } else {
                Err(create_error.into())
            }
        }
    }
}

async fn load_user(state: &AppState, parts: &Parts) -> Result<Option<User>, AppError> {
    if let Some(user) = load_session_user(state, parts).await {
        return Ok(Some(user));
    }
    load_proxy_user(state, parts).await
}

impl FromRequestParts<AppState> for MaybeUser {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Infallible> {
        match load_user(state, parts).await {
            Ok(user) => Ok(MaybeUser(user)),
            Err(error) => {
                tracing::warn!(%error, "trusted proxy authentication failed");
                Ok(MaybeUser(None))
            }
        }
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        load_user(state, parts)
            .await?
            .map(AuthUser)
            .ok_or(AppError::Unauthorized)
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let user = load_user(state, parts)
            .await?
            .ok_or(AppError::Unauthorized)?;
        if user.role == "admin" {
            Ok(AdminUser(user))
        } else {
            Err(AppError::Forbidden)
        }
    }
}
