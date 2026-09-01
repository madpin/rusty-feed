//! Public article-share lifecycle and privacy-boundary integration tests.

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

use rusty_feed_cache::Cache;
use rusty_feed_db::models::NewArticle;
use rusty_feed_db::{articles, feeds, subscriptions, users, Db};
use rusty_feed_web::{router, AppSettings, AppState};

async fn test_state() -> Option<AppState> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(
        AppState::new(
            db,
            Cache::in_memory().await,
            AppSettings::default(),
            String::new,
        )
        .with_public_share_secret(b"public-share-test-secret-that-is-long-enough"),
    )
}

fn set_cookies(resp: &Response<Body>) -> Vec<(String, String)> {
    resp.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| {
            let (name, value) = value.split(';').next()?.split_once('=')?;
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

fn cookie<'a>(values: &'a [(String, String)], name: &str) -> Option<&'a str> {
    values
        .iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, value)| value.as_str())
}

async fn body_string(response: Response<Body>) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

async fn register(app: &Router) -> (String, String) {
    let email = format!("share-{}%40example.com", uuid::Uuid::now_v7());
    let response = app
        .clone()
        .oneshot(Request::get("/register").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let csrf = cookie(&set_cookies(&response), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let response = app
        .clone()
        .oneshot(
            Request::post("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rusty_feed_csrf={csrf}"))
                .body(Body::from(format!(
                    "name=&email={email}&password=Str0ng-Test-Pass-9x&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    (
        cookie(&set_cookies(&response), "rusty_feed_sid")
            .unwrap()
            .to_string(),
        email.replace("%40", "@"),
    )
}

async fn seed_article(state: &AppState, email: &str) -> uuid::Uuid {
    let pool = state.db.pool();
    let user = users::find_by_email(pool, email).await.unwrap().unwrap();
    let feed_url = format!("https://shares.example/{}/feed.xml", uuid::Uuid::now_v7());
    let (feed, _) = feeds::upsert_by_url(pool, &feed_url, "Share-safe feed", None)
        .await
        .unwrap();
    subscriptions::subscribe(pool, user.id, feed.id, None)
        .await
        .unwrap();
    let (article, _) = articles::upsert(
        pool,
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://shares.example/story/{}?utm_source=test", uuid::Uuid::now_v7()),
            comments_url: None,
            guid: Some(uuid::Uuid::now_v7().to_string()),
            title: "A private reader shares safely".to_string(),
            author: Some("Ada Reader".to_string()),
            content: "<p>Useful shared text.</p><img src=\"https://tracker.example/hero.jpg\"><script>alert(1)</script>".to_string(),
            excerpt: Some("Useful shared text.".to_string()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    article.id
}

fn public_path(article_html: &str) -> String {
    let prefix = "value=\"http://localhost:8080/shared/";
    let start = article_html.find(prefix).expect("share URL rendered") + "value=\"".len();
    let end = article_html[start..].find('"').unwrap() + start;
    article_html[start..end]
        .strip_prefix("http://localhost:8080")
        .unwrap()
        .to_string()
}

async fn article_page(app: &Router, sid: &str, article_id: uuid::Uuid) -> (String, String, String) {
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/articles/{article_id}"))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = cookie(&set_cookies(&response), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let cache_control = response
        .headers()
        .get("cache-control")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (csrf, body_string(response).await, cache_control)
}

#[tokio::test]
async fn share_is_public_private_by_default_and_revocable() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let article_id = seed_article(&state, &email).await;
    let (csrf, initial, owner_cache_control) = article_page(&app, &sid, article_id).await;
    assert_eq!(owner_cache_control, "private, no-store");
    assert!(initial.contains("Public share"));
    assert!(initial.contains("class=\"reader-toolbar\""));
    assert!(initial.contains("class=\"share-controls reader-share\""));
    assert!(initial.contains("External images (may contact publisher servers)"));

    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{article_id}/share"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"))
                .body(Body::from(format!(
                    "csrf_token={csrf}&slug=safe-copy&content_source=rss&lifetime_days=7&show_title=on&show_metadata=on&show_source_link=on"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let (_, owner_html, _) = article_page(&app, &sid, article_id).await;
    let first_path = public_path(&owner_html);
    assert!(owner_html.contains("Rotate and update link"));
    let response = app
        .clone()
        .oneshot(Request::get(&first_path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("cache-control").unwrap(),
        "private, no-store, max-age=0"
    );
    assert_eq!(
        response.headers().get("x-robots-tag").unwrap(),
        "noindex, nofollow, noarchive"
    );
    let public_html = body_string(response).await;
    assert!(public_html.contains("A private reader shares safely"));
    assert!(public_html.contains("Ada Reader"));
    assert!(public_html.contains("Useful shared text."));
    assert!(public_html.contains("data-reader-surface"));
    assert!(public_html.contains("class=\"reader-view\""));
    assert!(public_html.contains("class=\"reader-header\""));
    assert!(public_html.contains("class=\"reader-article-copy\""));
    assert!(!public_html.contains("class=\"reader-toolbar\""));
    assert!(public_html.contains("External images were omitted"));
    assert!(!public_html.contains("tracker.example"));
    assert!(!public_html.contains("<script"));
    assert!(!public_html.contains(email.as_str()));

    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{article_id}/share"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"))
                .body(Body::from(format!(
                    "csrf_token={csrf}&slug=replacement&content_source=article_page&lifetime_days=30&infinite=on&include_images=on"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let (_, owner_html, _) = article_page(&app, &sid, article_id).await;
    let second_path = public_path(&owner_html);
    assert_ne!(first_path, second_path);
    assert_eq!(
        app.clone()
            .oneshot(Request::get(&first_path).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let response = app
        .clone()
        .oneshot(Request::get(&second_path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let replacement_html = body_string(response).await;
    assert!(replacement_html.contains("Shared article · Shared with Rusty Feed"));
    assert!(!replacement_html.contains("A private reader shares safely"));
    assert!(!replacement_html.contains("Ada Reader"));
    assert!(replacement_html.contains("selected public-page copy is unavailable"));
    assert!(replacement_html.contains("https://tracker.example/hero.jpg"));

    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{article_id}/share/revoke"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        app.oneshot(Request::get(&second_path).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let audit_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public_article_share_events WHERE article_id = $1",
    )
    .bind(article_id)
    .fetch_one(state.db.pool())
    .await
    .unwrap();
    assert_eq!(audit_events, 3);
}

#[tokio::test]
async fn another_user_cannot_share_an_unsubscribed_article() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state.clone());
    let (_owner_sid, owner_email) = register(&app).await;
    let article_id = seed_article(&state, &owner_email).await;
    let (other_sid, _) = register(&app).await;
    let response = app
        .clone()
        .oneshot(
            Request::get("/settings/preferences")
                .header("cookie", format!("rusty_feed_sid={other_sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = cookie(&set_cookies(&response), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let response = app
        .oneshot(
            Request::post(format!("/articles/{article_id}/share"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={other_sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!(
                    "csrf_token={csrf}&slug=forbidden&content_source=rss&lifetime_days=7"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
