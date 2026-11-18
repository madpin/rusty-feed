//! M8 integration tests: content extraction agent + wiring. Require
//! `TEST_DATABASE_URL`; skipped otherwise.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rusty_feed_agents::{Agent, AgentContext, ExtractContentAgent, FetchFeedAgent};
use rusty_feed_core::feed_content::{
    ContentLifetime, SubscriptionContentMode, SubscriptionContentSettings,
};
use rusty_feed_db::{articles, domain_state, feeds, jobs, subscriptions, users, Db};
use rusty_feed_extract::{ExtractConfig, ExtractOutcome, ExtractionService};
use rusty_feed_feeds::SafeFetchClient;

const ARTICLE_HTML: &str = r#"<html><head><title>Full Article</title></head><body>
  <nav><a href="/">Home</a><script>bad()</script></nav>
  <article>
    <h1>The Headline</h1>
    <p>This is the complete article body that a feed only summarized. It is intentionally
       long so it comfortably exceeds the extractor's minimum content threshold and is chosen
       as the main content rather than the navigation or footer boilerplate around it.</p>
    <p>A second paragraph continues the story with more than enough additional prose to keep
       the extracted text well above threshold, plus a <a href="https://x.ex">link</a>.</p>
    <figure class="wp-block-embed is-type-video is-provider-youtube"><iframe title="You Can See Everything | Official Teaser HD | A24" src="https://www.youtube.com/embed/GGJSRFWALTI?feature=oembed"></iframe></figure>
  </article>
  <footer>copyright</footer>
</body></html>"#;

const RSS: &str = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>F</title><link>https://m.ex</link>
  <item><title>Item</title><link>PLACEHOLDER</link><guid>PLACEHOLDER</guid><description>summary only</description></item>
</channel></rss>"#;

/// Every test here talks to a wiremock server on `127.0.0.1`, so they all share
/// one `domain_extraction_state` row. A concurrent successful extraction resets
/// `rate_limit_count` to 0, racing the 429 assertion (FND-253); run them one at
/// a time.
static DOMAIN_STATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn connect() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn ctx(db: Db) -> AgentContext {
    AgentContext::new(
        db,
        SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap(),
    )
}

/// A fast extraction agent (no politeness delay) for tests.
fn fast_extract_agent() -> ExtractContentAgent {
    ExtractContentAgent::new(Arc::new(ExtractionService::new(ExtractConfig {
        base_delay: Duration::from_millis(0),
        ..ExtractConfig::default()
    })))
}

async fn request_article_pages(db: &Db, feed_id: uuid::Uuid) {
    let user = users::create(
        db.pool(),
        &format!("extract-{}@example.com", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(db.pool(), user.id, feed_id, None)
        .await
        .unwrap();
    subscriptions::update_content_settings(
        db.pool(),
        user.id,
        feed_id,
        SubscriptionContentSettings {
            mode: SubscriptionContentMode::ArticlePage,
            lifetime: ContentLifetime::Days(30),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn extract_content_agent_enriches_article() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = DOMAIN_STATE_LOCK.lock().await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
        .mount(&server)
        .await;

    // A feed + one article whose RSS body was just a summary.
    let feed_url = format!("{}/feed.xml?{}", server.uri(), uuid::Uuid::now_v7());
    let (feed, _) = feeds::upsert_by_url(db.pool(), &feed_url, "F", None)
        .await
        .unwrap();
    request_article_pages(&db, feed.id).await;
    sqlx::query("UPDATE feeds SET extraction_settings = '{\"mode\": \"article_page\"}'::jsonb WHERE id = $1")
        .bind(feed.id)
        .execute(db.pool())
        .await
        .unwrap();
    let art_url = format!("{}/article?{}", server.uri(), uuid::Uuid::now_v7());
    let (article, _) = articles::upsert(
        db.pool(),
        &rusty_feed_db::models::NewArticle {
            feed_id: feed.id,
            url: art_url,
            comments_url: None,
            guid: None,
            title: "Item".into(),
            author: None,
            content: "summary only".into(),
            excerpt: Some("summary only".into()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();

    let out = fast_extract_agent()
        .run(&ctx(db.clone()), json!({ "article_id": article.id }))
        .await
        .unwrap();
    assert_eq!(out["status"], "success");

    let stored = articles::find_by_id(db.pool(), article.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.extraction_status, "success");
    assert!(stored.content.contains("complete article body"));
    assert!(stored
        .content
        .contains("https://www.youtube.com/watch?v=GGJSRFWALTI"));
    assert!(!stored.content.contains("<iframe"));
    assert!(!stored.content.contains("bad()"), "must be sanitized");

    // Idempotent: a second run is a no-op (already processed).
    let out2 = fast_extract_agent()
        .run(&ctx(db.clone()), json!({ "article_id": article.id }))
        .await
        .unwrap();
    assert_eq!(out2["skipped"], "public-page content is current");
}

#[tokio::test]
async fn on_demand_extraction_runs_without_changing_subscription_demand() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = DOMAIN_STATE_LOCK.lock().await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
        .mount(&server)
        .await;
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("{}/feed.xml?{}", server.uri(), uuid::Uuid::now_v7()),
        "On-demand",
        None,
    )
    .await
    .unwrap();
    let user = users::create(
        db.pool(),
        &format!("on-demand-{}@example.com", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(db.pool(), user.id, feed.id, None)
        .await
        .unwrap();
    let (article, _) = articles::upsert(
        db.pool(),
        &rusty_feed_db::models::NewArticle {
            feed_id: feed.id,
            url: format!("{}/article?{}", server.uri(), uuid::Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: "Temporary page view".into(),
            author: None,
            content: "summary only".into(),
            excerpt: Some("summary only".into()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();

    let out = fast_extract_agent()
        .run(
            &ctx(db.clone()),
            json!({
                "article_id": article.id,
                "content_revision": article.content_revision,
                "on_demand_lifetime_days": 1,
            }),
        )
        .await
        .unwrap();
    assert_eq!(out["status"], "success");
    assert!(
        subscriptions::requested_article_page_lifetime(db.pool(), feed.id)
            .await
            .unwrap()
            .is_none()
    );
    let stored = articles::find_by_id(db.pool(), article.id)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.article_page_content.is_some());
    assert!(stored.article_page_expires_at.is_some());
}

#[tokio::test]
async fn rate_limited_extraction_records_and_signals_retry() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = DOMAIN_STATE_LOCK.lock().await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "30"))
        .mount(&server)
        .await;

    let feed_url = format!("{}/feed.xml?{}", server.uri(), uuid::Uuid::now_v7());
    let (feed, _) = feeds::upsert_by_url(db.pool(), &feed_url, "F", None)
        .await
        .unwrap();
    request_article_pages(&db, feed.id).await;
    sqlx::query("UPDATE feeds SET extraction_settings = '{\"mode\": \"article_page\"}'::jsonb WHERE id = $1")
        .bind(feed.id)
        .execute(db.pool())
        .await
        .unwrap();
    let art_url = format!("{}/article?{}", server.uri(), uuid::Uuid::now_v7());
    let (article, _) = articles::upsert(
        db.pool(),
        &rusty_feed_db::models::NewArticle {
            feed_id: feed.id,
            url: art_url.clone(),
            comments_url: None,
            guid: None,
            title: "Item".into(),
            author: None,
            content: "s".into(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();

    // The agent should error (so the job retries with backoff).
    let res = fast_extract_agent()
        .run(&ctx(db.clone()), json!({ "article_id": article.id }))
        .await;
    assert!(res.is_err(), "429 should signal a retry");

    let domain = url::Url::parse(&art_url)
        .unwrap()
        .host_str()
        .unwrap()
        .to_string();
    let state = domain_state::get(db.pool(), &domain)
        .await
        .unwrap()
        .unwrap();
    assert!(
        state.rate_limit_count >= 1,
        "429 should be recorded per-domain"
    );
}

#[tokio::test]
async fn fetch_feed_enqueues_extraction_when_enabled() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = DOMAIN_STATE_LOCK.lock().await;
    let run = uuid::Uuid::now_v7().to_string();
    let item_url = format!("https://item.ex/{run}");
    let rss = RSS
        .replacen("PLACEHOLDER", &item_url, 1)
        .replacen("PLACEHOLDER", &run, 1);

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss))
        .mount(&server)
        .await;

    // Feed with extraction enabled.
    let feed_url = format!("{}/feed.xml?{run}", server.uri());
    let (feed, _) = feeds::upsert_by_url(db.pool(), &feed_url, "F", None)
        .await
        .unwrap();
    request_article_pages(&db, feed.id).await;
    sqlx::query(
        "UPDATE feeds SET extraction_settings = '{\"enabled\": true}'::jsonb WHERE id = $1",
    )
    .bind(feed.id)
    .execute(db.pool())
    .await
    .unwrap();
    let feed = feeds::find_by_id(db.pool(), feed.id)
        .await
        .unwrap()
        .unwrap();

    let stats = FetchFeedAgent
        .run(&ctx(db.clone()), json!({ "feed_id": feed.id }))
        .await
        .unwrap();
    assert_eq!(stats["created"], 1);
    assert_eq!(
        stats["extract_jobs"], 1,
        "one extract job for the new article"
    );

    // The enqueued extract job is deduped per article.
    let art = &articles::list_for_feed(db.pool(), feed.id, 5)
        .await
        .unwrap()[0];
    let dedup = format!("extract:{}:{}", art.id, art.content_revision);
    assert_eq!(
        jobs::count_active_by_dedup(db.pool(), &dedup)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn ignored_robots_policy_skips_the_robots_request() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = DOMAIN_STATE_LOCK.lock().await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("User-agent: *\nDisallow: /article"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
        .mount(&server)
        .await;

    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("{}/feed.xml?{}", server.uri(), uuid::Uuid::now_v7()),
        "F",
        None,
    )
    .await
    .unwrap();
    request_article_pages(&db, feed.id).await;
    sqlx::query(
        "UPDATE feeds SET extraction_settings = '{\"mode\": \"article_page\"}'::jsonb, \
         respect_robots_txt = false WHERE id = $1",
    )
    .bind(feed.id)
    .execute(db.pool())
    .await
    .unwrap();
    let (article, _) = articles::upsert(
        db.pool(),
        &rusty_feed_db::models::NewArticle {
            feed_id: feed.id,
            url: format!("{}/article?{}", server.uri(), uuid::Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: "Item".into(),
            author: None,
            content: "RSS fallback".into(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    let output = fast_extract_agent()
        .run(&ctx(db.clone()), json!({ "article_id": article.id }))
        .await
        .unwrap();
    assert_eq!(output["status"], "success");
    assert!(server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|request| request.url.path() != "/robots.txt"));
}

#[tokio::test]
async fn feeds_without_subscriber_demand_do_not_fetch_article_pages() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = DOMAIN_STATE_LOCK.lock().await;
    let server = MockServer::start().await;
    for mode in ["rss", "paywalled"] {
        let (feed, _) = feeds::upsert_by_url(
            db.pool(),
            &format!("{}/feed-{mode}.xml", server.uri()),
            "F",
            None,
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE feeds SET extraction_settings = jsonb_build_object('mode', $2) WHERE id = $1",
        )
        .bind(feed.id)
        .bind(mode)
        .execute(db.pool())
        .await
        .unwrap();
        let (article, _) = articles::upsert(
            db.pool(),
            &rusty_feed_db::models::NewArticle {
                feed_id: feed.id,
                url: format!("{}/article-{mode}", server.uri()),
                comments_url: None,
                guid: None,
                title: "Item".into(),
                author: None,
                content: "RSS fallback".into(),
                excerpt: None,
                image_url: None,
                enclosures: Vec::new(),
                published_at: None,
            },
        )
        .await
        .unwrap();
        let out = fast_extract_agent()
            .run(&ctx(db.clone()), json!({ "article_id": article.id }))
            .await
            .unwrap();
        assert_eq!(out["skipped"], "no subscriber requests public-page content");
        if mode == "paywalled" {
            sqlx::query(
                "UPDATE feeds SET extraction_settings = '{\"mode\": \"rss\"}'::jsonb WHERE id = $1",
            )
            .bind(feed.id)
            .execute(db.pool())
            .await
            .unwrap();
        }
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn extraction_is_ssrf_guarded() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = DOMAIN_STATE_LOCK.lock().await;
    // A STRICT client must refuse a loopback article URL.
    let strict = SafeFetchClient::new(Duration::from_secs(5)).unwrap();
    let service = ExtractionService::new(ExtractConfig {
        base_delay: Duration::from_millis(0),
        ..ExtractConfig::default()
    });
    let outcome = service
        .extract(
            &db,
            &strict,
            "http://127.0.0.1:9/article",
            None,
            false,
            None,
        )
        .await;
    assert!(matches!(outcome, ExtractOutcome::FetchFailed(_)));
}
