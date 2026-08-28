//! Session cookie helpers (login / logout).

use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use rustss_db::Db;

use crate::auth::token;

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "rustss_sid";
/// Session lifetime.
pub const SESSION_TTL_DAYS: i64 = 30;

/// Create a session for `user_id` and return the jar with the cookie set.
pub async fn login(
    jar: CookieJar,
    db: &Db,
    user_id: Uuid,
    secure: bool,
) -> Result<CookieJar, rustss_db::DbError> {
    let generated = token::generate();
    let expires = OffsetDateTime::now_utc() + Duration::days(SESSION_TTL_DAYS);
    rustss_db::sessions::create(db.pool(), user_id, &generated.hash, expires).await?;

    let cookie = Cookie::build((SESSION_COOKIE, generated.raw))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(Duration::days(SESSION_TTL_DAYS))
        .build();
    Ok(jar.add(cookie))
}

/// Destroy the current session (if any) and return the jar with the cookie
/// cleared.
pub async fn logout(jar: CookieJar, db: &Db) -> CookieJar {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        let _ = rustss_db::sessions::delete_by_token_hash(db.pool(), &token::hash(cookie.value()))
            .await;
    }
    jar.remove(Cookie::build((SESSION_COOKIE, "")).path("/").build())
}
