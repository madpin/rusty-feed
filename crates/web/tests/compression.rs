//! Response compression (FND-243): gzip for compressible pages when the client
//! asks for it, opt-out via `HTTP_COMPRESSION=false`, plain bodies otherwise.
//! These tests never touch the database (a lazy pool is never connected).

use std::io::Read;

use axum::body::Body;
use axum::http::{header, Request, Response};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

use rusty_feed_cache::Cache;
use rusty_feed_db::{articles, feeds, sessions, subscriptions, users, Db, NewArticle};
use rusty_feed_web::auth::token;
use rusty_feed_web::{router, AppSettings, AppState};

async fn app(http_compression: bool) -> Router {
    let settings = AppSettings {
        http_compression,
        ..AppSettings::default()
    };
    router(AppState::new(
        Db::connect_lazy("postgres://user:pass@localhost/rusty_feed_test").unwrap(),
        Cache::in_memory().await,
        settings,
        String::new,
    ))
}

async fn get(app: &Router, uri: &str, accept_encoding: Option<&str>) -> Response<Body> {
    let mut request = Request::get(uri);
    if let Some(value) = accept_encoding {
        request = request.header(header::ACCEPT_ENCODING, value);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn bytes(response: Response<Body>) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

fn gunzip(data: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(data)
        .read_to_end(&mut decoded)
        .expect("valid gzip");
    decoded
}

#[tokio::test]
async fn html_is_gzipped_on_request_and_decodes_to_the_plain_body() {
    let app = app(true).await;
    let plain = get(&app, "/terms", None).await;
    assert!(!plain.headers().contains_key(header::CONTENT_ENCODING));
    let plain_headers = plain.headers().clone();
    let plain_body = bytes(plain).await;
    assert!(plain_body.len() > 1_024, "terms page is a real document");

    let compressed = get(&app, "/terms", Some("gzip")).await;
    assert_eq!(compressed.headers()[header::CONTENT_ENCODING], "gzip");
    assert!(compressed.headers()[header::VARY]
        .to_str()
        .unwrap()
        .to_ascii_lowercase()
        .contains("accept-encoding"));
    assert_eq!(
        compressed.headers()[header::REFERRER_POLICY],
        plain_headers[header::REFERRER_POLICY],
        "security headers survive compression"
    );
    assert!(compressed
        .headers()
        .contains_key(header::CONTENT_SECURITY_POLICY));
    let compressed_body = bytes(compressed).await;
    assert!(compressed_body.len() < plain_body.len() / 2);
    assert_eq!(gunzip(&compressed_body), plain_body);
}

#[tokio::test]
async fn requests_without_accept_encoding_get_plain_bodies() {
    let app = app(true).await;
    for uri in ["/terms", "/static/app.css", "/api/health"] {
        let response = get(&app, uri, None).await;
        assert!(
            !response.headers().contains_key(header::CONTENT_ENCODING),
            "{uri}"
        );
        let identity = get(&app, uri, Some("identity")).await;
        assert!(
            !identity.headers().contains_key(header::CONTENT_ENCODING),
            "{uri}"
        );
    }
}

#[tokio::test]
async fn static_assets_are_compressed_and_round_trip() {
    let app = app(true).await;
    let plain = bytes(get(&app, "/static/app.css", None).await).await;
    let response = get(&app, "/static/app.css", Some("gzip")).await;
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(gunzip(&bytes(response).await), plain);
}

#[tokio::test]
async fn http_compression_false_disables_the_layer() {
    let app = app(false).await;
    let response = get(&app, "/terms", Some("gzip")).await;
    assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
    assert!(
        !response.headers().contains_key(header::VARY)
            || !response.headers()[header::VARY]
                .to_str()
                .unwrap()
                .to_ascii_lowercase()
                .contains("accept-encoding")
    );
    assert!(bytes(response).await.len() > 1_024);
}

/// Wire-size evidence for dynamic pages (prints sizes; run with `--nocapture`).
#[tokio::test]
async fn gzip_shrinks_the_library_and_admin_pages() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let admin = users::create(
        db.pool(),
        &format!("gzip-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "admin",
        Some("hash"),
    )
    .await
    .unwrap();
    let generated = token::generate();
    sessions::create(
        db.pool(),
        admin.id,
        &generated.hash,
        time::OffsetDateTime::now_utc() + time::Duration::days(1),
    )
    .await
    .unwrap();
    users::mark_onboarded(db.pool(), admin.id).await.unwrap();
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://gzip.example/{}.xml", uuid::Uuid::now_v7()),
        "Gzip feed",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(db.pool(), admin.id, feed.id, None)
        .await
        .unwrap();
    for index in 0..40 {
        articles::upsert(
            db.pool(),
            &NewArticle {
                feed_id: feed.id,
                url: format!("https://gzip.example/article/{}", uuid::Uuid::now_v7()),
                comments_url: None,
                guid: Some(uuid::Uuid::now_v7().to_string()),
                title: format!("Article number {index} about compressible things"),
                author: Some("Author".to_string()),
                content: "<p>Body text for the listing.</p>".repeat(8),
                excerpt: Some("A short excerpt of the article for the Library list.".repeat(2)),
                image_url: None,
                enclosures: Vec::new(),
                published_at: None,
            },
        )
        .await
        .unwrap();
    }
    let app = router(AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    ));
    let cookie = format!("rusty_feed_sid={}", generated.raw);
    for uri in ["/articles", "/admin"] {
        let fetch = |encoding: Option<&'static str>| {
            let mut request = Request::get(uri).header(header::COOKIE, cookie.clone());
            if let Some(encoding) = encoding {
                request = request.header(header::ACCEPT_ENCODING, encoding);
            }
            app.clone().oneshot(request.body(Body::empty()).unwrap())
        };
        let plain = fetch(None).await.unwrap();
        assert_eq!(plain.status(), 200, "{uri}");
        let plain = bytes(plain).await;
        let compressed = fetch(Some("gzip")).await.unwrap();
        assert_eq!(compressed.status(), 200, "{uri}");
        assert_eq!(
            compressed.headers()[header::CONTENT_ENCODING],
            "gzip",
            "{uri}"
        );
        let compressed = bytes(compressed).await;
        let decoded = gunzip(&compressed);
        eprintln!(
            "{uri}: plain {} bytes, gzip {} bytes ({:.1}% of plain)",
            plain.len(),
            compressed.len(),
            100.0 * compressed.len() as f64 / plain.len() as f64
        );
        assert!(compressed.len() < plain.len() / 2, "{uri}");
        assert!(
            decoded.len().abs_diff(plain.len()) < 256,
            "{uri}: same document"
        );
    }
}
