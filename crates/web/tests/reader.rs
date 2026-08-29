//! M5-start integration tests: reading list, preferences, and static assets.
//! Require `TEST_DATABASE_URL` (except the static-asset test); skipped otherwise.

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

use rustss_cache::Cache;
use rustss_db::{user_preferences, users, Db};
use rustss_web::{router, AppSettings, AppState};

async fn test_state() -> Option<AppState> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    ))
}

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

/// Register a fresh user; returns (session cookie value, local email).
async fn register(app: &Router) -> (String, String) {
    let email = format!("r-{}%40example.com", uuid::Uuid::now_v7());
    let resp = app
        .clone()
        .oneshot(Request::get("/register").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let csrf = find(&set_cookies(&resp), "rustss_csrf")
        .unwrap()
        .to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "name=&email={email}&password=password123&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let sid = find(&set_cookies(&resp), "rustss_sid").unwrap().to_string();
    (sid, email.replace("%40", "@"))
}

#[tokio::test]
async fn articles_shows_empty_state() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state);
    let (sid, _email) = register(&app).await;

    let resp = app
        .oneshot(
            Request::get("/articles")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("No articles yet"));
    assert!(!body.contains("reading-pane"));
    assert!(!body.contains("reading-workspace"));
}

#[tokio::test]
async fn articles_requires_auth() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state);
    let resp = app
        .oneshot(Request::get("/articles").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn preferences_save_roundtrip() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state.clone());
    let (sid, email) = register(&app).await;

    // GET issues a CSRF cookie.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/settings/preferences")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let csrf = find(&set_cookies(&resp), "rustss_csrf")
        .unwrap()
        .to_string();

    // Save a visual configuration, 30/page, and hide read (checkbox omitted).
    let resp = app
        .clone()
        .oneshot(
            Request::post("/settings/preferences")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "theme=ocean&reading_mode=side-panel&articles_per_page=30&article_density=compact&reader_font_size=large&reader_line_height=airy&sidebar_width=19&sidebar_collapsed=true&reader_panel_width=61&shortcuts_enabled=true&shortcut_next_article=ArrowDown&shortcut_previous_article=ArrowUp&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Preferences saved"));
    assert!(body.contains("data-theme=\"ocean\""));
    assert!(body.contains("--sidebar-width: 19rem"));
    assert!(body.contains("data-sidebar-collapsed=\"true\""));
    assert!(body.contains("--reader-panel-width: 61%"));
    assert!(body.contains("data-shortcut-next-article=\"ArrowDown\""));

    // Persisted correctly.
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    let stored = user_preferences::get(state.db.pool(), user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored["theme"], "ocean");
    assert_eq!(stored["reading_mode"], "side-panel");
    assert_eq!(stored["articles_per_page"], 30);
    assert_eq!(stored["article_density"], "compact");
    assert_eq!(stored["reader_font_size"], "large");
    assert_eq!(stored["reader_line_height"], "airy");
    assert_eq!(stored["sidebar_width"], 19);
    assert_eq!(stored["sidebar_collapsed"], true);
    assert_eq!(stored["reader_panel_width"], 61);
    assert_eq!(stored["shortcuts"]["enabled"], true);
    assert_eq!(stored["shortcuts"]["next_article"], "ArrowDown");
    assert_eq!(stored["shortcuts"]["previous_article"], "ArrowUp");
    assert_eq!(stored["show_read_articles"], false);
    assert_eq!(stored["auto_mark_read"], false);

    let response = app
        .oneshot(
            Request::post("/settings/layout")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "csrf_token={csrf}&reader_panel_width=45&sidebar_collapsed=false"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let stored = user_preferences::get(state.db.pool(), user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored["theme"], "ocean");
    assert_eq!(stored["sidebar_collapsed"], false);
    assert_eq!(stored["reader_panel_width"], 45);
}

#[tokio::test]
async fn duplicate_shortcuts_return_an_accessible_error_without_saving() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let response = app
        .clone()
        .oneshot(
            Request::get("/settings/preferences")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = find(&set_cookies(&response), "rustss_csrf")
        .unwrap()
        .to_string();
    let response = app
        .oneshot(
            Request::post("/settings/preferences")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "theme=system&reading_mode=side-panel&articles_per_page=20&shortcuts_enabled=true&shortcut_next_article=j&shortcut_previous_article=j&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Previous article conflicts with Next article."));
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    assert!(user_preferences::get(state.db.pool(), user.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn reading_actions_and_detail_work() {
    use rustss_db::models::NewArticle;
    use rustss_db::{articles, bookmarks, feeds, read_articles, subscriptions};

    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let pool = state.db.pool();

    // Seed: a subscribed feed with one article (feed ingestion is M6).
    let user = users::find_by_email(pool, &email).await.unwrap().unwrap();
    let feed_url = format!("https://ex.com/{}.xml", uuid::Uuid::now_v7());
    let (feed, _) = feeds::upsert_by_url(pool, &feed_url, "Example Feed", None)
        .await
        .unwrap();
    subscriptions::subscribe(pool, user.id, feed.id, None)
        .await
        .unwrap();
    let art_url = format!("https://ex.com/{}", uuid::Uuid::now_v7());
    let (article, _) = articles::upsert(
        pool,
        &NewArticle {
            feed_id: feed.id,
            url: art_url,
            comments_url: Some("https://news.ycombinator.com/item?id=42".to_string()),
            guid: Some(uuid::Uuid::now_v7().to_string()),
            title: "Hello World Article".into(),
            author: None,
            content: "<p>Body text</p><figure><img src=\"/cover.png\" alt=\"Cover\"></figure><script>alert(1)</script>".into(),
            excerpt: Some("An excerpt".into()),
            image_url: None,
            published_at: None,
        },
    )
    .await
    .unwrap();
    let aid = article.id;

    // The list shows the article and gives us a CSRF token.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/articles")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let csrf = find(&set_cookies(&resp), "rustss_csrf")
        .unwrap()
        .to_string();
    assert!(body_string(resp).await.contains("Hello World Article"));

    // Mark read (as the JS client would): 204, and it persists.
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{aid}/read"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(read_articles::is_read(pool, user.id, aid).await.unwrap());

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{aid}/unread"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(!read_articles::is_read(pool, user.id, aid).await.unwrap());

    // Bookmark it (plain post → redirect).
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{aid}/bookmark"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!("csrf_token={csrf}&next=/articles")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(bookmarks::is_bookmarked(pool, user.id, aid).await.unwrap());

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{aid}/unbookmark"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(!bookmarks::is_bookmarked(pool, user.id, aid).await.unwrap());

    let resp = app
        .clone()
        .oneshot(
            Request::post("/articles/read-all")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(read_articles::is_read(pool, user.id, aid).await.unwrap());

    // Detail page renders sanitized content (the <script> is stripped).
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/articles/{aid}"))
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Hello World Article"));
    assert!(body.contains("Open comments"));
    assert!(body.contains("https://news.ycombinator.com/item?id=42"));
    assert!(body.contains("AI summary unavailable"));
    assert!(body.contains("Body text"));
    assert!(body.contains("https://ex.com/cover.png"));
    assert!(body.contains("data-live-view-toggle"));
    assert!(body.contains("data-reader-view-content"));
    assert!(body.contains("data-live-view"));
    let frame_tag = body
        .split("<iframe")
        .nth(1)
        .and_then(|fragment| fragment.split('>').next())
        .expect("reader should render a live-view frame");
    assert!(frame_tag.contains("data-live-src=\"https://ex.com/"));
    assert!(frame_tag.contains("sandbox=\"allow-scripts\""));
    assert!(frame_tag.contains("referrerpolicy=\"no-referrer\""));
    assert!(frame_tag.contains("credentialless"));
    assert!(!frame_tag.contains(" src="));
    assert!(
        !body.contains("<script>alert(1)"),
        "content must be sanitized"
    );

    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/articles/{aid}/panel"))
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let panel = body_string(resp).await;
    assert!(panel.contains("data-panel-article"));
    assert!(panel.contains("data-panel-read=\"true\""));
    assert!(panel.contains("Hello World Article"));
    assert!(panel.contains("Open comments"));
    assert!(panel.contains("data-live-view-toggle"));
    assert!(panel.contains("data-reader-view-content"));
    let panel_frame_tag = panel
        .split("<iframe")
        .nth(1)
        .and_then(|fragment| fragment.split('>').next())
        .expect("panel should render a live-view frame");
    assert!(panel_frame_tag.contains("data-live-src=\"https://ex.com/"));
    assert!(panel_frame_tag.contains("sandbox=\"allow-scripts\""));
    assert!(panel_frame_tag.contains("referrerpolicy=\"no-referrer\""));
    assert!(panel_frame_tag.contains("credentialless"));
    assert!(!panel_frame_tag.contains(" src="));
}

#[tokio::test]
async fn article_detail_omits_unsafe_original_urls() {
    use rustss_db::models::NewArticle;
    use rustss_db::{articles, feeds, subscriptions};

    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let pool = state.db.pool();
    let user = users::find_by_email(pool, &email).await.unwrap().unwrap();
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://ex.com/{}.xml", uuid::Uuid::now_v7()),
        "Unsafe URL Feed",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(pool, user.id, feed.id, None)
        .await
        .unwrap();
    let unsafe_url = format!("javascript:alert({})", uuid::Uuid::now_v7());
    let (article, _) = articles::upsert(
        pool,
        &NewArticle {
            feed_id: feed.id,
            url: unsafe_url.clone(),
            comments_url: None,
            guid: None,
            title: "Unsafe source".into(),
            author: None,
            content: "<p>Safe stored content</p>".into(),
            excerpt: None,
            image_url: None,
            published_at: None,
        },
    )
    .await
    .unwrap();

    for endpoint in [
        format!("/articles/{}", article.id),
        format!("/articles/{}/panel", article.id),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get(endpoint)
                    .header("cookie", format!("rustss_sid={sid}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains("Original link unavailable"));
        assert!(!body.contains(&unsafe_url));
        assert!(!body.contains("data-live-view-toggle"));
        assert!(!body.contains("data-live-view-frame"));
    }
}

#[tokio::test]
async fn feedback_records_patterns_and_relevance_sort_works() {
    use rustss_db::models::NewArticle;
    use rustss_db::{articles, feeds, patterns, subscriptions};

    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let pool = state.db.pool();

    let user = users::find_by_email(pool, &email).await.unwrap().unwrap();
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://ex.com/{}.xml", uuid::Uuid::now_v7()),
        "F",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(pool, user.id, feed.id, None)
        .await
        .unwrap();
    let (article, _) = articles::upsert(
        pool,
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://ex.com/{}", uuid::Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: "Async Rust Concurrency".into(),
            author: None,
            content: "async await concurrency in the rust programming language".into(),
            excerpt: Some("async rust".into()),
            image_url: None,
            published_at: None,
        },
    )
    .await
    .unwrap();

    // CSRF from the article list.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/articles")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = find(&set_cookies(&resp), "rustss_csrf")
        .unwrap()
        .to_string();

    // Thumbs up via the fetch path → 204, and patterns are learned.
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{}/feedback", article.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!("value=up&csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(patterns::count_for(pool, user.id).await.unwrap() > 0);

    // Relevance sort renders and surfaces the liked article with a score badge.
    let resp = app
        .oneshot(
            Request::get("/articles?sort=relevance")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Async Rust Concurrency"));
    assert!(body.contains("Personalized"));
}

#[tokio::test]
async fn summarize_enqueues_a_job() {
    use rustss_db::models::NewArticle;
    use rustss_db::{articles, feeds, jobs, subscriptions};

    let Some(mut state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    std::sync::Arc::make_mut(&mut state.settings).summarization_enabled = true;
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let pool = state.db.pool();

    let user = users::find_by_email(pool, &email).await.unwrap().unwrap();
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://ex.com/{}.xml", uuid::Uuid::now_v7()),
        "F",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(pool, user.id, feed.id, None)
        .await
        .unwrap();
    let (article, _) = articles::upsert(
        pool,
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://ex.com/{}", uuid::Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: "To summarize".into(),
            author: None,
            content: "long content here".into(),
            excerpt: None,
            image_url: None,
            published_at: None,
        },
    )
    .await
    .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::get("/articles")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_string(response).await.contains("Summarize with AI"));

    // Get a CSRF token from the article page.
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/articles/{}", article.id))
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = find(&set_cookies(&resp), "rustss_csrf")
        .unwrap()
        .to_string();
    assert!(body_string(resp).await.contains("Generate AI summary"));

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{}/summarize", article.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let dedup = format!("summarize:{}", article.id);
    assert_eq!(jobs::count_active_by_dedup(pool, &dedup).await.unwrap(), 1);
    assert!(articles::set_summary(
        pool,
        article.id,
        article.content_revision,
        "AI-generated article summary",
        &["Key point".to_string()],
        &["Topic".to_string()],
        "test-model",
    )
    .await
    .unwrap());
    let response = app
        .clone()
        .oneshot(
            Request::get("/articles")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_string(response).await.contains("View AI summary"));
    let response = app
        .oneshot(
            Request::get(format!("/articles/{}", article.id))
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_string(response)
        .await
        .contains("AI-generated article summary"));
}

#[tokio::test]
async fn static_css_is_served() {
    // No DB needed — build a lazy-pool state.
    let db = Db::connect_lazy("postgres://u:p@localhost/none").unwrap();
    let state = AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    );
    let app = router(state);

    let resp = app
        .clone()
        .oneshot(Request::get("/static/app.css").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/css; charset=utf-8"
    );
    assert!(body_string(resp).await.contains("--accent"));

    let resp = app
        .oneshot(Request::get("/static/app.js").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let js = body_string(resp).await;
    assert!(js.contains("new URLSearchParams()"));
    assert!(js.contains("shortcutFromEvent"));
    assert!(js.contains("data-shortcuts-enabled"));
    assert!(!js.contains("arguments.callee"));
}
