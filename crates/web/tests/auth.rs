//! Integration tests for local-account auth (M4a), exercised through the real
//! Axum router. Require `TEST_DATABASE_URL`; skipped otherwise.

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

use rustss_cache::Cache;
use rustss_db::Db;
use rustss_web::auth::AdminUser;
use rustss_web::{router, AppSettings, AppState};

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
    let cookies = set_cookies(&resp);
    let csrf = find_cookie(&cookies, "rustss_csrf")
        .expect("csrf cookie")
        .to_string();

    // 2. POST /register creates the account and starts a session.
    let body = format!("name=&email={email}&password=password123&csrf_token={csrf}");
    let resp = app
        .clone()
        .oneshot(
            Request::post("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", cookie_header(&[("rustss_csrf", &csrf)]))
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
    let sid = set_cookies(&resp);
    let sid = find_cookie(&sid, "rustss_sid")
        .expect("session cookie")
        .to_string();

    // 3. The session authenticates: `/` redirects signed-in users to the reader,
    //    which renders (and issues a CSRF cookie we reuse for logout).
    let resp = app
        .clone()
        .oneshot(
            Request::get("/")
                .header("cookie", cookie_header(&[("rustss_sid", &sid)]))
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
                .header("cookie", cookie_header(&[("rustss_sid", &sid)]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let home_csrf = find_cookie(&set_cookies(&resp), "rustss_csrf")
        .expect("articles csrf")
        .to_string();

    // 4. Logout deletes the session server-side.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/logout")
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    cookie_header(&[("rustss_sid", &sid), ("rustss_csrf", &home_csrf)]),
                )
                .body(Body::from(format!("csrf_token={home_csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // 5. The old session cookie no longer authenticates (session was deleted).
    let resp = app
        .oneshot(
            Request::get("/articles")
                .header("cookie", cookie_header(&[("rustss_sid", &sid)]))
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
    let csrf = find_cookie(&set_cookies(&resp), "rustss_csrf")
        .unwrap()
        .to_string();
    app.clone()
        .oneshot(
            Request::post("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", cookie_header(&[("rustss_csrf", &csrf)]))
                .body(Body::from(format!(
                    "name=&email={email}&password=password123&csrf_token={csrf}"
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
                .header("cookie", cookie_header(&[("rustss_csrf", &csrf)]))
                .body(Body::from(format!(
                    "email={email}&password=WRONGpassword&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(find_cookie(&set_cookies(&resp), "rustss_sid").is_none());
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
    let csrf = find_cookie(&set_cookies(&resp), "rustss_csrf")
        .unwrap()
        .to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", cookie_header(&[("rustss_csrf", &csrf)]))
                .body(Body::from(format!(
                    "name=&email={email}&password=password123&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let sid = find_cookie(&set_cookies(&resp), "rustss_sid")
        .unwrap()
        .to_string();

    // GET the tokens page (issues a CSRF cookie).
    let resp = app
        .clone()
        .oneshot(
            Request::get("/settings/tokens")
                .header("cookie", cookie_header(&[("rustss_sid", &sid)]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let tcsrf = find_cookie(&set_cookies(&resp), "rustss_csrf")
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
                    cookie_header(&[("rustss_sid", &sid), ("rustss_csrf", &tcsrf)]),
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
    assert!(body.contains("RustSS"));

    // The token exists in the database.
    let email_local = email.replace("%40", "@");
    let user = rustss_db::users::find_by_email(state.db.pool(), &email_local)
        .await
        .unwrap()
        .unwrap();
    let toks = rustss_db::api_tokens::list_for_user(state.db.pool(), user.id)
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
                    cookie_header(&[("rustss_sid", &sid), ("rustss_csrf", &tcsrf)]),
                )
                .body(Body::from(format!("csrf_token={tcsrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_string(resp).await.contains("Token revoked."));
    let toks = rustss_db::api_tokens::list_for_user(state.db.pool(), user.id)
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
    let csrf = find_cookie(&set_cookies(&resp), "rustss_csrf")
        .unwrap()
        .to_string();
    let resp = auth_app
        .oneshot(
            Request::post("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", cookie_header(&[("rustss_csrf", &csrf)]))
                .body(Body::from(format!(
                    "name=&email={email}&password=password123&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let sid = find_cookie(&set_cookies(&resp), "rustss_sid")
        .unwrap()
        .to_string();

    // Normal user -> 403 (unless they happened to be the very first user).
    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin-only")
                .header("cookie", cookie_header(&[("rustss_sid", &sid)]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let email_local = email.replace("%40", "@");
    let user = rustss_db::users::find_by_email(state.db.pool(), &email_local)
        .await
        .unwrap()
        .unwrap();
    if user.role != "admin" {
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // Promote to admin -> 200.
    rustss_db::users::set_role(state.db.pool(), user.id, "admin")
        .await
        .unwrap();
    let resp = app
        .oneshot(
            Request::get("/admin-only")
                .header("cookie", cookie_header(&[("rustss_sid", &sid)]))
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
