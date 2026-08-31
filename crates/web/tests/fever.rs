use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

use rustss_cache::Cache;
use rustss_db::{api_tokens, articles, feeds, subscriptions, users, Db, NewArticle};
use rustss_web::{router, AppSettings, AppState};

#[tokio::test]
async fn fever_replays_groups_items_read_and_saved_state() {
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
        &format!("fever-{}@example.test", Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!("https://fever.example/{}/feed", Uuid::now_v7()),
        "Fever feed",
        Some("https://fever.example"),
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
            url: format!("https://fever.example/{}/article", Uuid::now_v7()),
            comments_url: None,
            guid: Some(Uuid::now_v7().to_string()),
            title: "Fever item".into(),
            author: Some("Author".into()),
            content: "<p>Body</p>".into(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    let api_key = format!(
        "{:x}",
        md5::compute(format!("reader-{}:long-password-123", Uuid::now_v7()))
    );
    api_tokens::create(
        state.db.pool(),
        user.id,
        "Fever test",
        &rustss_web::auth::token::hash(&api_key),
        &["fever".to_string()],
    )
    .await
    .unwrap();
    let item_id: i64 =
        sqlx::query_scalar("SELECT greader_id FROM greader_article_ids WHERE article_id = $1")
            .bind(article.id)
            .fetch_one(state.db.pool())
            .await
            .unwrap();
    let app = router(state.clone());

    let response = app
        .clone()
        .oneshot(
            Request::post("/fever/")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "api_key={api_key}&groups=&feeds=&items=&unread_item_ids="
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["auth"], 1);
    assert!(body["feeds"]
        .as_array()
        .unwrap()
        .iter()
        .any(|feed| feed["title"] == "Fever feed"));
    assert!(body["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == item_id));
    assert!(body["unread_item_ids"]
        .as_str()
        .unwrap()
        .contains(&item_id.to_string()));

    for action in ["read", "saved"] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/fever/")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "api_key={api_key}&mark=item&as={action}&id={item_id}"
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
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
}
