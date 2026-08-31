use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use rustss_cache::Cache;
use rustss_db::models::NewArticle;
use rustss_db::{api_tokens, articles, feeds, subscriptions, users, Db};
use rustss_web::{router, AppSettings, AppState};

async fn state() -> Option<AppState> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.ok()?;
    db.migrate().await.ok()?;
    Some(AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    ))
}

/// State with a finite saved-search quota (for the 409 test).
async fn state_with_saved_search_cap(cap: u32) -> Option<AppState> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.ok()?;
    db.migrate().await.ok()?;
    let settings = AppSettings {
        quotas: rustss_core::quotas::Quotas {
            max_saved_searches: cap,
            ..rustss_core::quotas::Quotas::default()
        },
        ..AppSettings::default()
    };
    Some(AppState::new(
        db,
        Cache::in_memory().await,
        settings,
        String::new,
    ))
}

async fn session_cookie(state: &AppState, user_id: uuid::Uuid) -> String {
    let raw = format!("sess-{}", uuid::Uuid::now_v7());
    let hash = rustss_web::auth::token::hash(&raw);
    let expires = time::OffsetDateTime::now_utc() + time::Duration::days(1);
    rustss_db::sessions::create(state.db.pool(), user_id, &hash, expires)
        .await
        .unwrap();
    format!("rustss_sid={raw}")
}

#[tokio::test]
async fn demo_reset_deletes_only_demo_users() {
    let Some(state) = state().await else { return };
    let real = users::create(
        state.db.pool(),
        &format!("real-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let demo = users::create_demo(
        state.db.pool(),
        &format!("demo-{}@demo.invalid", uuid::Uuid::now_v7()),
        "Demo user",
        "user",
    )
    .await
    .unwrap();
    users::delete_all_demo(state.db.pool()).await.unwrap();
    assert!(
        users::find_by_id(state.db.pool(), real.id)
            .await
            .unwrap()
            .is_some(),
        "the real user must survive the demo reset"
    );
    assert!(
        users::find_by_id(state.db.pool(), demo.id)
            .await
            .unwrap()
            .is_none(),
        "the demo user must be deleted"
    );
}

#[tokio::test]
async fn read_only_admin_can_read_but_not_write_admin_routes() {
    let Some(state) = state().await else { return };
    let ro = users::create(
        state.db.pool(),
        &format!("ro-admin-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "admin_readonly",
        None,
    )
    .await
    .unwrap();
    let cookie = session_cookie(&state, ro.id).await;
    let app = router(state.clone());

    // Read-only admin can GET every admin page.
    for path in [
        "/admin",
        "/admin/users",
        "/admin/feeds",
        "/admin/feeds/blocklist",
        "/admin/catalog",
        "/admin/acquisition",
        "/admin/activity",
        "/admin/logs",
        "/admin/costs",
        "/admin/ai",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get(path)
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "GET {path} should be 200"
        );
    }

    // Every admin write is a structural 403 (the extractor rejects before the
    // handler body, so no CSRF token is needed to prove the gate). Covered
    // route-by-route per §7.6 DoD.
    let id = uuid::Uuid::now_v7();
    let writes = [
        format!("/admin/users/{id}/role"),
        "/admin/invitations".to_string(),
        format!("/admin/invitations/{id}/revoke"),
        "/admin/feeds/blocklist".to_string(),
        format!("/admin/feeds/blocklist/{id}/delete"),
        format!("/admin/feeds/{id}/block"),
        format!("/admin/feeds/{id}/status"),
        format!("/admin/articles/{id}/takedown"),
        format!("/admin/abuse/{id}/resolve"),
        "/admin/catalog".to_string(),
        format!("/admin/catalog/{id}"),
        "/admin/maintenance".to_string(),
        "/admin/acquisition".to_string(),
        "/admin/acquisition/assign".to_string(),
        format!("/admin/acquisition/{id}/delete"),
        "/admin/ai".to_string(),
        format!("/admin/ai/{id}/toggle"),
        format!("/admin/ai/{id}/delete"),
    ];
    for path in &writes {
        let response = app
            .clone()
            .oneshot(
                Request::post(path)
                    .header("cookie", &cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("csrf_token=x"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "POST {path} must be 403 for a read-only admin"
        );
    }
}

#[tokio::test]
async fn landing_page_renders_for_anonymous_visitors() {
    let Some(state) = state().await else { return };
    let app = router(state.clone());
    let response = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("Create free account"), "hero CTA present");
    assert!(body.contains("Miniflux"), "comparison present");
    assert!(body.contains("Semantic"), "features present");
}

#[tokio::test]
async fn first_run_wizard_routes_and_persists() {
    let Some(state) = state().await else { return };
    let user = users::create(
        state.db.pool(),
        &format!("welcome-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let cookie = session_cookie(&state, user.id).await;
    let app = router(state.clone());

    // The wizard page renders for a signed-in user.
    let welcome = app
        .clone()
        .oneshot(
            Request::get("/welcome")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(welcome.status(), StatusCode::OK);
    let csrf = set_cookies_value(&welcome, "rustss_csrf").unwrap_or_else(|| "x".to_string());
    let welcome_cookie = format!("{cookie}; rustss_csrf={csrf}");

    // Finishing the wizard stamps onboarded_at.
    let finish = app
        .oneshot(
            Request::post("/welcome")
                .header("cookie", &welcome_cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "csrf_token={csrf}&theme=ocean&reading_mode=inline&font_family=serif&article_density=compact"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(finish.status(), StatusCode::SEE_OTHER);
    let refreshed = users::find_by_id(state.db.pool(), user.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        refreshed.onboarded_at.is_some(),
        "finishing the wizard stamps onboarded_at"
    );
}

fn set_cookies_value(resp: &axum::response::Response, name: &str) -> Option<String> {
    resp.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|hv| hv.to_str().ok())
        .find_map(|s| {
            let s = s.split(';').next()?;
            let (k, v) = s.split_once('=')?;
            (k.trim() == name).then(|| v.trim().to_string())
        })
}

#[tokio::test]
async fn robots_and_sitemap_are_served() {
    let Some(state) = state().await else { return };
    let app = router(state.clone());
    let robots = app
        .clone()
        .oneshot(Request::get("/robots.txt").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(robots.status(), StatusCode::OK);
    let robots_body = robots.into_body().collect().await.unwrap().to_bytes();
    let robots_body = String::from_utf8_lossy(&robots_body);
    assert!(
        robots_body.contains("Disallow: /admin"),
        "app is disallowed"
    );
    assert!(robots_body.contains("Sitemap:"), "points at the sitemap");

    let sitemap = app
        .oneshot(Request::get("/sitemap.xml").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(sitemap.status(), StatusCode::OK);
    let sitemap_body = sitemap.into_body().collect().await.unwrap().to_bytes();
    let sitemap_body = String::from_utf8_lossy(&sitemap_body);
    assert!(sitemap_body.contains("<urlset"));
    assert!(sitemap_body.contains("/terms"), "lists public pages");
}

#[tokio::test]
async fn legal_pages_are_public() {
    let Some(state) = state().await else { return };
    let app = router(state.clone());
    for path in ["/terms", "/privacy", "/acceptable-use", "/abuse"] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path} should be public");
    }
}

#[tokio::test]
async fn last_admin_cannot_self_delete() {
    let Some(state) = state().await else { return };
    // A fresh instance whose only admin is created here — deletion must be blocked.
    let admin = users::create(
        state.db.pool(),
        &format!("solo-admin-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "admin",
        None,
    )
    .await
    .unwrap();
    // Guard only blocks when this is the last admin; other tests may create
    // admins, so assert the guard logic directly rather than global state.
    let admins_before: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'admin'")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    let deleted = users::delete(state.db.pool(), admin.id).await.unwrap();
    if admins_before <= 1 {
        assert!(!deleted, "the last admin must not be deletable");
    } else {
        assert!(deleted, "a non-last admin may be deleted");
    }
}

#[tokio::test]
async fn blocklisted_feed_is_rejected_on_add() {
    let Some(state) = state().await else { return };
    let user = users::create(
        state.db.pool(),
        &format!("block-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let raw = format!("block-token-{}", uuid::Uuid::now_v7());
    api_tokens::create(
        state.db.pool(),
        user.id,
        "test",
        &rustss_web::auth::token::hash(&raw),
        &[],
    )
    .await
    .unwrap();
    let host = format!("blocked-{}.example", uuid::Uuid::now_v7());
    rustss_db::feed_blocklist::add(
        state.db.pool(),
        "domain",
        &host,
        Some("test"),
        "admin",
        None,
    )
    .await
    .unwrap();
    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::post("/api/v1/feeds")
                .header("authorization", format!("Bearer {raw}"))
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"url":"https://{host}/feed.xml"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn per_user_quota_returns_409_past_the_cap() {
    let Some(state) = state_with_saved_search_cap(1).await else {
        return;
    };
    let user = users::create(
        state.db.pool(),
        &format!("quota-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let raw = format!("quota-token-{}", uuid::Uuid::now_v7());
    api_tokens::create(
        state.db.pool(),
        user.id,
        "test",
        &rustss_web::auth::token::hash(&raw),
        &[],
    )
    .await
    .unwrap();
    let app = router(state.clone());

    // First saved search is within the cap.
    let first = app
        .clone()
        .oneshot(
            Request::post("/api/v1/saved-searches")
                .header("authorization", format!("Bearer {raw}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"one","query":"rust"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    // Second exceeds the cap → 409 Conflict (QuotaExceeded).
    let second = app
        .oneshot(
            Request::post("/api/v1/saved-searches")
                .header("authorization", format!("Bearer {raw}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"two","query":"go"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn bearer_resources_are_tenant_scoped_and_state_is_mutable() {
    let Some(state) = state().await else { return };
    let user = users::create(
        state.db.pool(),
        &format!("api-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!("https://api.example/{}/feed.xml", uuid::Uuid::now_v7()),
        "API feed",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(state.db.pool(), user.id, feed.id, None)
        .await
        .unwrap();
    let (article, _) = articles::upsert(
        state.db.pool(),
        &NewArticle {
            feed_id: feed.id,
            url: format!(
                "https://api.example/{}/article?id=42&utm_source=legacy",
                uuid::Uuid::now_v7()
            ),
            comments_url: None,
            guid: None,
            title: "Native API article".into(),
            author: None,
            content: "<p>Body</p>".into(),
            excerpt: Some("Body".into()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    let raw = format!("api-token-{}", uuid::Uuid::now_v7());
    api_tokens::create(
        state.db.pool(),
        user.id,
        "test",
        &rustss_web::auth::token::hash(&raw),
        &[],
    )
    .await
    .unwrap();
    let app = router(state.clone());

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/articles")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/articles")
                .header("authorization", format!("Bearer {raw}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let listed = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == article.id.to_string())
        .unwrap();
    assert_eq!(
        listed["url"].as_str().unwrap().split('?').nth(1),
        Some("id=42")
    );

    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/articles/{}", article.id))
                .header("authorization", format!("Bearer {raw}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let detail: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(detail["data"]["url"].as_str().unwrap().ends_with("?id=42"));
    assert!(detail["data"]["publisher_url"]
        .as_str()
        .unwrap()
        .contains("utm_source=legacy"));

    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/categories")
                .header("x-auth-token", &raw)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"API folder"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/articles/{}/state", article.id))
                .header("authorization", format!("Bearer {raw}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"read":true,"bookmarked":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        rustss_db::read_articles::is_read(state.db.pool(), user.id, article.id)
            .await
            .unwrap()
    );
    assert!(
        rustss_db::bookmarks::is_bookmarked(state.db.pool(), user.id, article.id)
            .await
            .unwrap()
    );
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_audit_events WHERE user_id = $1 AND operation IN ('categories.create', 'articles.state')",
    )
    .bind(user.id)
    .fetch_one(state.db.pool())
    .await
    .unwrap();
    assert_eq!(audit_count, 2);

    let disabled = router(AppState::new(
        state.db.clone(),
        Cache::in_memory().await,
        AppSettings {
            api_enabled: false,
            ..AppSettings::default()
        },
        String::new,
    ));
    let response = disabled
        .oneshot(
            Request::get("/api/v1/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
