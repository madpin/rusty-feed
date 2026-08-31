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
