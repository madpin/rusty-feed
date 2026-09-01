//! Replay the request sequence used by Google Reader-compatible clients.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use rusty_feed_cache::Cache;
use rusty_feed_db::models::NewArticle;
use rusty_feed_db::{api_tokens, articles, feeds, subscriptions, users, Db};
use rusty_feed_web::{router, AppSettings, AppState};

async fn body(response: axum::response::Response) -> String {
    String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn client_sequence_preserves_shared_content_and_personal_state() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let state = AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    );
    let user = users::create(
        state.db.pool(),
        &format!("greader-{}@example.test", uuid::Uuid::now_v7()),
        Some("GReader Test"),
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!("https://greader.example/{}/feed.xml", uuid::Uuid::now_v7()),
        "GReader feed",
        Some("https://greader.example/"),
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
            url: format!("https://greader.example/{}/article", uuid::Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: "Protocol article".into(),
            author: Some("Rusty Feed".into()),
            content: "<p>Protocol body</p>".into(),
            excerpt: Some("Protocol body".into()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    let raw_token = format!("greader-token-{}", uuid::Uuid::now_v7());
    api_tokens::create(
        state.db.pool(),
        user.id,
        "GReader replay",
        &rusty_feed_web::auth::token::hash(&raw_token),
        &["greader".to_string()],
    )
    .await
    .unwrap();
    let app = router(state.clone());

    let login_form = format!(
        "Email={}&Passwd={}&output=json",
        user.email.replace('@', "%40"),
        raw_token
    );
    let response = app
        .clone()
        .oneshot(
            Request::post("/accounts/ClientLogin")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(login_form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let login: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
    assert_eq!(login["Auth"], raw_token);

    let auth = format!("GoogleLogin auth={raw_token}");
    let response = app
        .clone()
        .oneshot(
            Request::get("/reader/api/0/user-info")
                .header("authorization", &auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::get("/reader/api/0/subscription/list?output=json")
                .header("authorization", &auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let subscriptions: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
    let protocol_feed_id = subscriptions["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["title"] == "GReader feed")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let response = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/reader/api/0/stream/items/ids?output=json&s={protocol_feed_id}&n=10"
            ))
            .header("authorization", &auth)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let ids: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
    let item_id = ids["itemRefs"].as_array().unwrap().first().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let response = app
        .clone()
        .oneshot(
            Request::post("/reader/api/0/stream/items/contents")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("T={raw_token}&output=json&i={item_id}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let contents: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
    assert_eq!(contents["items"][0]["title"], "Protocol article");
    assert_eq!(
        contents["items"][0]["content"]["content"],
        "<p>Protocol body</p>"
    );

    let read_tag = format!("user/{}/state/com.google/read", user.id);
    let star_tag = format!("user/{}/state/com.google/starred", user.id);
    let response = app
        .clone()
        .oneshot(
            Request::post("/reader/api/0/edit-tag")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "T={raw_token}&i={item_id}&a={read_tag}&a={star_tag}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        rusty_feed_db::read_articles::is_read(state.db.pool(), user.id, article.id)
            .await
            .unwrap()
    );
    assert!(
        rusty_feed_db::bookmarks::is_bookmarked(state.db.pool(), user.id, article.id)
            .await
            .unwrap()
    );

    let response = app
        .oneshot(
            Request::get("/reader/api/0/token")
                .header("authorization", auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body(response).await, raw_token);
}
