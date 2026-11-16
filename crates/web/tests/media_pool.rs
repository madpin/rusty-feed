//! Media proxy cache misses must not pin pooled database connections while
//! the upstream download is in flight (FND-242). Tests skip when
//! `TEST_DATABASE_URL` is unavailable.

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tokio::task::JoinSet;
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rusty_feed_cache::Cache;
use rusty_feed_db::{
    articles, feeds, sessions, subscriptions, users, Db, DbPoolSettings, NewArticle,
    NewArticleEnclosure,
};
use rusty_feed_feeds::SafeFetchClient;
use rusty_feed_web::auth::token;
use rusty_feed_web::{router, AppSettings, AppState};

const UPSTREAM_DELAY: Duration = Duration::from_millis(1_500);

struct Harness {
    app: Router,
    db: Db,
    cookie: String,
    article_ids: Vec<Uuid>,
}

async fn harness(
    database_url: &str,
    publisher: &MockServer,
    paths: &[&str],
    pool_size: u32,
) -> Harness {
    let setup = Db::connect(database_url).await.unwrap();
    setup.migrate().await.unwrap();
    let user = users::create(
        setup.pool(),
        &format!("media-pool-{}@example.test", Uuid::now_v7()),
        None,
        None,
        "user",
        Some("hash"),
    )
    .await
    .unwrap();
    let generated = token::generate();
    sessions::create(
        setup.pool(),
        user.id,
        &generated.hash,
        time::OffsetDateTime::now_utc() + time::Duration::days(1),
    )
    .await
    .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        setup.pool(),
        &format!("https://media-pool.example/{}.xml", Uuid::now_v7()),
        "Media pool",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(setup.pool(), user.id, feed.id, None)
        .await
        .unwrap();
    let mut article_ids = Vec::new();
    for media_path in paths {
        let (article, _) = articles::upsert(
            setup.pool(),
            &NewArticle {
                feed_id: feed.id,
                url: format!("https://media-pool.example/article/{}", Uuid::now_v7()),
                comments_url: None,
                guid: Some(Uuid::now_v7().to_string()),
                title: "Episode".to_string(),
                author: None,
                content: "<p>Listen</p>".to_string(),
                excerpt: None,
                image_url: None,
                enclosures: vec![NewArticleEnclosure {
                    url: format!("{}{media_path}?run={}", publisher.uri(), Uuid::now_v7()),
                    media_kind: "audio".to_string(),
                    mime_type: Some("audio/mpeg".to_string()),
                    title: None,
                    size_bytes: Some(10),
                    duration_seconds: None,
                    width: None,
                    height: None,
                }],
                published_at: None,
            },
        )
        .await
        .unwrap();
        article_ids.push(article.id);
    }
    let db = Db::connect_with(
        database_url,
        &DbPoolSettings {
            max_connections: pool_size,
            acquire_timeout: Duration::from_secs(30),
        },
    )
    .await
    .unwrap();
    let state = AppState::new(
        db.clone(),
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    )
    .with_fetch(SafeFetchClient::new_allow_private(Duration::from_secs(20)).unwrap());
    Harness {
        app: router(state),
        db,
        cookie: format!("rusty_feed_sid={}", generated.raw),
        article_ids,
    }
}

async fn get_media(app: Router, cookie: String, article_id: Uuid) -> (StatusCode, Vec<u8>) {
    let response = app
        .oneshot(
            Request::get(format!("/articles/{article_id}/media/0"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, body.to_vec())
}

#[tokio::test]
async fn cold_media_fetches_leave_the_pool_available() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let publisher = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/mpeg")
                .set_body_bytes(b"0123456789")
                .set_delay(UPSTREAM_DELAY),
        )
        .mount(&publisher)
        .await;
    // More cold URLs than the 8-connection lock pool: the overflow must still
    // be fetched (without the cross-process lock) rather than queue or fail.
    let paths = [
        "/a.mp3", "/b.mp3", "/c.mp3", "/d.mp3", "/e.mp3", "/f.mp3", "/g.mp3", "/h.mp3", "/i.mp3",
        "/j.mp3", "/k.mp3", "/l.mp3",
    ];
    let harness = harness(&database_url, &publisher, &paths, 2).await;

    let mut requests = JoinSet::new();
    for article_id in &harness.article_ids {
        requests.spawn(get_media(
            harness.app.clone(),
            harness.cookie.clone(),
            *article_id,
        ));
    }

    // Wait until every upstream download is in flight, then probe the pool.
    let started = Instant::now();
    while publisher.received_requests().await.unwrap().len() < paths.len() {
        assert!(
            started.elapsed() < Duration::from_millis(1_000),
            "all {} cold fetches should reach the upstream while the pool has 2 connections \
             (saw {})",
            paths.len(),
            publisher.received_requests().await.unwrap().len()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut slowest = Duration::ZERO;
    for _ in 0..5 {
        let probe = Instant::now();
        tokio::time::timeout(Duration::from_millis(750), harness.db.ping())
            .await
            .expect("a trivial query must not wait for the upstream downloads")
            .unwrap();
        slowest = slowest.max(probe.elapsed());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        slowest < UPSTREAM_DELAY / 2,
        "trivial query took {slowest:?} while fetches were in flight"
    );

    while let Some(result) = requests.join_next().await {
        let (status, body) = result.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"0123456789");
    }
}

#[tokio::test]
async fn concurrent_misses_for_one_url_make_one_upstream_request() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let publisher = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/shared.mp3"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/mpeg")
                .set_body_bytes(b"0123456789")
                .set_delay(Duration::from_millis(400)),
        )
        .expect(1)
        .mount(&publisher)
        .await;
    let harness = harness(&database_url, &publisher, &["/shared.mp3"], 10).await;

    let mut requests = JoinSet::new();
    for _ in 0..6 {
        requests.spawn(get_media(
            harness.app.clone(),
            harness.cookie.clone(),
            harness.article_ids[0],
        ));
    }
    while let Some(result) = requests.join_next().await {
        let (status, body) = result.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"0123456789");
    }
    publisher.verify().await;
}
