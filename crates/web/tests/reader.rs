//! M5-start integration tests: reading list, preferences, and static assets.
//! Require `TEST_DATABASE_URL` (except the static-asset test); skipped otherwise.

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

use rusty_feed_cache::Cache;
use rusty_feed_db::{user_preferences, users, Db};
use rusty_feed_web::{router, AppSettings, AppState};

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
async fn articles_shows_empty_filtered_state() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state);
    let (sid, _email) = register(&app).await;

    let resp = app
        .oneshot(
            Request::get("/articles?tag=missing-release-test-tag")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("No articles yet"));
    assert!(!body.contains("article-card"));
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
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let csrf = find(&set_cookies(&resp), "rusty_feed_csrf")
        .unwrap()
        .to_string();

    // Save a visual configuration, 30/page, and hide read (checkbox omitted).
    let resp = app
        .clone()
        .oneshot(
            Request::post("/settings/preferences")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"))
                .body(Body::from(format!(
                    "theme=rusty&reading_mode=side-panel&articles_per_page=30&article_density=compact&font_family=serif&reader_font_size=large&reader_line_height=airy&sidebar_width=19&sidebar_collapsed=true&reader_panel_width=61&external_link_mode=original&external_open_mode=same-tab&enclosure_settings_present=true&enclosure_load_mode=links-only&enclosure_delivery_mode=mixed-content&enclosure_audio=true&youtube_settings_present=true&youtube_player_mode=privacy&youtube_autoplay=true&youtube_captions=on&touch_gestures_enabled=true&touch_gesture_sensitivity=high&touch_gesture_direction=reversed&shortcuts_enabled=true&shortcut_next_article=ArrowDown&shortcut_previous_article=ArrowUp&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Preferences saved"));
    assert!(body.contains("data-theme=\"rusty\""));
    assert!(body.contains("data-font-family=\"serif\""));
    assert!(body.contains("sidebar-width-19"));
    assert!(body.contains("data-sidebar-collapsed=\"true\""));
    assert!(body.contains("reader-panel-width-61"));
    assert!(body.contains("data-shortcut-next-article=\"ArrowDown\""));
    assert!(body.contains("data-touch-gestures-enabled=\"true\""));
    assert!(body.contains("data-touch-gesture-sensitivity=\"high\""));
    assert!(body.contains("data-touch-gesture-direction=\"reversed\""));

    // Persisted correctly.
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    let stored = user_preferences::get(state.db.pool(), user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored["theme"], "rusty");
    assert_eq!(stored["reading_mode"], "side-panel");
    assert_eq!(stored["articles_per_page"], 30);
    assert_eq!(stored["article_density"], "compact");
    assert_eq!(stored["font_family"], "serif");
    assert_eq!(stored["reader_font_size"], "large");
    assert_eq!(stored["reader_line_height"], "airy");
    assert_eq!(stored["sidebar_width"], 19);
    assert_eq!(stored["sidebar_collapsed"], true);
    assert_eq!(stored["reader_panel_width"], 61);
    assert_eq!(stored["external_link_mode"], "original");
    assert_eq!(stored["external_open_mode"], "same-tab");
    assert_eq!(stored["show_blocked_tracking_notice"], true);
    assert_eq!(stored["enclosure_load_mode"], "links-only");
    assert_eq!(stored["enclosure_delivery_mode"], "mixed-content");
    assert_eq!(
        stored["enclosure_player_kinds"],
        serde_json::json!(["audio"])
    );
    assert_eq!(stored["enclosure_show_links"], false);
    assert_eq!(stored["youtube_player_mode"], "privacy");
    assert_eq!(stored["youtube_autoplay"], true);
    assert_eq!(stored["youtube_captions"], "on");
    assert_eq!(stored["touch_gestures_enabled"], true);
    assert_eq!(stored["touch_gesture_sensitivity"], "high");
    assert_eq!(stored["touch_gesture_direction"], "reversed");
    assert_eq!(stored["shortcuts"]["enabled"], true);
    assert_eq!(stored["shortcuts"]["next_article"], "ArrowDown");
    assert_eq!(stored["shortcuts"]["previous_article"], "ArrowUp");
    assert_eq!(stored["show_read_articles"], false);
    assert_eq!(stored["auto_mark_read"], false);

    let response = app
        .clone()
        .oneshot(
            Request::post("/settings/layout")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
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
    assert_eq!(stored["theme"], "rusty");
    assert_eq!(stored["sidebar_collapsed"], false);
    assert_eq!(stored["reader_panel_width"], 45);

    let response = app
        .clone()
        .oneshot(
            Request::post("/settings/layout")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!(
                    "csrf_token={csrf}&sidebar_collapsed=true"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = app
        .clone()
        .oneshot(
            Request::post("/settings/layout")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!(
                    "csrf_token={csrf}&reader_panel_width=62"
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
    assert_eq!(stored["theme"], "rusty");
    assert_eq!(stored["sidebar_collapsed"], true);
    assert_eq!(stored["reader_panel_width"], 62);
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
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = find(&set_cookies(&response), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let response = app
        .oneshot(
            Request::post("/settings/preferences")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"))
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
    use rusty_feed_db::models::{NewArticle, NewArticleEnclosure};
    use rusty_feed_db::{articles, bookmarks, feeds, integrations, read_articles, subscriptions};

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
    let original_article_url = format!("{art_url}?utm_source=publisher");
    let (article, _) = articles::upsert_with_original_url(
        pool,
        &NewArticle {
            feed_id: feed.id,
            url: art_url,
            comments_url: Some("https://news.ycombinator.com/item?id=42".to_string()),
            guid: Some(uuid::Uuid::now_v7().to_string()),
            title: "Hello World Article".into(),
            author: None,
            content: "<p>Body text with <a href=\"https://youtu.be/dQw4w9WgXcQ?t=42&utm_source=rss\">a video</a>.</p><figure><img src=\"/cover.png\" alt=\"Cover\"></figure><img src=\"/open.gif\" width=\"1\" height=\"1\"><script>alert(1)</script>".into(),
            excerpt: Some("An excerpt".into()),
            image_url: Some("https://cdn.example/header.jpg".into()),
            enclosures: vec![
                NewArticleEnclosure {
                    url: "https://cdn.example/episode.mp3".into(),
                    media_kind: "audio".into(),
                    mime_type: Some("audio/mpeg".into()),
                    title: Some("The full episode".into()),
                    size_bytes: Some(5_242_880),
                    duration_seconds: Some(754),
                    width: None,
                    height: None,
                },
                NewArticleEnclosure {
                    url: "https://cdn.example/trailer.mp4".into(),
                    media_kind: "video".into(),
                    mime_type: Some("video/mp4".into()),
                    title: Some("Trailer".into()),
                    size_bytes: None,
                    duration_seconds: Some(62),
                    width: Some(1280),
                    height: Some(720),
                },
                NewArticleEnclosure {
                    url: "https://cdn.example/vector.svg".into(),
                    media_kind: "image".into(),
                    mime_type: Some("image/svg+xml".into()),
                    title: Some("Vector artwork".into()),
                    size_bytes: None,
                    duration_seconds: None,
                    width: None,
                    height: None,
                },
                NewArticleEnclosure {
                    url: "https://cdn.example/notes.pdf".into(),
                    media_kind: "file".into(),
                    mime_type: Some("application/pdf".into()),
                    title: Some("Episode notes".into()),
                    size_bytes: None,
                    duration_seconds: None,
                    width: None,
                    height: None,
                },
            ],
            published_at: None,
        },
        Some(&original_article_url),
    )
    .await
    .unwrap();
    let aid = article.id;

    // The list shows the article and gives us a CSRF token.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/articles")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let csrf = find(&set_cookies(&resp), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let list_body = body_string(resp).await;
    assert!(list_body.contains("Hello World Article"));
    assert!(list_body.contains("data-seen-badge"));
    assert!(list_body.contains(&format!("/articles/{aid}/unseen")));
    assert!(!list_body.contains("AI summary unavailable"));

    let unseen_list = app
        .clone()
        .oneshot(
            Request::get("/articles?unseen=1")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unseen_list.status(), StatusCode::OK);
    let unseen_body = body_string(unseen_list).await;
    assert!(unseen_body.contains("data-skip-glance=\"true\""));
    assert!(unseen_body.contains("Hello World Article"));

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{aid}/seen"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(rusty_feed_db::seen_articles::is_seen(pool, user.id, aid)
        .await
        .unwrap());
    assert!(!read_articles::is_read(pool, user.id, aid).await.unwrap());

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{aid}/unseen"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(!rusty_feed_db::seen_articles::is_seen(pool, user.id, aid)
        .await
        .unwrap());

    // Mark read (as the JS client would): 204, and it persists.
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{aid}/read"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
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
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
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
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
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
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
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
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
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
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Hello World Article"));
    assert!(body.contains("Open clean link"));
    assert!(body.contains("Publisher URL"));
    assert!(body.contains(&original_article_url.replace('&', "&amp;")));
    assert!(body.contains("Open comments"));
    assert!(body.contains("https://news.ycombinator.com/item?id=42"));
    assert!(!body.contains("AI summary unavailable"));
    assert!(body.contains("Body text"));
    assert!(body.contains("https://ex.com/cover.png"));
    assert!(body.contains("Tracking images removed by Rusty Feed: 1."));
    assert!(!body.contains("open.gif"));
    assert!(body.contains("data-content-live-control"));
    assert!(body.contains("data-reader-surface"));
    assert!(body.contains("class=\"reader-view\""));
    assert!(body.contains("class=\"reader-toolbar\""));
    assert!(body.contains("data-article-send-trigger"));
    assert!(body.contains(&format!("data-modal-url=\"/articles/{aid}/send/modal\"")));
    assert!(body.contains("data-reader-view-content"));
    assert!(body.contains(&format!(
        "class=\"article-hero\" src=\"/articles/{aid}/hero\""
    )));
    assert!(!body.contains("class=\"article-hero\" src=\"https://cdn.example/header.jpg\""));
    assert!(!body.contains("summary-box"));
    assert!(body.contains("Attachments"));
    assert!(body.contains("The full episode"));
    assert!(body.contains("12:34 · 5.0 MB · audio/mpeg"));
    assert!(body.contains("data-enclosure-kind=\"audio\""));
    assert!(body.contains(&format!("data-src=\"/articles/{aid}/media/0\"")));
    let audio_tag = body
        .split("<audio")
        .nth(1)
        .and_then(|fragment| fragment.split('>').next())
        .expect("reader should render an on-demand audio player");
    assert!(!audio_tag.contains(" src="));
    assert!(body.contains("data-enclosure-kind=\"video\""));
    assert!(body.contains(&format!("data-src=\"/articles/{aid}/media/1\"")));
    assert!(!body.contains(&format!("data-src=\"/articles/{aid}/media/2\"")));
    assert!(body.contains(&format!("href=\"/articles/{aid}/media/2?download=1\"")));
    assert!(body.contains(&format!("href=\"/articles/{aid}/media/3?download=1\"")));
    assert!(body.contains("href=\"https://cdn.example/vector.svg\""));
    assert!(body.contains("href=\"https://cdn.example/notes.pdf\""));
    assert!(body.contains("YouTube video — Hello World Article"));
    assert!(body.contains("href=\"https://www.youtube.com/watch?v=dQw4w9WgXcQ&amp;t=42s\""));
    assert!(body.contains("Starts at 0:42"));
    assert!(body.contains("data-youtube-load"));
    assert!(body.contains("data-youtube-unload"));
    assert!(body.contains("data-youtube-player aria-busy=\"false\" hidden"));
    assert!(body.contains(
        "data-src=\"https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ?playsinline=1&amp;rel=0&amp;start=42\""
    ));
    let youtube_frame = body
        .split("<iframe")
        .skip(1)
        .find_map(|fragment| {
            let tag = fragment.split('>').next()?;
            tag.contains("data-youtube-frame").then_some(tag)
        })
        .expect("reader should render a deferred privacy YouTube player");
    assert!(youtube_frame.contains("referrerpolicy=\"strict-origin-when-cross-origin\""));
    assert!(!youtube_frame.contains("credentialless"));
    assert!(!youtube_frame.contains(" src="));
    assert!(body.contains("data-live-view"));
    assert!(body.contains("publisher frame restrictions do not block the page"));
    assert!(body.contains("data-live-view-status"));
    let frame_tag = body
        .split("<iframe")
        .skip(1)
        .find_map(|fragment| {
            let tag = fragment.split('>').next()?;
            tag.contains("data-live-view-frame").then_some(tag)
        })
        .expect("reader should render a live-view frame");
    assert!(frame_tag.contains(&format!(
        "data-live-ticket-url=\"/articles/{aid}/live-ticket\""
    )));
    assert!(frame_tag.contains("sandbox=\"allow-scripts allow-forms allow-popups"));
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
            Request::get(format!("/articles/{aid}/send/modal"))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let empty_modal = body_string(resp).await;
    assert!(empty_modal.contains("data-article-send-dialog"));
    assert!(empty_modal.contains("No active destination exists"));
    assert!(empty_modal.contains("/settings/integrations"));
    assert!(!empty_modal.contains("<html"));

    let profile = integrations::create_profile(
        pool,
        user.id,
        "Reading archive",
        "webhook",
        "https://hooks.example.test/rusty-feed",
        None,
        "none",
        "{{title}} {{url}}",
        &[],
        &["article.saved".into()],
        3,
        30,
        true,
    )
    .await
    .unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/articles/{aid}/send/modal"))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let modal_cookies = set_cookies(&resp);
    let modal_csrf = find(&modal_cookies, "rusty_feed_csrf").unwrap().to_string();
    let modal = body_string(resp).await;
    assert!(modal.contains("data-article-send-form"));
    assert!(modal.contains("Reading archive · webhook · default"));
    assert!(modal.contains(&format!("value=\"{}\" selected", profile.id)));
    assert!(!modal.contains("<html"));

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{aid}/send"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={modal_csrf}"),
                )
                .body(Body::from(format!(
                    "csrf_token={modal_csrf}&profile_id={}",
                    profile.id
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let deliveries = integrations::list_deliveries(pool, user.id, 10)
        .await
        .unwrap();
    assert!(deliveries.iter().any(|delivery| {
        delivery.profile_id == profile.id
            && delivery.article_id == Some(aid)
            && delivery.status == "queued"
    }));

    rusty_feed_db::subscriptions::update_youtube_mode(
        pool,
        user.id,
        feed.id,
        rusty_feed_core::youtube::SubscriptionYouTubeMode::LinksOnly,
    )
    .await
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/articles/{aid}/panel"))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let links_only = body_string(response).await;
    assert!(links_only.contains("Open on YouTube"));
    assert!(!links_only.contains("data-youtube-frame"));
    assert!(links_only.contains("Embedded playback is disabled"));

    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/articles/{aid}/live-ticket"))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("cache-control").unwrap(),
        "private, no-store"
    );
    assert!(body_string(resp)
        .await
        .starts_with(&format!("/articles/{aid}/live?ticket=")));

    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/articles/{aid}/panel"))
                .header("cookie", format!("rusty_feed_sid={sid}"))
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
    assert!(!panel.contains("AI summary unavailable"));
    assert!(panel.contains("data-content-live-control"));
    assert!(panel.contains("data-reader-surface"));
    assert!(panel.contains("class=\"reader-view\""));
    assert!(panel.contains("class=\"reader-toolbar\""));
    assert!(panel.contains("data-article-send-trigger"));
    assert!(panel.contains("data-reader-view-content"));
    assert!(panel.contains(&format!(
        "class=\"article-hero\" src=\"/articles/{aid}/hero\""
    )));
    assert!(!panel.contains("summary-box"));
    assert!(panel.contains("The full episode"));
    assert!(panel.contains(&format!("data-src=\"/articles/{aid}/media/0\"")));
    assert!(panel.contains("publisher frame restrictions do not block the page"));
    assert!(panel.contains("data-live-view-status"));
    let panel_frame_tag = panel
        .split("<iframe")
        .nth(1)
        .and_then(|fragment| fragment.split('>').next())
        .expect("panel should render a live-view frame");
    assert!(panel_frame_tag.contains(&format!(
        "data-live-ticket-url=\"/articles/{aid}/live-ticket\""
    )));
    assert!(panel_frame_tag.contains("sandbox=\"allow-scripts allow-forms allow-popups"));
    assert!(panel_frame_tag.contains("referrerpolicy=\"no-referrer\""));
    assert!(panel_frame_tag.contains("credentialless"));
    assert!(!panel_frame_tag.contains(" src="));
}

#[tokio::test]
async fn temporary_missing_public_page_queues_once_and_serves_rss_until_ready() {
    use rusty_feed_core::feed_content::ContentLifetime;
    use rusty_feed_db::models::NewArticle;
    use rusty_feed_db::{articles, feeds, jobs, subscriptions};

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
        &format!("https://access.example/{}.xml", uuid::Uuid::now_v7()),
        "Access feed",
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
            url: format!("https://access.example/{}", uuid::Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: "Access refresh".into(),
            author: None,
            content: "<p>RSS ACCESS SENTINEL</p>".into(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::get(format!("/articles/{}?content=article_page", article.id))
                    .header("cookie", format!("rusty_feed_sid={sid}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains("RSS ACCESS SENTINEL"));
        assert!(body.contains("data-content-pending=\"true\""));
    }
    let dedup = format!("extract:{}:{}", article.id, article.content_revision);
    assert_eq!(
        jobs::count_active_by_dedup(pool, &dedup).await.unwrap(),
        1,
        "access refresh must stay deduplicated"
    );
    let on_demand_days: Option<i32> = sqlx::query_scalar(
        "SELECT (payload->>'on_demand_lifetime_days')::int FROM job_queue WHERE dedup_key = $1",
    )
    .bind(&dedup)
    .fetch_optional(pool)
    .await
    .unwrap()
    .flatten();
    assert_eq!(on_demand_days, Some(1));
    assert_eq!(
        subscriptions::content_settings_for_user(pool, user.id, feed.id)
            .await
            .unwrap()
            .unwrap()
            .mode,
        rusty_feed_core::feed_content::SubscriptionContentMode::Rss
    );

    articles::set_extraction_failed(pool, article.id, article.content_revision, "reader_failed")
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE job_queue SET status = 'done' WHERE dedup_key = $1")
        .bind(&dedup)
        .execute(pool)
        .await
        .unwrap();
    let failed_response = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/articles/{}/content?content=article_page",
                article.id
            ))
            .header("cookie", format!("rusty_feed_sid={sid}"))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        failed_response
            .headers()
            .get("x-rusty-feed-content-status")
            .unwrap(),
        "failed"
    );
    assert_eq!(jobs::count_active_by_dedup(pool, &dedup).await.unwrap(), 0);
    assert!(body_string(failed_response)
        .await
        .contains("RSS ACCESS SENTINEL"));

    articles::set_extracted_content(
        pool,
        article.id,
        article.content_revision,
        "<p>PUBLIC ACCESS SENTINEL</p>",
        None,
        ContentLifetime::Days(7),
    )
    .await
    .unwrap()
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/articles/{}/content?content=article_page",
                article.id
            ))
            .header("cookie", format!("rusty_feed_sid={sid}"))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response
            .headers()
            .get("x-rusty-feed-content-status")
            .unwrap(),
        "ready"
    );
    assert!(body_string(response)
        .await
        .contains("PUBLIC ACCESS SENTINEL"));

    sqlx::query("UPDATE articles SET article_page_processing_version = 0 WHERE id = $1")
        .bind(article.id)
        .execute(pool)
        .await
        .unwrap();
    let stale_response = app
        .oneshot(
            Request::get(format!("/articles/{}?content=article_page", article.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let stale_body = body_string(stale_response).await;
    assert!(stale_body.contains("RSS ACCESS SENTINEL"));
    assert!(stale_body.contains("data-content-pending=\"true\""));
}

#[tokio::test]
async fn article_content_mode_switches_are_temporary_until_made_default() {
    use rusty_feed_core::feed_content::{ContentLifetime, SubscriptionContentMode};
    use rusty_feed_db::models::NewArticle;
    use rusty_feed_db::{articles, feeds, subscriptions};

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
        &format!("https://content-modes.example/{}.xml", uuid::Uuid::now_v7()),
        "Content modes",
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
            url: format!("https://content-modes.example/{}", uuid::Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: "Three content modes".into(),
            author: None,
            content: "<p>RSS MODE SENTINEL</p>".into(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    articles::set_extracted_content(
        pool,
        article.id,
        article.content_revision,
        "<p>PAGE MODE SENTINEL</p>",
        None,
        ContentLifetime::Days(30),
    )
    .await
    .unwrap()
    .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/articles/{}", article.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = find(&set_cookies(&response), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let body = body_string(response).await;
    assert!(body.contains("RSS MODE SENTINEL"));
    assert!(body.contains("data-content-mode-current=\"rss\""));
    assert!(body.contains(">Page content</a>"));
    assert!(body.contains(">Live</a>"));
    assert!(!body.contains("Make default for this feed"));

    let temporary_page = app
        .clone()
        .oneshot(
            Request::get(format!("/articles/{}?content=article_page", article.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let temporary_page = body_string(temporary_page).await;
    assert!(temporary_page.contains("PAGE MODE SENTINEL"));
    assert!(temporary_page.contains("data-content-mode-current=\"article_page\""));
    assert!(temporary_page.contains("Make default for this feed"));
    assert_eq!(
        subscriptions::content_settings_for_user(pool, user.id, feed.id)
            .await
            .unwrap()
            .unwrap()
            .mode,
        SubscriptionContentMode::Rss
    );
    assert!(
        subscriptions::requested_article_page_lifetime(pool, feed.id)
            .await
            .unwrap()
            .is_none(),
        "temporary Page Content must not change durable extraction demand"
    );

    let temporary_live = app
        .clone()
        .oneshot(
            Request::get(format!("/articles/{}?content=live", article.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let temporary_live = body_string(temporary_live).await;
    assert!(temporary_live.contains("RSS MODE SENTINEL"));
    assert!(temporary_live.contains("data-content-mode-current=\"live\""));
    assert!(temporary_live.contains("Make default for this feed"));
    assert_eq!(
        subscriptions::content_settings_for_user(pool, user.id, feed.id)
            .await
            .unwrap()
            .unwrap()
            .mode,
        SubscriptionContentMode::Rss
    );

    let make_live_default = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{}/content-mode", article.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!(
                    "csrf_token={csrf}&content_mode=live&next=/articles/{}",
                    article.id
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(make_live_default.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        subscriptions::content_settings_for_user(pool, user.id, feed.id)
            .await
            .unwrap()
            .unwrap()
            .mode,
        SubscriptionContentMode::Live
    );

    let temporary_rss = app
        .clone()
        .oneshot(
            Request::get(format!("/articles/{}?content=rss", article.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let temporary_rss = body_string(temporary_rss).await;
    assert!(temporary_rss.contains("RSS MODE SENTINEL"));
    assert!(temporary_rss.contains("Make default for this feed"));
    assert_eq!(
        subscriptions::content_settings_for_user(pool, user.id, feed.id)
            .await
            .unwrap()
            .unwrap()
            .mode,
        SubscriptionContentMode::Live
    );

    let make_page_default = app
        .oneshot(
            Request::post(format!("/articles/{}/content-mode", article.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!(
                    "csrf_token={csrf}&content_mode=article_page&next=/articles/{}",
                    article.id
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(make_page_default.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        subscriptions::content_settings_for_user(pool, user.id, feed.id)
            .await
            .unwrap()
            .unwrap()
            .mode,
        SubscriptionContentMode::ArticlePage
    );
    assert!(
        subscriptions::requested_article_page_lifetime(pool, feed.id)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        rusty_feed_db::jobs::count_active_by_dedup(
            pool,
            &format!("extract:{}:{}", article.id, article.content_revision),
        )
        .await
        .unwrap(),
        0,
        "current shared page content must not be re-extracted"
    );
}

#[tokio::test]
async fn youtube_from_shared_page_content_renders_in_every_reader_mode() {
    use rusty_feed_core::feed_content::ContentLifetime;
    use rusty_feed_db::models::NewArticle;
    use rusty_feed_db::{articles, feeds, subscriptions};

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
        &format!("https://youtube-page.example/{}.xml", uuid::Uuid::now_v7()),
        "YouTube page content",
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
            url: format!("https://youtube-page.example/{}", uuid::Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: "YouTube in shared page content".into(),
            author: None,
            content: "<p>RSS body without a video link</p>".into(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    articles::set_extracted_content(
        pool,
        article.id,
        article.content_revision,
        "<p><a href=\"https://www.youtube.com/watch?v=GGJSRFWALTI\">Watch video on YouTube</a></p>",
        None,
        ContentLifetime::Days(30),
    )
    .await
    .unwrap()
    .unwrap();

    for endpoint in [
        format!("/articles/{}", article.id),
        format!("/articles/{}?content=article_page", article.id),
        format!("/articles/{}/panel", article.id),
        format!("/articles/{}/panel?content=article_page", article.id),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get(endpoint)
                    .header("cookie", format!("rusty_feed_sid={sid}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains("data-youtube-loader"));
        assert!(body.contains("GGJSRFWALTI"));
    }
}

#[tokio::test]
async fn article_detail_omits_unsafe_original_urls() {
    use rusty_feed_core::feed_content::{ContentLifetime, SubscriptionContentMode};
    use rusty_feed_db::models::NewArticle;
    use rusty_feed_db::{articles, feeds, subscriptions};

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
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    subscriptions::update_content_settings(
        pool,
        user.id,
        feed.id,
        rusty_feed_core::feed_content::SubscriptionContentSettings {
            mode: SubscriptionContentMode::Live,
            lifetime: ContentLifetime::Days(30),
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
                    .header("cookie", format!("rusty_feed_sid={sid}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains("Original link unavailable"));
        assert!(body.contains("data-content-mode-current=\"live\""));
        assert!(body.contains("Live unavailable"));
        assert!(body.contains("Showing its stored RSS content instead"));
        assert!(!body.contains(&unsafe_url));
        assert!(!body.contains("data-live-view-toggle"));
        assert!(!body.contains("data-content-live-control"));
        assert!(!body.contains("data-live-view-frame"));
    }
}

#[tokio::test]
async fn sidebar_keeps_all_feeds_visible_and_exposes_unfiled_scope() {
    use rusty_feed_core::feed_content::{ContentLifetime, SubscriptionContentMode};
    use rusty_feed_db::models::NewArticle;
    use rusty_feed_db::{articles, categories, feeds, read_articles, subscriptions};

    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let pool = state.db.pool();
    let user = users::find_by_email(pool, &email).await.unwrap().unwrap();
    let folder = categories::create(pool, user.id, "Reader folder", None)
        .await
        .unwrap();
    categories::set_collapsed(pool, user.id, folder.id, true)
        .await
        .unwrap();
    let (filed_feed, _) = feeds::upsert_by_url(
        pool,
        &format!(
            "https://reader-sidebar.example/{}/filed.xml",
            uuid::Uuid::now_v7()
        ),
        "Filed source",
        None,
    )
    .await
    .unwrap();
    let (unfiled_feed, _) = feeds::upsert_by_url(
        pool,
        &format!(
            "https://reader-sidebar.example/{}/unfiled.xml",
            uuid::Uuid::now_v7()
        ),
        "Unfiled source",
        None,
    )
    .await
    .unwrap();
    for feed in [&filed_feed, &unfiled_feed] {
        subscriptions::subscribe(pool, user.id, feed.id, None)
            .await
            .unwrap();
    }
    subscriptions::update_organization(
        pool,
        user.id,
        filed_feed.id,
        &["Reader-Tag".to_string()],
        Some(folder.id),
    )
    .await
    .unwrap();
    subscriptions::update_organization(
        pool,
        user.id,
        unfiled_feed.id,
        &["Reader-Tag".to_string()],
        None,
    )
    .await
    .unwrap();
    for (feed_id, title) in [
        (filed_feed.id, "Visible only in folder scope"),
        (unfiled_feed.id, "Visible only in unfiled scope"),
    ] {
        articles::upsert(
            pool,
            &NewArticle {
                feed_id,
                url: format!(
                    "https://reader-sidebar.example/article/{}",
                    uuid::Uuid::now_v7()
                ),
                comments_url: None,
                guid: None,
                title: title.to_string(),
                author: None,
                content: "body".to_string(),
                excerpt: None,
                image_url: None,
                enclosures: Vec::new(),
                published_at: None,
            },
        )
        .await
        .unwrap();
    }

    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/articles?category={}", folder.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let csrf = find(&set_cookies(&response), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let body = body_string(response).await;
    assert!(body.contains("Visible only in folder scope"));
    assert!(body.contains("Filed source"));
    assert!(body.contains("Unfiled source"));
    assert!(body.contains("href=\"/articles?unread=1&amp;unfiled=1\""));
    assert!(body.contains(&format!(
        "href=\"/articles?unread=1&amp;feed={}\"",
        unfiled_feed.id
    )));
    assert!(!body.contains("<details class=\"folder-tree-node"));
    assert!(body.contains("class=\"page-articles side-panel-mode\""));
    assert!(body.contains(&format!("data-folder-collapse=\"{}\"", folder.id)));
    assert!(body.contains("data-folder-collapsed=\"true\""));
    assert!(body.contains(&format!("data-folder-id=\"{}\"", folder.id)));
    assert!(body.contains(&format!("data-feed-id=\"{}\"", filed_feed.id)));
    assert!(body.contains("data-sidebar-quick-toggle"));
    assert!(body.contains("data-content-settings-dialog"));
    assert!(body.contains("data-tags-settings-dialog"));
    assert!(body.contains("data-move-feed-dialog"));
    assert!(body.contains("href=\"/articles?tag=Reader-Tag\" data-library-tag-nav"));
    assert!(!body.contains(&format!("category={}&amp;tag=Reader-Tag", folder.id)));
    let filed_article = articles::list_for_feed(pool, filed_feed.id, 1)
        .await
        .unwrap()
        .pop()
        .unwrap();
    read_articles::mark_read(pool, user.id, filed_article.id)
        .await
        .unwrap();
    user_preferences::upsert(
        pool,
        user.id,
        &serde_json::json!({ "show_read_articles": false }),
    )
    .await
    .unwrap();

    let tagged = app
        .clone()
        .oneshot(
            Request::get("/articles?tag=reader-tag")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tagged.status(), StatusCode::OK);
    let tagged = body_string(tagged).await;
    assert!(tagged.contains("Tag: #reader-tag"));
    assert!(tagged.contains("Visible only in folder scope"));
    assert!(tagged.contains("Visible only in unfiled scope"));

    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/feeds/{}/content-settings", filed_feed.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!(
                    "csrf_token={csrf}&content_mode=article_page&content_lifetime=infinite"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let settings = subscriptions::content_settings_for_user(pool, user.id, filed_feed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settings.mode, SubscriptionContentMode::ArticlePage);
    assert_eq!(settings.lifetime, ContentLifetime::Infinite);

    let response = app
        .oneshot(
            Request::get("/articles?unfiled=1")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Visible only in unfiled scope"));
    assert!(!body.contains("Visible only in folder scope"));
}

#[tokio::test]
async fn feedback_records_patterns_and_relevance_sort_works() {
    use rusty_feed_db::models::NewArticle;
    use rusty_feed_db::{articles, feeds, patterns, subscriptions};

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
            enclosures: Vec::new(),
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
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = find(&set_cookies(&resp), "rusty_feed_csrf")
        .unwrap()
        .to_string();

    // Thumbs up via the fetch path → 204, and patterns are learned.
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{}/feedback", article.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
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
                .header("cookie", format!("rusty_feed_sid={sid}"))
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

/// A reader who queues several summaries and moves on to other articles needs
/// one filter that finds exactly the ones now ready and still unread — not
/// every summarized article (including already-read ones) and not every
/// unread article (including ones with no summary at all).
#[tokio::test]
async fn unread_ai_summaries_filter_shows_only_ready_and_unread() {
    use rusty_feed_db::models::NewArticle;
    use rusty_feed_db::{articles, feeds, read_articles, subscriptions};

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

    let (ready_unread, _) = articles::upsert(
        pool,
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://ex.com/{}", uuid::Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: "Ready Summary Unread".into(),
            author: None,
            content: "content for the unread ready summary".into(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    assert!(articles::set_summary(
        pool,
        ready_unread.id,
        ready_unread.content_revision,
        "A ready summary",
        &[],
        &[],
        "test-model",
    )
    .await
    .unwrap());

    let (ready_read, _) = articles::upsert(
        pool,
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://ex.com/{}", uuid::Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: "Ready Summary Already Read".into(),
            author: None,
            content: "content for the already-read ready summary".into(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    assert!(articles::set_summary(
        pool,
        ready_read.id,
        ready_read.content_revision,
        "Another ready summary",
        &[],
        &[],
        "test-model",
    )
    .await
    .unwrap());
    read_articles::mark_read(pool, user.id, ready_read.id)
        .await
        .unwrap();

    articles::upsert(
        pool,
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://ex.com/{}", uuid::Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: "No Summary Yet".into(),
            author: None,
            content: "content for the article with no summary".into(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();

    let resp = app
        .oneshot(
            Request::get("/articles?summary=1")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Ready Summary Unread"));
    assert!(!body.contains("Ready Summary Already Read"));
    assert!(!body.contains("No Summary Yet"));
    assert!(body.contains("Unread AI summaries"));
}

#[tokio::test]
async fn summarize_enqueues_a_job() {
    use rusty_feed_db::models::NewArticle;
    use rusty_feed_db::{articles, feeds, jobs, subscriptions};

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
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::get("/articles")
                .header("cookie", format!("rusty_feed_sid={sid}"))
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
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = find(&set_cookies(&resp), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    assert!(body_string(resp).await.contains("Generate AI summary"));

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/articles/{}/summarize", article.id))
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
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let dedup = format!("summarize:{}:{}", article.id, article.content_revision);
    assert_eq!(jobs::count_active_by_dedup(pool, &dedup).await.unwrap(), 1);
    let lane: String = sqlx::query_scalar(
        "SELECT queue_name FROM job_queue WHERE dedup_key = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&dedup)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(lane, "interactive");
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/articles/{}/summary/status", article.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response).await.contains("\"state\":\"queued\""));
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
            Request::get(format!("/articles/{}/summary/status", article.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_string(response).await.contains("\"state\":\"ready\""));
    let response = app
        .clone()
        .oneshot(
            Request::get("/articles?unread=0")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_string(response).await.contains("View AI summary"));
    let response = app
        .oneshot(
            Request::get(format!("/articles/{}", article.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_string(response).await;
    assert!(body.contains("AI-generated article summary"));
    assert!(body.contains("class=\"reader-summary\""));
    assert!(body.contains("class=\"reader-section-kicker\">TL;DR"));
    assert!(body.contains("class=\"reader-article-copy\""));
    assert!(!body.contains("summary-box"));
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

#[tokio::test]
async fn legacy_digests_addresses_redirect_permanently_to_briefing() {
    // No DB needed — build a lazy-pool state.
    let db = Db::connect_lazy("postgres://u:p@localhost/none").unwrap();
    let app = router(AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    ));
    for (method, from, to) in [
        ("GET", "/digests", "/briefing"),
        ("POST", "/digests/refresh", "/briefing/refresh"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(from)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // 308 keeps the method, so a bookmarked form post still reaches the new route.
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT, "{from}");
        assert_eq!(response.headers().get("location").unwrap(), to);
    }
}

#[tokio::test]
async fn empty_briefing_explains_why_and_what_is_needed() {
    use rusty_feed_db::models::NewArticle;
    use rusty_feed_db::{articles, embeddings, feeds, subscriptions};

    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let pool = state.db.pool();
    let user = users::find_by_email(pool, &email).await.unwrap().unwrap();
    let get = |app: Router, sid: String| async move {
        let response = app
            .oneshot(
                Request::get("/briefing")
                    .header("cookie", format!("rusty_feed_sid={sid}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        body_string(response).await
    };

    let body = get(app.clone(), sid.clone()).await;
    assert!(body.contains("<h1>Briefing</h1>"));
    assert!(!body.contains("class=\"eyebrow\">Library"));
    assert!(body.contains("Nothing to brief yet."));
    assert!(body.contains("similarity is at least 0.82"));
    assert!(body.contains("href=\"/settings/preferences#briefing-settings\""));
    assert!(body.contains("It is off."));
    assert!(body.contains("None of your feeds has published an article in the last 14 days"));
    assert!(body.contains("action=\"/briefing/refresh\""));
    assert!(!body.contains("/digests"));

    user_preferences::upsert(pool, user.id, &serde_json::json!({ "daily_digest": true }))
        .await
        .unwrap();
    let suffix = uuid::Uuid::now_v7();
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://briefing-empty.example/{suffix}.xml"),
        "Briefing feed",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(pool, user.id, feed.id, None)
        .await
        .unwrap();
    let mut indexed = None;
    for (slug, title) in [
        ("indexed", "Indexed article"),
        ("waiting", "Waiting article"),
    ] {
        let (article, _) = articles::upsert(
            pool,
            &NewArticle {
                feed_id: feed.id,
                url: format!("https://briefing-empty.example/{suffix}/{slug}"),
                comments_url: None,
                guid: None,
                title: title.to_string(),
                author: None,
                content: format!("<p>{title}</p>"),
                excerpt: None,
                image_url: None,
                enclosures: Vec::new(),
                published_at: Some(time::OffsetDateTime::now_utc()),
            },
        )
        .await
        .unwrap();
        indexed.get_or_insert(article);
    }
    let indexed = indexed.unwrap();
    let model: String =
        sqlx::query_scalar("SELECT model FROM instance_embedding_space WHERE singleton")
            .fetch_optional(pool)
            .await
            .unwrap()
            .unwrap_or_else(|| "briefing-empty-test".to_string());
    embeddings::upsert(
        pool,
        indexed.id,
        &vec![0.5_f32; 384],
        &model,
        indexed.content_revision,
    )
    .await
    .unwrap();

    let body = get(app, sid).await;
    assert!(body.contains("It is on."));
    assert!(
        body.contains("1 of 2 articles from your feeds in the last 14 days are indexed."),
        "{body}"
    );
}

fn card_ids(body: &str) -> Vec<String> {
    body.match_indices("id=\"card-")
        .map(|(start, marker)| body[start + marker.len()..start + marker.len() + 36].to_string())
        .collect()
}

/// The unescaped `href` of the link whose markup continues with `marker`.
fn link_before(body: &str, marker: &str) -> Option<String> {
    let end = body.find(marker)?;
    let start = body[..end].rfind("href=\"")? + "href=\"".len();
    Some(body[start..end].replace("&amp;", "&"))
}

fn without_after(url: &str) -> String {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let kept: Vec<_> = query
        .split('&')
        .filter(|pair| !pair.starts_with("after="))
        .collect();
    format!("{path}?{}", kept.join("&"))
}

async fn get_body(app: &Router, sid: &str, url: &str) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::get(url)
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "{url}");
    body_string(resp).await
}

#[tokio::test]
async fn library_cursor_links_show_the_same_pages_as_offset_links() {
    use rusty_feed_db::models::NewArticle;
    use rusty_feed_db::{articles, feeds, read_articles, subscriptions};

    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let pool = state.db.pool();
    let user = users::find_by_email(pool, &email).await.unwrap().unwrap();
    user_preferences::upsert(
        pool,
        user.id,
        &serde_json::json!({"articles_per_page": 5, "show_read_articles": true}),
    )
    .await
    .unwrap();
    let mut feed_ids = Vec::new();
    for name in ["Cursor feed A", "Cursor feed B"] {
        let (feed, _) = feeds::upsert_by_url(
            pool,
            &format!("https://cursor.example/{}.xml", uuid::Uuid::now_v7()),
            name,
            None,
        )
        .await
        .unwrap();
        subscriptions::subscribe(pool, user.id, feed.id, None)
            .await
            .unwrap();
        feed_ids.push(feed.id);
    }
    // Hide every title containing "Hidden" on feed A only.
    sqlx::query("UPDATE user_feeds SET settings = $3 WHERE user_id = $1 AND feed_id = $2")
        .bind(user.id)
        .bind(feed_ids[0])
        .bind(serde_json::json!({"entry_filters": [{
            "name": "hide", "target": "title", "mode": "exclude", "pattern": "Hidden"
        }]}))
        .execute(pool)
        .await
        .unwrap();
    for (feed_index, feed_id) in feed_ids.iter().enumerate() {
        for slot in 0..24_i32 {
            let title = if slot % 4 == 1 {
                format!("Hidden {feed_index}/{slot}")
            } else {
                format!("Visible {feed_index}/{slot}")
            };
            let (article, _) = articles::upsert(
                pool,
                &NewArticle {
                    feed_id: *feed_id,
                    url: format!("https://cursor.example/a/{}", uuid::Uuid::now_v7()),
                    comments_url: None,
                    guid: None,
                    title,
                    author: None,
                    content: format!("Body {feed_index}/{slot}"),
                    excerpt: None,
                    image_url: None,
                    enclosures: Vec::new(),
                    published_at: None,
                },
            )
            .await
            .unwrap();
            sqlx::query(
                "UPDATE articles SET \
                     published_at = CASE WHEN $2 THEN NULL \
                         ELSE timestamptz '2026-02-01 00:00:00+00' - make_interval(hours => $3) END, \
                     created_at = timestamptz '2026-02-01 00:00:00+00' - make_interval(mins => $4) \
                 WHERE id = $1",
            )
            .bind(article.id)
            .bind(slot >= 18)
            .bind(slot / 3)
            .bind(slot % 2)
            .execute(pool)
            .await
            .unwrap();
            if (feed_index as i32 + slot) % 5 == 0 {
                read_articles::mark_read(pool, user.id, article.id)
                    .await
                    .unwrap();
            }
        }
    }

    for start in ["/articles?unread=0", "/articles?unread=1"] {
        let mut url = start.to_string();
        let mut pages = 0;
        let mut seen_cursor_pages = 0;
        loop {
            let body = get_body(&app, &sid, &url).await;
            let ids = card_ids(&body);
            assert!(!ids.is_empty(), "{url}");
            assert!(!body.contains("Hidden 0/"), "entry filter applies on {url}");
            if url.contains("after=") {
                seen_cursor_pages += 1;
                let offset_body = get_body(&app, &sid, &without_after(&url)).await;
                assert_eq!(ids, card_ids(&offset_body), "{url}");
                assert_eq!(
                    link_before(&body, "\" class=\"pager-next\"").map(|next| without_after(&next)),
                    link_before(&offset_body, "\" class=\"pager-next\"")
                        .map(|next| without_after(&next)),
                );
                // Newer/back links are offset links, as before.
                let offset: i64 = url
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("offset="))
                    .unwrap()
                    .parse()
                    .unwrap();
                let prev = link_before(&body, "\">Newer</a>").expect("cursor page has Newer");
                assert_eq!(prev, link_before(&offset_body, "\">Newer</a>").unwrap());
                let expected_prev = if offset - 5 > 0 {
                    format!("{start}&offset={}", offset - 5)
                } else {
                    start.to_string()
                };
                assert_eq!(prev, expected_prev);
                assert!(!prev.contains("after="));

                // Infinite scroll continues from the same cursor.
                let fragment = get_body(&app, &sid, &format!("{url}&fragment=items")).await;
                let offset_fragment = get_body(
                    &app,
                    &sid,
                    &format!("{}&fragment=items", without_after(&url)),
                )
                .await;
                assert_eq!(card_ids(&fragment), ids);
                assert_eq!(card_ids(&offset_fragment), ids);
                assert_eq!(
                    link_before(&fragment, "\" class=\"pager-next\""),
                    link_before(&body, "\" class=\"pager-next\"")
                );
            }
            pages += 1;
            let Some(next) = link_before(&body, "\" class=\"pager-next\"") else {
                break;
            };
            assert!(next.contains("after="), "{next}");
            assert!(next.contains("offset="), "{next}");
            url = next;
        }
        assert!(pages >= 4, "{start} walked {pages} pages");
        assert_eq!(seen_cursor_pages, pages - 1);
    }

    // A malformed cursor falls back to the offset page instead of failing.
    let offset_page = card_ids(&get_body(&app, &sid, "/articles?unread=0&offset=5").await);
    for token in ["not-a-cursor", "AAAA", "fHx8", "MXwyfDM"] {
        let body = get_body(
            &app,
            &sid,
            &format!("/articles?unread=0&offset=5&after={token}"),
        )
        .await;
        assert_eq!(card_ids(&body), offset_page, "after={token}");
    }
}

/// The sidebar unread badge for the feed named `name` (0 when absent).
fn sidebar_unread(body: &str, name: &str) -> i64 {
    let marker = format!("aria-label=\"{name}, ");
    body.find(&marker)
        .and_then(|start| {
            let rest = &body[start + marker.len()..];
            rest.split_once(" unread\"")
                .and_then(|(count, _)| count.parse().ok())
        })
        .unwrap_or(0)
}

async fn post_form(app: &Router, cookie: &str, url: &str, form: String) -> StatusCode {
    app.clone()
        .oneshot(
            Request::post(url)
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn library_unread_counts_are_cached_and_follow_every_read_action() {
    use rusty_feed_cache::keys;
    use rusty_feed_db::models::NewArticle;
    use rusty_feed_db::{api_tokens, articles, feeds, subscriptions};

    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let pool = state.db.pool();
    let user = users::find_by_email(pool, &email).await.unwrap().unwrap();
    let feed_name = format!("Unread cache {}", uuid::Uuid::now_v7());
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://unread-cache.example/{}.xml", uuid::Uuid::now_v7()),
        &feed_name,
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(pool, user.id, feed.id, None)
        .await
        .unwrap();
    let add_article = |index: usize| {
        let feed_id = feed.id;
        async move {
            articles::upsert(
                pool,
                &NewArticle {
                    feed_id,
                    url: format!("https://unread-cache.example/a/{}", uuid::Uuid::now_v7()),
                    comments_url: None,
                    guid: None,
                    title: format!("Unread cache story {index}"),
                    author: None,
                    content: format!("Unread cache body {index}"),
                    excerpt: None,
                    image_url: None,
                    enclosures: Vec::new(),
                    published_at: None,
                },
            )
            .await
            .unwrap()
            .0
            .id
        }
    };
    let mut ids = Vec::new();
    for index in 0..8 {
        ids.push(add_article(index).await);
    }
    let greader_token = format!("unread-greader-{}", uuid::Uuid::now_v7());
    let fever_key = format!(
        "{:x}",
        md5::compute(format!("unread-{}", uuid::Uuid::now_v7()))
    );
    let api_token = format!("unread-api-{}", uuid::Uuid::now_v7());
    for (raw, scopes) in [
        (&greader_token, vec!["greader".to_string()]),
        (&fever_key, vec!["fever".to_string()]),
        (&api_token, Vec::new()),
    ] {
        api_tokens::create(
            pool,
            user.id,
            "Unread cache test",
            &rusty_feed_web::auth::token::hash(raw),
            &scopes,
        )
        .await
        .unwrap();
    }
    let protocol_id = |article_id: uuid::Uuid| async move {
        sqlx::query_scalar::<_, i64>(
            "SELECT greader_id FROM greader_article_ids WHERE article_id = $1",
        )
        .bind(article_id)
        .fetch_one(pool)
        .await
        .unwrap()
    };

    let list = app
        .clone()
        .oneshot(
            Request::get("/articles?unread=0")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = find(&set_cookies(&list), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let cookie = format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}");
    assert_eq!(sidebar_unread(&body_string(list).await, &feed_name), 8);
    let cached_key = keys::unread(user.id);
    assert!(state
        .cache
        .get::<Vec<(uuid::Uuid, i64)>>(&cached_key)
        .await
        .is_some());
    let count = || async {
        sidebar_unread(
            &get_body(&app, &sid, "/articles?unread=0").await,
            &feed_name,
        )
    };

    // New arrivals are not the user's action: the cached count may lag.
    ids.push(add_article(8).await);
    ids.push(add_article(9).await);
    assert_eq!(count().await, 8, "arrivals wait for the cache TTL");

    // Every action of the user's own is reflected on the very next load.
    assert_eq!(
        post_form(
            &app,
            &cookie,
            &format!("/articles/{}/read", ids[0]),
            format!("csrf_token={csrf}")
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(count().await, 9, "web mark read");
    post_form(
        &app,
        &cookie,
        &format!("/articles/{}/unread", ids[0]),
        format!("csrf_token={csrf}"),
    )
    .await;
    assert_eq!(count().await, 10, "web mark unread");

    get_body(&app, &sid, &format!("/articles/{}", ids[1])).await;
    assert_eq!(count().await, 9, "opening an article auto-marks it read");

    let read_tag = format!("user/{}/state/com.google/read", user.id);
    let item = protocol_id(ids[2]).await;
    assert_eq!(
        post_form(
            &app,
            "",
            "/reader/api/0/edit-tag",
            format!("T={greader_token}&i={item}&a={read_tag}")
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(count().await, 8, "GReader edit-tag read");
    post_form(
        &app,
        "",
        "/reader/api/0/edit-tag",
        format!("T={greader_token}&i={item}&r={read_tag}"),
    )
    .await;
    assert_eq!(count().await, 9, "GReader edit-tag unread");

    let item = protocol_id(ids[3]).await;
    assert_eq!(
        post_form(
            &app,
            "",
            "/fever/",
            format!("api_key={fever_key}&mark=item&as=read&id={item}")
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(count().await, 8, "Fever read");
    post_form(
        &app,
        "",
        "/fever/",
        format!("api_key={fever_key}&mark=item&as=unread&id={item}"),
    )
    .await;
    assert_eq!(count().await, 9, "Fever unread");

    for (read, expected) in [(true, 8), (false, 9)] {
        let response = app
            .clone()
            .oneshot(
                Request::post(format!("/api/v1/articles/{}/state", ids[4]))
                    .header("authorization", format!("Bearer {api_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"read":{read}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(count().await, expected, "JSON API read={read}");
    }

    post_form(
        &app,
        &cookie,
        "/articles/read-all",
        format!("csrf_token={csrf}&feed={}", feed.id),
    )
    .await;
    assert_eq!(count().await, 0, "web mark all read");
    post_form(
        &app,
        &cookie,
        &format!("/articles/{}/unread", ids[5]),
        format!("csrf_token={csrf}"),
    )
    .await;
    assert_eq!(count().await, 1);
    assert_eq!(
        post_form(
            &app,
            "",
            "/reader/api/0/mark-all-as-read",
            format!("T={greader_token}")
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(count().await, 0, "GReader mark all as read before now");

    // Counts are personal: a second subscriber of the same feed sees their own.
    let (other_sid, other_email) = register(&app).await;
    let other = users::find_by_email(pool, &other_email)
        .await
        .unwrap()
        .unwrap();
    subscriptions::subscribe(pool, other.id, feed.id, None)
        .await
        .unwrap();
    assert_eq!(
        sidebar_unread(
            &get_body(&app, &other_sid, "/articles?unread=0").await,
            &feed_name
        ),
        10
    );
    assert_eq!(count().await, 0);
    assert_ne!(keys::unread(other.id), cached_key);

    // Unsubscribing (web and JSON API) drops the cached counts too.
    assert_eq!(
        post_form(
            &app,
            &cookie,
            &format!("/feeds/{}/unsubscribe", feed.id),
            format!("csrf_token={csrf}")
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert!(state
        .cache
        .get::<Vec<(uuid::Uuid, i64)>>(&cached_key)
        .await
        .is_none());
    get_body(&app, &other_sid, "/articles?unread=0").await;
    let response = app
        .clone()
        .oneshot(
            Request::delete(format!("/api/v1/feeds/{}", feed.id))
                .header("authorization", format!("Bearer {api_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "already unsubscribed"
    );
    assert!(state
        .cache
        .get::<Vec<(uuid::Uuid, i64)>>(&keys::unread(other.id))
        .await
        .is_some());
}

#[tokio::test]
async fn unread_count_cache_can_be_disabled() {
    use rusty_feed_cache::keys;
    use rusty_feed_db::models::NewArticle;
    use rusty_feed_db::{articles, feeds, subscriptions};

    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    let state = AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings {
            unread_count_cache_ttl: std::time::Duration::ZERO,
            ..AppSettings::default()
        },
        String::new,
    );
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let pool = state.db.pool();
    let user = users::find_by_email(pool, &email).await.unwrap().unwrap();
    let feed_name = format!("Uncached {}", uuid::Uuid::now_v7());
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://uncached.example/{}.xml", uuid::Uuid::now_v7()),
        &feed_name,
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(pool, user.id, feed.id, None)
        .await
        .unwrap();
    for expected in 1..=3 {
        articles::upsert(
            pool,
            &NewArticle {
                feed_id: feed.id,
                url: format!("https://uncached.example/a/{}", uuid::Uuid::now_v7()),
                comments_url: None,
                guid: None,
                title: format!("Uncached story {expected}"),
                author: None,
                content: format!("Uncached body {expected}"),
                excerpt: None,
                image_url: None,
                enclosures: Vec::new(),
                published_at: None,
            },
        )
        .await
        .unwrap();
        let body = get_body(&app, &sid, "/articles?unread=0").await;
        assert_eq!(
            sidebar_unread(&body, &feed_name),
            expected,
            "exact on every load"
        );
        assert!(state
            .cache
            .get::<Vec<(uuid::Uuid, i64)>>(&keys::unread(user.id))
            .await
            .is_none());
    }
}
