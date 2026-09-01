//! CSRF protection using the double-submit-cookie pattern.
//!
//! A random token is stored in a `SameSite=Strict` cookie and echoed in a hidden
//! form field; on a mutating request the two must match. This defends
//! cookie-authenticated form posts without needing a session for anonymous
//! pages (login/register).

use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};

use crate::auth::token;

/// Name of the CSRF cookie.
pub const CSRF_COOKIE: &str = "rusty_feed_csrf";
/// Form field name carrying the CSRF token.
pub const CSRF_FIELD: &str = "csrf_token";

/// Ensure a CSRF token exists, returning the token and the (possibly updated)
/// jar. Reuses an existing token so multiple forms on a page agree.
pub fn issue(jar: CookieJar, secure: bool) -> (String, CookieJar) {
    if let Some(existing) = jar.get(CSRF_COOKIE) {
        if !existing.value().is_empty() {
            return (existing.value().to_string(), jar);
        }
    }
    let tok = token::generate().raw;
    let cookie = Cookie::build((CSRF_COOKIE, tok.clone()))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Strict)
        .path("/")
        .build();
    (tok, jar.add(cookie))
}

/// Verify a submitted token against the CSRF cookie (constant-time).
pub fn verify(jar: &CookieJar, submitted: &str) -> bool {
    match jar.get(CSRF_COOKIE) {
        Some(cookie) => !submitted.is_empty() && constant_time_eq(cookie.value(), submitted),
        None => false,
    }
}

pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_then_verify() {
        let (tok, jar) = issue(CookieJar::new(), false);
        assert!(verify(&jar, &tok));
        assert!(!verify(&jar, "wrong"));
        assert!(!verify(&jar, ""));
    }

    #[test]
    fn verify_without_cookie_fails() {
        assert!(!verify(&CookieJar::new(), "anything"));
    }

    #[test]
    fn issue_is_idempotent_within_a_jar() {
        let (t1, jar) = issue(CookieJar::new(), false);
        let (t2, _) = issue(jar, false);
        assert_eq!(t1, t2);
    }
}
