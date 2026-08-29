//! M6 integration tests for fetch/refresh, using a mocked feed server.
//! The refresh test requires `TEST_DATABASE_URL`; fetch/SSRF tests do not.

use std::time::Duration;

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rustss_feeds::{fetch_feed, refresh_feed, SafeFetchClient};

/// Build an RSS document whose item URLs are unique per `run` (article URLs are
/// globally unique, so a stable URL would be "skipped" as a duplicate on reruns).
fn rss(run: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <title>Mock News</title>
  <link>https://mock.example</link>
  <item><title>One</title><link>https://mock.example/{run}/1</link><guid>{run}-1</guid>
    <description>First</description></item>
  <item><title>Two</title><link>https://mock.example/{run}/2</link><guid>{run}-2</guid>
    <description>Second</description></item>
</channel></rss>"#
    )
}

fn rss_with_comments(run: &str, comments_url: &str) -> String {
    rss(run).replace(
        "<description>First</description>",
        &format!("<comments>{comments_url}</comments><description>First</description>"),
    )
}

fn rss_with_icon(run: &str, icon_url: &str) -> String {
    rss(run).replace(
        "<item><title>One</title>",
        &format!(
            "<image><url>{icon_url}</url><title>Mock News</title><link>https://mock.example</link></image>\n  <item><title>One</title>"
        ),
    )
}

#[tokio::test]
async fn fetch_returns_body_and_validators() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"v1\"")
                .set_body_string(rss("fetch")),
        )
        .mount(&server)
        .await;

    let client = SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap();
    let out = fetch_feed(&client, &format!("{}/feed.xml", server.uri()), None, None)
        .await
        .unwrap();
    assert!(!out.not_modified);
    assert_eq!(out.etag.as_deref(), Some("\"v1\""));
    assert!(!out.body.is_empty());
}

#[tokio::test]
async fn conditional_get_returns_not_modified() {
    let server = MockServer::start().await;
    // Only respond when the client sends the matching validator.
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;

    let client = SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap();
    let out = fetch_feed(
        &client,
        &format!("{}/feed.xml", server.uri()),
        Some("\"v1\""),
        None,
    )
    .await
    .unwrap();
    assert!(out.not_modified);
}

#[tokio::test]
async fn ssrf_blocks_loopback_literal() {
    let client = SafeFetchClient::new(Duration::from_secs(5)).unwrap();
    // Blocked before any connection (IP-literal validation).
    assert!(fetch_feed(&client, "http://127.0.0.1:9/feed", None, None)
        .await
        .is_err());
}

#[tokio::test]
async fn refresh_ingests_articles() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = rustss_db::Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();

    // Unique item URLs per run so ingestion isn't "skipped" as duplicates.
    let run = uuid::Uuid::now_v7().to_string();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&run)))
        .mount(&server)
        .await;

    // A unique feed row so the test is idempotent against a shared DB.
    let feed_url = format!("{}/feed.xml?run={}", server.uri(), run);
    let (feed, _) = rustss_db::feeds::upsert_by_url(db.pool(), &feed_url, "Mock News", None)
        .await
        .unwrap();

    let client = SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap();
    let stats = refresh_feed(&db, &client, &feed).await.unwrap();
    assert_eq!(stats.created, 2, "two items should be ingested");

    // Re-refreshing the same content is idempotent (no new rows).
    let feed = rustss_db::feeds::find_by_id(db.pool(), feed.id)
        .await
        .unwrap()
        .unwrap();
    let stats2 = refresh_feed(&db, &client, &feed).await.unwrap();
    assert_eq!(stats2.created, 0);
    assert!(stats2.skipped >= 2 || stats2.not_modified);

    let arts = rustss_db::articles::list_for_feed(db.pool(), feed.id, 10)
        .await
        .unwrap();
    assert_eq!(arts.len(), 2);
}

#[tokio::test]
async fn refresh_persists_a_distinct_rss_comments_link() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = rustss_db::Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();
    let run = uuid::Uuid::now_v7().to_string();
    let server = MockServer::start().await;
    let comments_url = "https://news.ycombinator.com/item?id=123";
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(rss_with_comments(&run, comments_url)),
        )
        .mount(&server)
        .await;
    let (feed, _) = rustss_db::feeds::upsert_by_url(
        db.pool(),
        &format!("{}/feed.xml?run={run}", server.uri()),
        "Mock News",
        None,
    )
    .await
    .unwrap();
    let client = SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap();
    refresh_feed(&db, &client, &feed).await.unwrap();
    let articles = rustss_db::articles::list_for_feed(db.pool(), feed.id, 10)
        .await
        .unwrap();
    assert_eq!(
        articles
            .iter()
            .find_map(|article| article.comments_url.as_deref()),
        Some(comments_url)
    );
}

#[tokio::test]
async fn refresh_records_a_safe_parse_failure() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = rustss_db::Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not a feed"))
        .mount(&server)
        .await;
    let token = "RAW_QUERY_TOKEN_SENTINEL";
    let (feed, _) = rustss_db::feeds::upsert_by_url(
        db.pool(),
        &format!("{}/feed.xml?token={token}", server.uri()),
        "Broken feed",
        None,
    )
    .await
    .unwrap();
    let client = SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap();
    assert!(refresh_feed(&db, &client, &feed).await.is_err());
    let stored = rustss_db::feeds::find_by_id(db.pool(), feed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.health_status, "warning");
    assert_eq!(
        stored.last_error.as_deref(),
        Some("Feed response could not be parsed")
    );
    let event = rustss_db::feeds::list_recent_errors(db.pool(), 100)
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.feed_id == feed.id)
        .unwrap();
    assert_eq!(event.error_type, "parse");
}

#[tokio::test]
async fn refresh_downloads_a_declared_feed_icon() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = rustss_db::Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();
    let run = uuid::Uuid::now_v7().to_string();
    let server = MockServer::start().await;
    let icon = vec![
        0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 1, 2, 3,
    ];
    Mock::given(method("GET"))
        .and(path("/icon.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(icon.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(rss_with_icon(&run, &format!("{}/icon.png", server.uri()))),
        )
        .mount(&server)
        .await;

    let (feed, _) = rustss_db::feeds::upsert_by_url(
        db.pool(),
        &format!("{}/feed.xml?run={run}", server.uri()),
        "Mock News",
        None,
    )
    .await
    .unwrap();
    let client = SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap();
    refresh_feed(&db, &client, &feed).await.unwrap();

    let stored = rustss_db::feed_icons::get(db.pool(), feed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.content_type, "image/png");
    assert_eq!(stored.content, icon);
    assert_eq!(
        rustss_db::feeds::find_by_id(db.pool(), feed.id)
            .await
            .unwrap()
            .unwrap()
            .image_url,
        Some(format!("{}/icon.png", server.uri()))
    );
}

#[tokio::test]
async fn feeds_missing_an_icon_attempt_are_queued_for_backfill() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = rustss_db::Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();
    let user = rustss_db::users::create(
        db.pool(),
        &format!("icon-backfill-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (feed, _) = rustss_db::feeds::upsert_by_url(
        db.pool(),
        &format!("https://icon-backfill.example/{}.xml", uuid::Uuid::now_v7()),
        "Icon backfill",
        None,
    )
    .await
    .unwrap();
    rustss_db::subscriptions::subscribe(db.pool(), user.id, feed.id, None)
        .await
        .unwrap();
    rustss_db::feeds::record_fetch_success(db.pool(), feed.id, None, None)
        .await
        .unwrap();
    assert!(rustss_db::feeds::list_due(db.pool(), 10_000)
        .await
        .unwrap()
        .iter()
        .any(|candidate| candidate.id == feed.id));
    rustss_db::feed_icons::record_failure(
        db.pool(),
        feed.id,
        "https://icon-backfill.example/favicon.ico",
        "not found",
    )
    .await
    .unwrap();
    assert!(!rustss_db::feeds::list_due(db.pool(), 10_000)
        .await
        .unwrap()
        .iter()
        .any(|candidate| candidate.id == feed.id));
}
