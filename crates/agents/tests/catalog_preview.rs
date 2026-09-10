use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rusty_feed_agents::{Agent, AgentContext, CatalogPreviewAgent};
use rusty_feed_db::{articles, catalog, feeds, Db, NewArticle};
use rusty_feed_extract::{ExtractConfig, ExtractionService};
use rusty_feed_feeds::SafeFetchClient;

const FULL_PAGE: &str = r#"<html><body><article><h1>Full page</h1>
<p>This public article contains enough original prose to exceed the readability threshold and
provide a meaningful full-content preview. It intentionally has several sentences of detailed
context, explanations, and supporting information for the reader to inspect before subscribing.</p>
<p>The second paragraph makes the sample clearly larger than an RSS excerpt while retaining a
simple semantic article structure that the extractor can identify safely.</p>
<p>A final paragraph supplies enough substantive prose for a conservative public-body threshold,
covering the practical implications, relevant caveats, and additional background needed to make
this preview useful before a reader decides to subscribe to the source.</p></article></body></html>"#;

async fn test_db() -> Option<Db> {
    let database_url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&database_url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn insert_article(db: &Db, feed_id: uuid::Uuid, url: String, title: &str) -> uuid::Uuid {
    let (article, _) = articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id,
            url,
            comments_url: None,
            guid: Some(uuid::Uuid::now_v7().to_string()),
            title: title.to_string(),
            author: None,
            content: "Short RSS excerpt".to_string(),
            excerpt: Some("Short RSS excerpt".to_string()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    article.id
}

#[tokio::test]
async fn catalog_probe_samples_three_public_pages_without_changing_article_policy() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/unprobed"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/full"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(FULL_PAGE)
                .insert_header("content-type", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/paywall"))
        .respond_with(ResponseTemplate::new(402))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/restricted"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("{}/feed.xml?{}", server.uri(), uuid::Uuid::now_v7()),
        "Preview source",
        None,
    )
    .await
    .unwrap();
    catalog::upsert(
        db.pool(),
        feed.id,
        catalog::CatalogDraft {
            title: "Preview source",
            description: None,
            category: "Testing",
            country: "Global",
            language: "en",
            featured: false,
            sort_order: 0,
            published: true,
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE feeds SET respect_robots_txt = false WHERE id = $1")
        .bind(feed.id)
        .execute(db.pool())
        .await
        .unwrap();
    let unprobed = insert_article(
        &db,
        feed.id,
        format!("{}/unprobed", server.uri()),
        "Unprobed",
    )
    .await;
    let full = insert_article(&db, feed.id, format!("{}/full", server.uri()), "Full").await;
    let paywall =
        insert_article(&db, feed.id, format!("{}/paywall", server.uri()), "Paywall").await;
    let restricted = insert_article(
        &db,
        feed.id,
        format!("{}/restricted", server.uri()),
        "Restricted",
    )
    .await;
    for (article_id, offset) in [(unprobed, 4), (full, 3), (paywall, 2), (restricted, 1)] {
        sqlx::query(
            "UPDATE articles SET published_at = now() - ($2 * interval '1 day') WHERE id = $1",
        )
        .bind(article_id)
        .bind(offset)
        .execute(db.pool())
        .await
        .unwrap();
    }

    let agent = CatalogPreviewAgent::new(Arc::new(ExtractionService::new(ExtractConfig {
        base_delay: Duration::from_millis(0),
        ..ExtractConfig::default()
    })));
    let context = AgentContext::new(
        db.clone(),
        SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap(),
    );
    let output = agent
        .run(&context, json!({ "feed_id": feed.id }))
        .await
        .unwrap();
    assert_eq!(output["sampled"], 3);
    assert_eq!(output["full_content"], 1, "{output}");
    assert_eq!(output["http_402"], 1);

    let stats = catalog::content_probe_stats(db.pool(), feed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stats.sampled_article_count, 3);
    assert_eq!(stats.full_content_count, 1);
    assert!(stats
        .average_processed_body_chars
        .is_some_and(|size| size > 0));
    assert_eq!(stats.http_402_count, 1);
    assert_eq!(stats.http_403_count, 1);
    let requests = server.received_requests().await.unwrap();
    assert!(requests
        .iter()
        .any(|request| request.url.path() == "/robots.txt"));
    assert!(requests
        .iter()
        .all(|request| request.url.path() != "/unprobed"));
    assert!(requests.iter().all(|request| {
        !request.headers.contains_key("cookie") && !request.headers.contains_key("authorization")
    }));
    let previews = catalog::preview_articles(db.pool(), feed.id, 3)
        .await
        .unwrap();
    assert_eq!(previews.len(), 3);
    assert!(previews
        .iter()
        .any(|preview| preview.feed_content.as_deref() == Some("Short RSS excerpt")));
    assert!(articles::list_for_feed(db.pool(), feed.id, 3)
        .await
        .unwrap()
        .iter()
        .all(|article| article.content_source == "feed" && article.extraction_status == "none"));
    assert!(
        !feeds::find_by_id(db.pool(), feed.id)
            .await
            .unwrap()
            .unwrap()
            .respect_robots_txt
    );
}

#[tokio::test]
async fn catalog_probe_obeys_robots_when_the_feed_opted_out() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("User-agent: *\nDisallow: /blocked"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/blocked"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FULL_PAGE))
        .mount(&server)
        .await;
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("{}/feed.xml?{}", server.uri(), uuid::Uuid::now_v7()),
        "Robots source",
        None,
    )
    .await
    .unwrap();
    catalog::upsert(
        db.pool(),
        feed.id,
        catalog::CatalogDraft {
            title: "Robots source",
            description: None,
            category: "Testing",
            country: "Global",
            language: "en",
            featured: false,
            sort_order: 0,
            published: true,
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE feeds SET respect_robots_txt = false WHERE id = $1")
        .bind(feed.id)
        .execute(db.pool())
        .await
        .unwrap();
    insert_article(&db, feed.id, format!("{}/blocked", server.uri()), "Blocked").await;

    CatalogPreviewAgent::new(Arc::new(ExtractionService::new(ExtractConfig {
        base_delay: Duration::from_millis(0),
        ..ExtractConfig::default()
    })))
    .run(
        &AgentContext::new(
            db.clone(),
            SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap(),
        ),
        json!({ "feed_id": feed.id }),
    )
    .await
    .unwrap();

    let stats = catalog::content_probe_stats(db.pool(), feed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stats.sampled_article_count, 1);
    assert_eq!(stats.robots_disallowed_count, 1);
    let requests = server.received_requests().await.unwrap();
    assert!(requests
        .iter()
        .any(|request| request.url.path() == "/robots.txt"));
    assert!(requests
        .iter()
        .all(|request| request.url.path() != "/blocked"));
}

#[tokio::test]
async fn catalog_probe_does_not_follow_redirects() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/redirect"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/target", server.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/target"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FULL_PAGE))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/unavailable"))
        .respond_with(ResponseTemplate::new(451))
        .mount(&server)
        .await;
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("{}/feed.xml?{}", server.uri(), uuid::Uuid::now_v7()),
        "Redirect source",
        None,
    )
    .await
    .unwrap();
    catalog::upsert(
        db.pool(),
        feed.id,
        catalog::CatalogDraft {
            title: "Redirect source",
            description: None,
            category: "Testing",
            country: "Global",
            language: "en",
            featured: false,
            sort_order: 0,
            published: true,
        },
    )
    .await
    .unwrap();
    insert_article(
        &db,
        feed.id,
        format!("{}/redirect", server.uri()),
        "Redirect",
    )
    .await;
    insert_article(
        &db,
        feed.id,
        format!("{}/unavailable", server.uri()),
        "Unavailable",
    )
    .await;

    CatalogPreviewAgent::new(Arc::new(ExtractionService::new(ExtractConfig {
        base_delay: Duration::from_millis(0),
        ..ExtractConfig::default()
    })))
    .run(
        &AgentContext::new(
            db.clone(),
            SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap(),
        ),
        json!({ "feed_id": feed.id }),
    )
    .await
    .unwrap();

    let stats = catalog::content_probe_stats(db.pool(), feed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stats.sampled_article_count, 2);
    assert_eq!(stats.fetch_failed_count, 1);
    assert_eq!(stats.http_451_count, 1);
    assert!(server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|request| request.url.path() != "/target"));
}

#[tokio::test]
async fn catalog_probe_persists_rate_limits_and_stops_sampling() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rate"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "60"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/later"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FULL_PAGE))
        .mount(&server)
        .await;
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("{}/feed.xml?{}", server.uri(), uuid::Uuid::now_v7()),
        "Rate source",
        None,
    )
    .await
    .unwrap();
    catalog::upsert(
        db.pool(),
        feed.id,
        catalog::CatalogDraft {
            title: "Rate source",
            description: None,
            category: "Testing",
            country: "Global",
            language: "en",
            featured: false,
            sort_order: 0,
            published: true,
        },
    )
    .await
    .unwrap();
    let later = insert_article(&db, feed.id, format!("{}/later", server.uri()), "Later").await;
    let rate = insert_article(&db, feed.id, format!("{}/rate", server.uri()), "Rate").await;
    sqlx::query("UPDATE articles SET published_at = now() - interval '1 day' WHERE id = $1")
        .bind(later)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE articles SET published_at = now() WHERE id = $1")
        .bind(rate)
        .execute(db.pool())
        .await
        .unwrap();

    CatalogPreviewAgent::new(Arc::new(ExtractionService::new(ExtractConfig {
        base_delay: Duration::from_millis(0),
        ..ExtractConfig::default()
    })))
    .run(
        &AgentContext::new(
            db.clone(),
            SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap(),
        ),
        json!({ "feed_id": feed.id }),
    )
    .await
    .unwrap();

    let stats = catalog::content_probe_stats(db.pool(), feed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stats.sampled_article_count, 1);
    assert_eq!(stats.rate_limited_count, 1);
    assert!(!catalog::probe_needs_refresh(db.pool(), feed.id)
        .await
        .unwrap());
    let requests = server.received_requests().await.unwrap();
    assert!(requests.iter().any(|request| request.url.path() == "/rate"));
    assert!(requests
        .iter()
        .all(|request| request.url.path() != "/later"));
}

#[tokio::test]
async fn catalog_probe_does_not_fetch_when_robots_is_unavailable() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FULL_PAGE))
        .mount(&server)
        .await;
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("{}/feed.xml?{}", server.uri(), uuid::Uuid::now_v7()),
        "Unavailable robots source",
        None,
    )
    .await
    .unwrap();
    catalog::upsert(
        db.pool(),
        feed.id,
        catalog::CatalogDraft {
            title: "Unavailable robots source",
            description: None,
            category: "Testing",
            country: "Global",
            language: "en",
            featured: false,
            sort_order: 0,
            published: true,
        },
    )
    .await
    .unwrap();
    insert_article(
        &db,
        feed.id,
        format!("{}/article", server.uri()),
        "Unavailable robots",
    )
    .await;

    CatalogPreviewAgent::new(Arc::new(ExtractionService::new(ExtractConfig {
        base_delay: Duration::from_millis(0),
        ..ExtractConfig::default()
    })))
    .run(
        &AgentContext::new(
            db.clone(),
            SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap(),
        ),
        json!({ "feed_id": feed.id }),
    )
    .await
    .unwrap();

    let stats = catalog::content_probe_stats(db.pool(), feed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stats.sampled_article_count, 1);
    assert_eq!(stats.fetch_failed_count, 1);
    let requests = server.received_requests().await.unwrap();
    assert!(requests
        .iter()
        .any(|request| request.url.path() == "/robots.txt"));
    assert!(requests
        .iter()
        .all(|request| request.url.path() != "/article"));
}
