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
