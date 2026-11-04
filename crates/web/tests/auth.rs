//! Integration tests for local-account auth (M4a), exercised through the real
//! Axum router. Require `TEST_DATABASE_URL`; skipped otherwise.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, Response, StatusCode};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

use rusty_feed_cache::Cache;
use rusty_feed_core::config::RegistrationMode;
use rusty_feed_db::{users, Db};
use rusty_feed_web::auth::AdminUser;
use rusty_feed_web::{router, AppSettings, AppState, ReverseProxyAuthSettings};

async fn test_state() -> Option<AppState> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    let cache = Cache::in_memory().await;
    Some(AppState::new(
        db,
        cache,
        AppSettings::default(),
        String::new,
    ))
}

/// Parse `Set-Cookie` response headers into (name, value) pairs.
fn set_cookies(resp: &Response<Body>) -> Vec<(String, String)> {
    resp.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|hv| hv.to_str().ok())
        .filter_map(|s| {
            let first = s.split(';').next()?;
            let (name, value) = first.split_once('=')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

fn find_cookie<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

fn cookie_header(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

async fn body_string(resp: Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

fn unique_email() -> String {
    format!("u-{}%40example.com", uuid::Uuid::now_v7())
}

#[tokio::test]
async fn login_page_uses_the_rust_technical_surface() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let response = router(state)
        .oneshot(Request::get("/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("login-page"));
    assert!(body.contains("Return to your signal"));
    assert!(body.contains("rusty-feed / auth"));
    assert!(body.contains("Rust · Axum · PostgreSQL"));
}

#[tokio::test]
async fn demo_accounts_do_not_consume_the_admin_bootstrap_slot() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let before = users::count_non_demo(state.db.pool()).await.unwrap();
    let demo = users::create_demo(
        state.db.pool(),
        &format!("demo-{}@demo.invalid", uuid::Uuid::now_v7()),
        "Bootstrap demo",
        "user",
    )
    .await
    .unwrap();
    assert_eq!(
        users::count_non_demo(state.db.pool()).await.unwrap(),
        before
    );

    let persistent = users::create(
        state.db.pool(),
        &format!("persistent-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        users::count_non_demo(state.db.pool()).await.unwrap(),
        before + 1
    );

    sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(vec![demo.id, persistent.id])
        .execute(state.db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn reverse_proxy_auth_requires_a_trusted_immediate_peer() {
    let Some(base_state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let trusted_email = format!("proxy-{}@example.test", uuid::Uuid::now_v7());
    let spoofed_email = format!("spoof-{}@example.test", uuid::Uuid::now_v7());
    let mut settings = (*base_state.settings).clone();
    settings.reverse_proxy_auth = ReverseProxyAuthSettings {
        enabled: true,
        trusted_cidrs: vec!["10.20.0.0/16".parse().unwrap()],
        user_header: "x-auth-email".to_string(),
        name_header: "x-auth-name".to_string(),
        auto_provision: true,
        allowed_domains: vec!["example.test".to_string()],
        admin_emails: vec![trusted_email.clone()],
        logout_url: Some("/upstream/logout".to_string()),
    };
    let state = AppState::new(
        base_state.db,
        Cache::in_memory().await,
        settings,
        String::new,
    );
    let app = router(state.clone());

    let response = app
        .clone()
        .oneshot(
            Request::get("/admin")
                .header("x-auth-email", &trusted_email)
                .header("x-auth-name", "Proxy Administrator")
                .extension(ConnectInfo(
                    "10.20.3.4:4321".parse::<std::net::SocketAddr>().unwrap(),
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let provisioned = users::find_by_email(state.db.pool(), &trusted_email)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(provisioned.role, "admin");
    assert_eq!(provisioned.name.as_deref(), Some("Proxy Administrator"));

    let response = app
        .oneshot(
            Request::get("/articles")
                .header("x-auth-email", &spoofed_email)
                .extension(ConnectInfo(
                    "203.0.113.50:4321".parse::<std::net::SocketAddr>().unwrap(),
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(users::find_by_email(state.db.pool(), &spoofed_email)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn browser_navigation_redirects_to_login_but_api_keeps_json() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state);

    // A genuine document navigation (Accept: text/html) to a protected page is
    // redirected to sign-in with a same-origin return path, not shown raw JSON.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/articles")
                .header("accept", "text/html,application/xhtml+xml")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login?next=%2Farticles");

    // An API-style client on the same URL still receives the JSON envelope.
    let resp = app
        .oneshot(
            Request::get("/articles")
                .header("accept", "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(body_string(resp).await.contains("authentication required"));
}

#[tokio::test]
async fn register_login_logout_flow() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state.clone());
    let email = unique_email();

    // 1. GET /register issues a CSRF cookie.
    let resp = app
        .clone()
        .oneshot(Request::get("/register").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let csp = resp
        .headers()
        .get("content-security-policy")
        .and_then(|value| value.to_str().ok())
        .expect("default CSP");
    assert!(csp.contains("script-src 'self'"));
    assert!(!csp.contains("'unsafe-inline'"));
    let cookies = set_cookies(&resp);
    let csrf = find_cookie(&cookies, "rusty_feed_csrf")
        .expect("csrf cookie")
        .to_string();

    // 2. POST /register creates the account and starts a session.
    let body = format!("name=&email={email}&password=Str0ng-Test-Pass-9x&csrf_token={csrf}");
    let resp = app
        .clone()
        .oneshot(
            Request::post("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    cookie_header(&[("rusty_feed_csrf", &csrf), ("rusty_feed_demo", "1")]),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "register should redirect"
    );
    let response_cookies = set_cookies(&resp);
    assert_eq!(
        find_cookie(&response_cookies, "rusty_feed_demo"),
        Some(""),
        "regular registration clears a stale demo marker"
    );
    let sid = find_cookie(&response_cookies, "rusty_feed_sid")
        .expect("session cookie")
        .to_string();

    // 3. The session authenticates: `/` redirects signed-in users to the reader,
    //    which renders (and issues a CSRF cookie we reuse for logout).
    let resp = app
        .clone()
        .oneshot(
            Request::get("/")
                .header("cookie", cookie_header(&[("rusty_feed_sid", &sid)]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let resp = app
        .clone()
        .oneshot(
            Request::get("/articles")
                .header("cookie", cookie_header(&[("rusty_feed_sid", &sid)]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let home_csrf = find_cookie(&set_cookies(&resp), "rusty_feed_csrf")
        .expect("articles csrf")
        .to_string();
    let html = body_string(resp).await;
    for forbidden in ["<script>", " style=", " onclick=", " onsubmit="] {
        assert!(
            !html.contains(forbidden),
            "authenticated shell contains inline CSP violation: {forbidden}"
        );
    }

    // 4. Logout deletes the session server-side.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/logout")
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    cookie_header(&[
                        ("rusty_feed_sid", &sid),
                        ("rusty_feed_csrf", &home_csrf),
                        ("rusty_feed_demo", "1"),
                    ]),
                )
                .body(Body::from(format!("csrf_token={home_csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        find_cookie(&set_cookies(&resp), "rusty_feed_demo"),
        Some(""),
        "logout clears the demo marker"
    );

    // 5. The old session cookie no longer authenticates (session was deleted).
    let resp = app
        .oneshot(
            Request::get("/articles")
                .header("cookie", cookie_header(&[("rusty_feed_sid", &sid)]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_rejects_wrong_password() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state);
    let email = unique_email();

    // Register first.
    let resp = app
        .clone()
        .oneshot(Request::get("/register").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let csrf = find_cookie(&set_cookies(&resp), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    app.clone()
        .oneshot(
            Request::post("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", cookie_header(&[("rusty_feed_csrf", &csrf)]))
                .body(Body::from(format!(
                    "name=&email={email}&password=Str0ng-Test-Pass-9x&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    // Wrong password re-renders the form (no session cookie set).
    let resp = app
        .oneshot(
            Request::post("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", cookie_header(&[("rusty_feed_csrf", &csrf)]))
                .body(Body::from(format!(
                    "email={email}&password=WRONGpassword&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(find_cookie(&set_cookies(&resp), "rusty_feed_sid").is_none());
    assert!(body_string(resp)
        .await
        .contains("Invalid email or password"));
}

#[tokio::test]
async fn post_without_csrf_is_forbidden() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state);

    let resp = app
        .oneshot(
            Request::post("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "email=x%40y.com&password=whatever&csrf_token=nope",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rate_limit_blocks_excessive_logins() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state); // fresh limiter: 10/min per IP
    let mut last = StatusCode::OK;
    for _ in 0..11 {
        let resp = app
            .clone()
            .oneshot(
                Request::post("/login")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("x-forwarded-for", "203.0.113.7")
                    .body(Body::from("email=x%40y.com&password=z&csrf_token=nope"))
                    .unwrap(),
            )
            .await
            .unwrap();
        last = resp.status();
    }
    // First 10 fail CSRF (403) but pass rate limit; the 11th is throttled.
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn api_token_create_list_revoke() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state.clone());
    let email = unique_email();

    // Register and capture the session.
    let resp = app
        .clone()
        .oneshot(Request::get("/register").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let csrf = find_cookie(&set_cookies(&resp), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", cookie_header(&[("rusty_feed_csrf", &csrf)]))
                .body(Body::from(format!(
                    "name=&email={email}&password=Str0ng-Test-Pass-9x&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let sid = find_cookie(&set_cookies(&resp), "rusty_feed_sid")
        .unwrap()
        .to_string();

    // GET the tokens page (issues a CSRF cookie).
    let resp = app
        .clone()
        .oneshot(
            Request::get("/settings/tokens")
                .header("cookie", cookie_header(&[("rusty_feed_sid", &sid)]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let tcsrf = find_cookie(&set_cookies(&resp), "rusty_feed_csrf")
        .unwrap()
        .to_string();

    // Create a token; the raw value is shown once.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/settings/tokens")
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    cookie_header(&[("rusty_feed_sid", &sid), ("rusty_feed_csrf", &tcsrf)]),
                )
                .body(Body::from(format!("name=Reeder&csrf_token={tcsrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("cache-control").unwrap(), "no-store");
    let body = body_string(resp).await;
    assert!(body.contains("Reeder"));
    assert!(body.contains("Copy this token now"));
    assert!(body.contains("Rusty Feed"));

    // The token exists in the database.
    let email_local = email.replace("%40", "@");
    let user = rusty_feed_db::users::find_by_email(state.db.pool(), &email_local)
        .await
        .unwrap()
        .unwrap();
    let toks = rusty_feed_db::api_tokens::list_for_user(state.db.pool(), user.id)
        .await
        .unwrap();
    assert_eq!(toks.len(), 1);
    assert!(toks[0].revoked_at.is_none());

    // Revoke it.
    let resp = app
        .oneshot(
            Request::post(format!("/settings/tokens/{}/revoke", toks[0].id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    cookie_header(&[("rusty_feed_sid", &sid), ("rusty_feed_csrf", &tcsrf)]),
                )
                .body(Body::from(format!("csrf_token={tcsrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_string(resp).await.contains("Token revoked."));
    let toks = rusty_feed_db::api_tokens::list_for_user(state.db.pool(), user.id)
        .await
        .unwrap();
    assert!(toks[0].revoked_at.is_some(), "token should be revoked");
}

#[tokio::test]
async fn admin_extractor_enforces_role() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };

    // A tiny router with an admin-guarded route.
    async fn admin_only(_: AdminUser) -> &'static str {
        "secret"
    }
    let app = Router::new()
        .route("/admin-only", get(admin_only))
        .with_state(state.clone());
    let auth_app = router(state.clone());
    let email = unique_email();

    // Register a normal user and capture their session.
    let resp = auth_app
        .clone()
        .oneshot(Request::get("/register").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let csrf = find_cookie(&set_cookies(&resp), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let resp = auth_app
        .oneshot(
            Request::post("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", cookie_header(&[("rusty_feed_csrf", &csrf)]))
                .body(Body::from(format!(
                    "name=&email={email}&password=Str0ng-Test-Pass-9x&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let sid = find_cookie(&set_cookies(&resp), "rusty_feed_sid")
        .unwrap()
        .to_string();

    // Normal user -> 403 (unless they happened to be the very first user).
    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin-only")
                .header("cookie", cookie_header(&[("rusty_feed_sid", &sid)]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let email_local = email.replace("%40", "@");
    let user = rusty_feed_db::users::find_by_email(state.db.pool(), &email_local)
        .await
        .unwrap()
        .unwrap();
    if user.role != "admin" {
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // Promote to admin -> 200.
    rusty_feed_db::users::set_role(state.db.pool(), user.id, "admin")
        .await
        .unwrap();
    let resp = app
        .oneshot(
            Request::get("/admin-only")
                .header("cookie", cookie_header(&[("rusty_feed_sid", &sid)]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Unauthenticated -> 401.
    let resp = Router::new()
        .route("/admin-only", get(admin_only))
        .with_state(state)
        .oneshot(Request::get("/admin-only").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn invitation_is_one_use_and_new_user_starts_without_subscriptions() {
    let Some(mut state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    std::sync::Arc::make_mut(&mut state.settings).registration_mode = RegistrationMode::Invite;
    let inviter_email = format!("admin-{}@example.com", uuid::Uuid::now_v7());
    let inviter =
        rusty_feed_db::users::create(state.db.pool(), &inviter_email, None, None, "admin", None)
            .await
            .unwrap();
    let invitation = rusty_feed_web::auth::token::generate();
    let invited_email = format!("invited-{}@example.com", uuid::Uuid::now_v7());
    rusty_feed_db::invitations::create(
        state.db.pool(),
        inviter.id,
        &invitation.hash,
        Some(&invited_email),
        7,
    )
    .await
    .unwrap();
    let app = router(state.clone());

    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/register?invite={}", invitation.raw))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let csrf = find_cookie(&set_cookies(&resp), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    assert!(body_string(resp).await.contains("Create your account"));

    let resp =
        app.clone()
            .oneshot(
                Request::post("/register")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("cookie", cookie_header(&[("rusty_feed_csrf", &csrf)]))
                    .body(Body::from(format!(
                    "name=Invited&email={}&password=Str0ng-Test-Pass-9x&invite_token={}&csrf_token={csrf}",
                    invited_email.replace('@', "%40"), invitation.raw
                )))
                    .unwrap(),
            )
            .await
            .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let sid = find_cookie(&set_cookies(&resp), "rusty_feed_sid")
        .unwrap()
        .to_string();
    let user = rusty_feed_db::users::find_by_email(state.db.pool(), &invited_email)
        .await
        .unwrap()
        .unwrap();
    let subscription_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_feeds WHERE user_id = $1")
            .bind(user.id)
            .fetch_one(state.db.pool())
            .await
            .unwrap();
    assert_eq!(
        subscription_count, 0,
        "featured defaults are reserved for demo accounts"
    );
    assert!(!rusty_feed_db::invitations::is_valid(
        state.db.pool(),
        &invitation.hash,
        Some(&invited_email)
    )
    .await
    .unwrap());

    rusty_feed_db::sessions::create(
        state.db.pool(),
        user.id,
        "a-different-session-hash",
        time::OffsetDateTime::now_utc() + time::Duration::days(30),
    )
    .await
    .unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::get("/settings/sessions")
                .header("cookie", cookie_header(&[("rusty_feed_sid", &sid)]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let session_csrf = find_cookie(&set_cookies(&resp), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    assert!(body_string(resp).await.contains("This session"));
    let resp = app
        .clone()
        .oneshot(
            Request::post("/settings/sessions/revoke-others")
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    cookie_header(&[("rusty_feed_sid", &sid), ("rusty_feed_csrf", &session_csrf)]),
                )
                .body(Body::from(format!("csrf_token={session_csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_string(resp)
        .await
        .contains("Signed out 1 other session"));
    assert_eq!(
        rusty_feed_db::sessions::list_for_user(state.db.pool(), user.id)
            .await
            .unwrap()
            .len(),
        1
    );

    let resp = app
        .oneshot(
            Request::get(format!("/register?invite={}", invitation.raw))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_string(resp)
        .await
        .contains("Registration is currently closed"));
}

#[tokio::test]
async fn password_reset_replaces_the_local_password_once() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let login = router(state.clone())
        .oneshot(Request::get("/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(body_string(login).await.contains("Forgot password?"));

    let forgot = router(state.clone())
        .oneshot(
            Request::get("/forgot-password")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forgot.status(), StatusCode::OK);
    assert!(body_string(forgot)
        .await
        .contains("not configured to send email"));

    let email = format!("reset-{}@example.test", uuid::Uuid::now_v7());
    let password_hash =
        rusty_feed_web::auth::password::hash_password("Old-Password-9x").expect("hash");
    let user = users::create(
        state.db.pool(),
        &email,
        None,
        None,
        "user",
        Some(&password_hash),
    )
    .await
    .unwrap();
    let generated = rusty_feed_web::auth::token::generate();
    rusty_feed_db::password_resets::issue(
        state.db.pool(),
        user.id,
        &generated.hash,
        time::OffsetDateTime::now_utc() + time::Duration::minutes(30),
    )
    .await
    .unwrap();

    let page = router(state.clone())
        .oneshot(
            Request::get(format!("/reset-password?token={}", generated.raw))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let cookies = set_cookies(&page);
    let csrf = find_cookie(&cookies, "rusty_feed_csrf")
        .expect("csrf")
        .to_string();
    let body = body_string(page).await;
    assert!(body.contains("Choose a new password"));

    let reset = router(state.clone())
        .oneshot(
            Request::post("/reset-password")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", cookie_header(&[("rusty_feed_csrf", &csrf)]))
                .body(Body::from(format!(
                    "csrf_token={csrf}&token={}&password=New-Password-9x",
                    generated.raw
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reset.status(), StatusCode::SEE_OTHER);

    let again = router(state.clone())
        .oneshot(
            Request::post("/reset-password")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", cookie_header(&[("rusty_feed_csrf", &csrf)]))
                .body(Body::from(format!(
                    "csrf_token={csrf}&token={}&password=Another-Password-9x",
                    generated.raw
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::OK);
    assert!(body_string(again).await.contains("invalid or has expired"));

    let stored = users::find_by_id(state.db.pool(), user.id)
        .await
        .unwrap()
        .unwrap();
    assert!(rusty_feed_web::auth::password::verify_password(
        "New-Password-9x",
        stored.password_hash.as_deref().unwrap()
    ));
}
