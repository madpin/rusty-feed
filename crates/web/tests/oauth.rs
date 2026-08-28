//! OAuth login integration test (M4b-2) using a mocked provider (wiremock).
//! Requires `TEST_DATABASE_URL`; skipped otherwise.

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rustss_cache::Cache;
use rustss_core::Secret;
use rustss_db::{oauth_accounts, users, Db};
use rustss_web::{router, AppSettings, AppState, OAuthProvider, ProviderKind};

fn set_cookies(resp: &Response<Body>) -> Vec<(String, String)> {
    resp.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|hv| hv.to_str().ok())
        .filter_map(|s| {
            let first = s.split(';').next()?;
            let (n, v) = first.split_once('=')?;
            Some((n.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

fn find<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

async fn body_string(resp: Response<Body>) -> String {
    let b = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&b).to_string()
}

#[tokio::test]
async fn oauth_generic_login_creates_and_links_user() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&db_url).await.expect("connect");
    db.migrate().await.expect("migrate");

    // A mock OIDC provider: token + userinfo endpoints. Both the email and the
    // subject are unique per run so the test is idempotent against a shared DB.
    let run_id = uuid::Uuid::now_v7();
    let email = format!("oauth-{run_id}@example.com");
    let subject = format!("subject-{run_id}");
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"access_token": "test-access-token", "token_type": "bearer"}),
            ),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/userinfo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sub": subject,
            "email": email,
            "name": "Test User",
        })))
        .mount(&mock)
        .await;

    let provider = OAuthProvider {
        slug: "test".to_string(),
        display_name: "Test SSO".to_string(),
        client_id: "client-id".to_string(),
        client_secret: Secret::new("client-secret"),
        auth_url: format!("{}/authorize", mock.uri()),
        token_url: format!("{}/token", mock.uri()),
        userinfo_url: format!("{}/userinfo", mock.uri()),
        scopes: "openid email".to_string(),
        kind: ProviderKind::Oidc,
    };
    let settings = AppSettings {
        base_url: "http://localhost:8080".to_string(),
        oauth_providers: vec![provider],
        ..Default::default()
    };
    let state = AppState::new(db, Cache::in_memory().await, settings, String::new);
    let app = router(state.clone());

    // 1. Start: redirects to the provider and sets state + PKCE cookies.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/auth/oauth/test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookies = set_cookies(&resp);
    let oauth_state = find(&cookies, "rustss_oauth_state")
        .expect("state cookie")
        .to_string();
    let pkce = find(&cookies, "rustss_oauth_pkce")
        .expect("pkce cookie")
        .to_string();

    // 2. Callback with the matching state → exchanges code, creates + links user.
    let resp = app
        .oneshot(
            Request::get(format!(
                "/auth/oauth/test/callback?code=abc&state={oauth_state}"
            ))
            .header(
                "cookie",
                format!("rustss_oauth_state={oauth_state}; rustss_oauth_pkce={pkce}"),
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "callback should sign in and redirect"
    );
    assert!(
        find(&set_cookies(&resp), "rustss_sid").is_some(),
        "a session cookie should be set"
    );

    // 3. The user and the OAuth link now exist.
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .expect("user created");
    let account = oauth_accounts::find(state.db.pool(), "test", &subject)
        .await
        .unwrap()
        .expect("oauth account linked");
    assert_eq!(account.user_id, user.id);
}

#[tokio::test]
async fn oauth_callback_rejects_bad_state() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&db_url).await.expect("connect");
    db.migrate().await.expect("migrate");

    let provider = OAuthProvider {
        slug: "test".to_string(),
        display_name: "Test SSO".to_string(),
        client_id: "client-id".to_string(),
        client_secret: Secret::new("client-secret"),
        auth_url: "http://example.invalid/authorize".to_string(),
        token_url: "http://example.invalid/token".to_string(),
        userinfo_url: "http://example.invalid/userinfo".to_string(),
        scopes: "openid email".to_string(),
        kind: ProviderKind::Oidc,
    };
    let settings = AppSettings {
        oauth_providers: vec![provider],
        ..Default::default()
    };
    let state = AppState::new(db, Cache::in_memory().await, settings, String::new);
    let app = router(state);

    // State cookie says "expected" but the query says "attacker" → 403 (no token
    // exchange is attempted, so the invalid provider URL is never contacted).
    let resp = app
        .oneshot(
            Request::get("/auth/oauth/test/callback?code=abc&state=attacker")
                .header("cookie", "rustss_oauth_state=expected; rustss_oauth_pkce=v")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let _ = body_string(resp).await;
}
