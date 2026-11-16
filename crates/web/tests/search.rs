//! M9 web test: the /search page returns semantically-matched articles.
//! Requires `TEST_DATABASE_URL`; skipped otherwise.

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

use rusty_feed_cache::Cache;
use rusty_feed_db::models::NewArticle;
use rusty_feed_db::{articles, embeddings, feeds, subscriptions, user_preferences, users, Db};
use rusty_feed_embed::{Embedder, HashEmbedder};
use rusty_feed_web::{router, AppSettings, AppState};

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
    let csrf = find(&set_cookies(&resp), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rusty_feed_csrf={csrf}"))
                .body(Body::from(format!(
                    "name=&email={email}&password=Str0ng-Test-Pass-9x&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let sid = find(&set_cookies(&resp), "rusty_feed_sid")
        .unwrap()
        .to_string();
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
            comments_url: None,
            guid: None,
            title: "Async Rust Concurrency".into(),
            author: None,
            content: "a deep dive into async await and concurrency in the rust language".into(),
            excerpt: Some("async rust".into()),
            image_url: None,
            enclosures: Vec::new(),
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
        .clone()
        .oneshot(
            Request::get("/search?q=async%20rust%20concurrency")
                .header("cookie", format!("rusty_feed_sid={sid}"))
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
    assert!(body.contains("% words"));
    assert!(body.contains("% meaning"));

    // A personal keyword preference uses the indexed path and can find an
    // article before embedding. Phrase/exclusion syntax is handled by
    // websearch_to_tsquery, not by SQL string construction.
    for (title, content) in [
        ("Exact Phrase Guide", "A stable release."),
        ("Exact Phrase Draft", "An unfinished release."),
    ] {
        articles::upsert(
            db.pool(),
            &NewArticle {
                feed_id: feed.id,
                url: format!("https://ex.com/{}", uuid::Uuid::now_v7()),
                comments_url: None,
                guid: None,
                title: title.into(),
                author: None,
                content: content.into(),
                excerpt: None,
                image_url: None,
                enclosures: Vec::new(),
                published_at: None,
            },
        )
        .await
        .unwrap();
    }
    let preferences = app
        .clone()
        .oneshot(
            Request::get("/settings/preferences")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = find(&set_cookies(&preferences), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let saved = app
        .clone()
        .oneshot(
            Request::post("/settings/preferences")
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!(
                    "csrf_token={csrf}&theme=system&articles_per_page=20&reading_mode=side-panel&search_settings_present=true&search_mode=keyword&search_text_config=simple&search_semantic_weight_percent=25"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let stored = user_preferences::get(db.pool(), user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored["search_mode"], "keyword");
    assert_eq!(stored["search_text_config"], "simple");
    assert_eq!(stored["search_semantic_weight_percent"], 25);
    let resp = app
        .clone()
        .oneshot(
            Request::get("/search?q=%22exact%20phrase%22%20-draft")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Exact Phrase Guide"));
    assert!(!body.contains("Exact Phrase Draft"));
    assert!(body.contains("keyword, simple text"));
    assert!(body.contains("% words"));
    assert!(!body.contains("% meaning"));

    let reset = app
        .oneshot(
            Request::post("/settings/preferences")
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!(
                    "csrf_token={csrf}&theme=system&articles_per_page=20&reading_mode=side-panel&search_settings_present=true&search_mode=instance&search_text_config=instance"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reset.status(), StatusCode::OK);
    let stored = user_preferences::get(db.pool(), user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored["search_mode"], "instance");
    assert_eq!(stored["search_text_config"], "instance");
    assert!(stored["search_semantic_weight_percent"].is_null());
}

/// `EMBEDDINGS_ENABLED=false` must be authoritative: neither an explicit
/// `?mode=semantic` request nor a personal preference saved before the
/// instance disabled embeddings should ever produce a semantic score, even
/// though the article is fully embedded and would otherwise match.
#[tokio::test]
async fn embeddings_disabled_forces_keyword_search_regardless_of_request_or_preference() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();
    let state = AppState::new(
        db.clone(),
        Cache::in_memory().await,
        AppSettings {
            embeddings_enabled: false,
            ..AppSettings::default()
        },
        String::new,
    );
    let app = router(state);
    let (sid, email) = register(&app).await;

    let user = users::find_by_email(db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    // A stale personal preference from before the instance disabled
    // embeddings — must not resurrect semantic search.
    user_preferences::upsert(
        db.pool(),
        user.id,
        &serde_json::json!({ "search_mode": "semantic" }),
    )
    .await
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
            comments_url: None,
            guid: None,
            title: "Async Rust Concurrency".into(),
            author: None,
            content: "a deep dive into async await and concurrency in the rust language".into(),
            excerpt: Some("async rust".into()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    // Fully embedded: if the disabled flag were ignored, this would still be
    // findable (and scored) via semantic similarity.
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

    let resp = app
        .oneshot(
            Request::get("/search?q=async%20rust%20concurrency&mode=semantic")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("Async Rust Concurrency"),
        "keyword search should still find the article"
    );
    assert!(body.contains("Current settings: keyword"));
    assert!(body.contains("% words"));
    assert!(!body.contains("% meaning"));
    assert!(body.contains("disabled by the instance administrator"));
}

/// More keyword matches than the ranking cap (FND-231) still return results,
/// and the page says that only the most recent matches were ranked.
#[tokio::test]
async fn search_says_when_only_the_most_recent_matches_were_ranked() {
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
    let cap = rusty_feed_db::search::KEYWORD_CANDIDATE_CAP;
    sqlx::query(
        "INSERT INTO articles (id, feed_id, url, original_url, title, content, published_at) \
         SELECT gen_random_uuid(), $1, $2 || n, $2 || n, 'Lumenword note ' || n, \
                'lumenword body', now() - make_interval(mins => n) \
         FROM generate_series(1, $3) AS n",
    )
    .bind(feed.id)
    .bind(format!("https://ex.com/{}/", uuid::Uuid::now_v7()))
    .bind(i32::try_from(cap + 1).unwrap())
    .execute(db.pool())
    .await
    .unwrap();

    let search = |query: &'static str| {
        app.clone().oneshot(
            Request::get(format!("/search?q={query}&mode=keyword"))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
    };
    let notice = "these results come from the most recent matches only";
    let resp = search("lumenword").await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains(notice), "the capped search should say so");
    assert!(
        body.contains("Lumenword note 1<"),
        "the newest match is kept"
    );

    let resp = search("%22lumenword%20note%2042%22").await.unwrap();
    let body = body_string(resp).await;
    assert!(body.contains("Lumenword note 42<"));
    assert!(!body.contains(notice), "a narrow search is not capped");
}

async fn subscribed_article(db: &Db, user_id: uuid::Uuid, title: &str) -> (uuid::Uuid, uuid::Uuid) {
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://ex.com/{}.xml", uuid::Uuid::now_v7()),
        "Cache feed",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(db.pool(), user_id, feed.id, None)
        .await
        .unwrap();
    let (article, _) = articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://ex.com/{}", uuid::Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: title.into(),
            author: None,
            content: "A body about caching.".into(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    (feed.id, article.id)
}

async fn search_body(app: &Router, sid: &str, query: &str) -> (String, Option<String>) {
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/search?q={query}"))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let csrf = find(&set_cookies(&resp), "rusty_feed_csrf").map(str::to_string);
    (body_string(resp).await, csrf)
}

async fn query_embeddings(db: &Db, user_id: uuid::Uuid) -> i64 {
    rusty_feed_db::cost_ledger::user_call_count_since(
        db.pool(),
        user_id,
        "embedding",
        rusty_feed_core::ai_budget::AiBudget::today_start(),
    )
    .await
    .unwrap()
}

async fn rename(db: &Db, article_id: uuid::Uuid, title: &str) {
    sqlx::query("UPDATE articles SET title = $2 WHERE id = $1")
        .bind(article_id)
        .bind(title)
        .execute(db.pool())
        .await
        .unwrap();
}

/// FND-233: a repeated search is served from the per-user cache (no second
/// query embedding, no database query), another user never sees it, and an
/// unsubscribe invalidates it immediately.
#[tokio::test]
async fn repeated_search_is_cached_per_user_until_subscriptions_change() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();
    let app = router(AppState::new(
        db.clone(),
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    ));
    let (sid, email) = register(&app).await;
    let reader = users::find_by_email(db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    let word = format!("cacheword{}", uuid::Uuid::now_v7().simple());
    let (feed_id, article_id) =
        subscribed_article(&db, reader.id, &format!("{word} original")).await;

    let (body, _) = search_body(&app, &sid, &word).await;
    assert!(body.contains(&format!("{word} original")));
    assert!(body.contains("% meaning"), "the default search is hybrid");
    assert_eq!(query_embeddings(&db, reader.id).await, 1);

    rename(&db, article_id, &format!("{word} renamed")).await;
    let (body, csrf) = search_body(&app, &sid, &word).await;
    assert!(
        body.contains(&format!("{word} original")),
        "the repeat is served from the cache"
    );
    assert_eq!(
        query_embeddings(&db, reader.id).await,
        1,
        "a cached search is neither embedded nor metered again"
    );

    // Another subscriber of the same shared feed gets their own fresh result.
    let (other_sid, other_email) = register(&app).await;
    let other = users::find_by_email(db.pool(), &other_email)
        .await
        .unwrap()
        .unwrap();
    subscriptions::subscribe(db.pool(), other.id, feed_id, None)
        .await
        .unwrap();
    let (body, _) = search_body(&app, &other_sid, &word).await;
    assert!(body.contains(&format!("{word} renamed")));
    assert!(!body.contains(&format!("{word} original")));

    // Unsubscribing drops the reader's cached results right away.
    let csrf = csrf.unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/feeds/{feed_id}/unsubscribe"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_redirection(), "{}", resp.status());
    let (body, _) = search_body(&app, &sid, &word).await;
    assert!(
        body.contains("No matches."),
        "fresh results after unsubscribe"
    );
    assert_eq!(query_embeddings(&db, reader.id).await, 2);
}

/// Degraded keyword-fallback results are not cached, so meaning search comes
/// back as soon as the embedder recovers.
#[tokio::test]
async fn degraded_search_results_are_not_cached() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();
    let app = router(
        AppState::new(
            db.clone(),
            Cache::in_memory().await,
            AppSettings::default(),
            String::new,
        )
        .with_embedder(std::sync::Arc::new(rusty_feed_embed::DisabledEmbedder)),
    );
    let (sid, email) = register(&app).await;
    let reader = users::find_by_email(db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    let word = format!("fallbackword{}", uuid::Uuid::now_v7().simple());
    let (_, article_id) = subscribed_article(&db, reader.id, &format!("{word} original")).await;

    let (body, _) = search_body(&app, &sid, &word).await;
    assert!(body.contains("temporarily unavailable"));
    assert!(body.contains(&format!("{word} original")));

    rename(&db, article_id, &format!("{word} renamed")).await;
    let (body, _) = search_body(&app, &sid, &word).await;
    assert!(body.contains("temporarily unavailable"));
    assert!(
        body.contains(&format!("{word} renamed")),
        "a degraded result must not be served from the cache"
    );
}
