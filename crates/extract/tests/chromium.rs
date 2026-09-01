use std::time::Duration;

use rusty_feed_extract::{
    chromium::ChromiumConfig, ExtractConfig, ExtractOutcome, ExtractionService,
};
use rusty_feed_feeds::SafeFetchClient;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn chromium_renders_inline_application_content_without_publisher_network_access() {
    let (Ok(database_url), Ok(cdp_url)) = (
        std::env::var("TEST_DATABASE_URL"),
        std::env::var("CHROMIUM_CDP_URL"),
    ) else {
        eprintln!("skipping: TEST_DATABASE_URL or CHROMIUM_CDP_URL not set");
        return;
    };
    let db = rusty_feed_db::Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/story"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/html").set_body_string(
            r#"<!doctype html><html><body><main id="app"></main><script>
            document.getElementById('app').innerHTML = '<article><h1>Rendered story</h1><p>' +
              'A browser-rendered paragraph with useful article detail. '.repeat(12) + '</p></article>';
            </script></body></html>"#,
        ))
        .mount(&server)
        .await;
    let service = ExtractionService::new(ExtractConfig {
        base_delay: Duration::ZERO,
        chromium: Some(ChromiumConfig {
            endpoint: cdp_url,
            max_concurrency: 1,
            render_wait: Duration::from_millis(100),
            timeout: Duration::from_secs(10),
        }),
        ..ExtractConfig::default()
    });
    let fetch = SafeFetchClient::new_allow_private(Duration::from_secs(5)).unwrap();
    let outcome = service
        .extract(
            &db,
            &fetch,
            &format!("{}/story", server.uri()),
            None,
            false,
            Some(0),
        )
        .await;
    let ExtractOutcome::Extracted(extracted) = outcome else {
        panic!("Chromium fallback did not produce content");
    };
    assert_eq!(extracted.extractor, "chromium");
    assert!(extracted.content.contains("browser-rendered paragraph"));
    assert!(!extracted.content.contains("<script"));
}

#[tokio::test]
async fn unavailable_chromium_falls_back_to_local_content() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = rusty_feed_db::Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/fallback"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/html").set_body_string(
            "<!doctype html><html><body><p>Local fallback remains readable.</p></body></html>",
        ))
        .mount(&server)
        .await;
    let service = ExtractionService::new(ExtractConfig {
        base_delay: Duration::ZERO,
        chromium: Some(ChromiumConfig {
            endpoint: "http://127.0.0.1:1".to_string(),
            max_concurrency: 1,
            render_wait: Duration::ZERO,
            timeout: Duration::from_millis(200),
        }),
        ..ExtractConfig::default()
    });
    let fetch = SafeFetchClient::new_allow_private(Duration::from_secs(5)).unwrap();
    let outcome = service
        .extract(
            &db,
            &fetch,
            &format!("{}/fallback", server.uri()),
            None,
            false,
            Some(0),
        )
        .await;
    let ExtractOutcome::Extracted(extracted) = outcome else {
        panic!("local extraction should survive an unavailable Chromium service");
    };
    assert!(extracted
        .content
        .contains("Local fallback remains readable"));
    assert_ne!(extracted.extractor, "chromium");
}
