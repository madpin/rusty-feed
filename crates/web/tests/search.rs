//! M9 web test: the /search page returns semantically-matched articles.
//! Requires `TEST_DATABASE_URL`; skipped otherwise.

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

use rustss_cache::Cache;
use rustss_db::models::NewArticle;
use rustss_db::{articles, embeddings, feeds, subscriptions, users, Db};
use rustss_embed::{Embedder, HashEmbedder};
use rustss_web::{router, AppSettings, AppState};

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

async fn register(app: &Router) -> (String, String) {
    let email = format!("s-{}%40example.com", uuid::Uuid::now_v7());
    let resp = app
        .clone()
        .oneshot(Request::get("/register").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let csrf = find(&set_cookies(&resp), "rustss_csrf")
        .unwrap()
        .to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "name=&email={email}&password=password123&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let sid = find(&set_cookies(&resp), "rustss_sid").unwrap().to_string();
    (sid, email.replace("%40", "@"))
}

#[tokio::test]
async fn search_returns_matching_article() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();
    let state = AppState::new(
        db.clone(),
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    );
    let app = router(state);
    let (sid, email) = register(&app).await;

    // Seed a subscribed, embedded article.
    let user = users::find_by_email(db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://ex.com/{}.xml", uuid::Uuid::now_v7()),
        "F",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(db.pool(), user.id, feed.id, None)
        .await
        .unwrap();
    let (article, _) = articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://ex.com/{}", uuid::Uuid::now_v7()),
            guid: None,
            title: "Async Rust Concurrency".into(),
            author: None,
            content: "a deep dive into async await and concurrency in the rust language".into(),
            excerpt: Some("async rust".into()),
            image_url: None,
            published_at: None,
        },
    )
    .await
    .unwrap();
    let embedder = HashEmbedder::default();
    let v = embedder
        .embed_one("a deep dive into async await and concurrency in the rust language")
        .await
        .unwrap();
    embeddings::upsert(
        db.pool(),
        article.id,
        &v,
        embedder.model(),
        article.content_revision,
    )
    .await
    .unwrap();

    // Search finds it (the default AppState embedder is the same HashEmbedder).
    let resp = app
        .oneshot(
            Request::get("/search?q=async%20rust%20concurrency")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("Async Rust Concurrency"),
        "expected the seeded article"
    );
    assert!(body.contains("% match"));
}
