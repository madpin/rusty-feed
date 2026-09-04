//! Session cookie helpers (login / logout).

use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use rusty_feed_db::Db;

use crate::auth::token;

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "rusty_feed_sid";
/// Name of the readable marker used by the demo banner.
pub const DEMO_COOKIE: &str = "rusty_feed_demo";
/// Session lifetime.
pub const SESSION_TTL_DAYS: i64 = 30;

/// Create a session for `user_id` and return the jar with the cookie set.
pub async fn login(
    jar: CookieJar,
    db: &Db,
    user_id: Uuid,
    secure: bool,
) -> Result<CookieJar, rusty_feed_db::DbError> {
    login_with_ttl(jar, db, user_id, secure, Duration::days(SESSION_TTL_DAYS)).await
}

/// Create a session with an explicit lifetime. Demo accounts (§7.5) use a short
/// TTL so abandoned sessions expire well before the daily reset.
pub async fn login_with_ttl(
    jar: CookieJar,
    db: &Db,
    user_id: Uuid,
    secure: bool,
    ttl: Duration,
) -> Result<CookieJar, rusty_feed_db::DbError> {
    let generated = token::generate();
    let expires = OffsetDateTime::now_utc() + ttl;
    rusty_feed_db::sessions::create(db.pool(), user_id, &generated.hash, expires).await?;

    let cookie = Cookie::build((SESSION_COOKIE, generated.raw))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(ttl)
        .build();
    Ok(clear_demo_marker(jar).add(cookie))
}

fn clear_demo_marker(jar: CookieJar) -> CookieJar {
    jar.remove(Cookie::build((DEMO_COOKIE, "")).path("/").build())
}

/// Destroy the current session (if any) and return the jar with the cookie
/// cleared.
pub async fn logout(jar: CookieJar, db: &Db) -> CookieJar {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        let _ =
            rusty_feed_db::sessions::delete_by_token_hash(db.pool(), &token::hash(cookie.value()))
                .await;
    }
    clear_demo_marker(jar.remove(Cookie::build((SESSION_COOKIE, "")).path("/").build()))
}
