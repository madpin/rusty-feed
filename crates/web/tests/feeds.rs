//! Feed UI integration: add a feed, queue shared ingestion, process it, see
//! articles in the reader, then unsubscribe. Requires `TEST_DATABASE_URL`.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rustss_agents::{Agent, AgentContext, FetchFeedAgent};
use rustss_cache::Cache;
use rustss_db::{
    articles, categories, feed_icons, feeds, jobs, subscriptions, users, Db, NewArticle,
};
use rustss_feeds::SafeFetchClient;
use rustss_web::{router, AppSettings, AppState};
use serde_json::json;

/// RSS with unique item URLs per `run` (article URLs are globally unique, so a
/// stable URL would be skipped as a duplicate on reruns / across tests).
fn rss(run: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <title>Mock Feed</title><link>https://mock.example</link>
  <item><title>Alpha Post</title><link>https://mock.example/{run}/a</link><guid>{run}-a</guid>
    <description>Alpha body</description></item>
  <item><title>Beta Post</title><link>https://mock.example/{run}/b</link><guid>{run}-b</guid>
    <description>Beta body</description></item>
</channel></rss>"#
    )
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

async fn register(app: &Router) -> (String, String) {
    let email = format!("f-{}%40example.com", uuid::Uuid::now_v7());
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
                    "name=&email={email}&password=Str0ng-Test-Pass-9x&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let sid = find(&set_cookies(&resp), "rustss_sid").unwrap().to_string();
    (sid, email.replace("%40", "@"))
}

#[tokio::test]
async fn bookmarklet_discovers_feeds_and_requires_confirmation() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/story"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string(
                    r#"<html><head><link rel="alternate" type="application/rss+xml" title="News feed" href="/feed.xml"></head><body>Story</body></html>"#,
                ),
        )
        .mount(&server)
        .await;
    let state = AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    )
    .with_fetch(SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap());
    let app = router(state);
    let (sid, _) = register(&app).await;

    let page_url = format!("{}/story", server.uri());
    let encoded: String = url::form_urlencoded::byte_serialize(page_url.as_bytes()).collect();
    let response = app
        .oneshot(
            Request::get(format!("/bookmarklet/subscribe?url={encoded}"))
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Choose a feed to subscribe"));
    assert!(body.contains("News feed"));
    assert!(body.contains(&format!("{}/feed.xml", server.uri())));
    assert!(body.contains("action=\"/feeds\""));
    assert!(!body.contains("Subscribed to"));
}

#[tokio::test]
async fn add_feed_ingests_and_shows_articles() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();

    let run = uuid::Uuid::now_v7().to_string();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&run)))
        .mount(&server)
        .await;

    // App state with an allow-private fetch client so it can reach wiremock.
    let state = AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    )
    .with_fetch(SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap());
    let app = router(state.clone());
    let (sid, email) = register(&app).await;

    // CSRF from the feeds page.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/feeds")
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

    // Add the feed.
    let feed_url = format!("{}/feed.xml?u={}", server.uri(), uuid::Uuid::now_v7());
    let resp = app
        .clone()
        .oneshot(
            Request::post("/feeds")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "url={}&csrf_token={csrf}",
                    urlencoding_min(&feed_url)
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_string(resp).await.contains("Refresh queued"));

    // The user is subscribed and one shared fetch job is queued.
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    let subs = feeds::list_for_user(state.db.pool(), user.id)
        .await
        .unwrap();
    let feed = subs
        .iter()
        .find(|feed| feed.url == feed_url)
        .expect("newly added feed must be among the user's subscriptions");
    assert!(
        subscriptions::is_subscribed(state.db.pool(), user.id, feed.id)
            .await
            .unwrap()
    );
    assert_eq!(
        jobs::count_active_by_dedup(state.db.pool(), &format!("fetch_feed:{}", feed.id))
            .await
            .unwrap(),
        1
    );
    FetchFeedAgent
        .run(
            &AgentContext::new(state.db.clone(), state.fetch.clone()),
            json!({ "feed_id": feed.id }),
        )
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::get("/feeds")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let feeds_page = body_string(response).await;
    assert!(feeds_page.contains("Last updated: just now"));
    assert!(feeds_page.contains("Last successful update:"));

    // They show up in the reader.
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
    let body = body_string(resp).await;
    assert!(body.contains("Alpha Post"));
    assert!(body.contains("Beta Post"));

    // Unsubscribe.
    let resp = app
        .oneshot(
            Request::post(format!("/feeds/{}/unsubscribe", feed.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(
        !subscriptions::is_subscribed(state.db.pool(), user.id, feed.id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn subscriber_owns_content_choice_while_admin_controls_shared_acquisition() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();
    let state = AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    );
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = $1")
        .bind(user.id)
        .execute(state.db.pool())
        .await
        .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!("https://example.com/{}.xml", uuid::Uuid::now_v7()),
        "Example",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(state.db.pool(), user.id, feed.id, None)
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/feeds/{}/settings", feed.id))
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let csrf = find(&set_cookies(&response), "rustss_csrf")
        .unwrap()
        .to_string();
    let body = body_string(response).await;
    assert!(body.contains("Your stored content"));
    assert!(body.contains("Ignore robots.txt for this feed"));

    let response = app
        .oneshot(
            Request::post(format!("/feeds/{}/settings", feed.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "csrf_token={csrf}&content_mode=article_page&content_lifetime=infinite&youtube_player_mode=links_only&enclosure_delivery_mode=proxy&enclosure_load_mode=links-only&selector=article&respect_robots_txt=false&extraction_delay_ms=500&fetch_interval_minutes=30"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response)
        .await
        .contains("Your content preference was saved"));

    let saved = feeds::find_by_id(state.db.pool(), feed.id)
        .await
        .unwrap()
        .unwrap();
    let settings = saved.extraction_settings.unwrap();
    assert_eq!(settings["mode"], "rss");
    assert_eq!(settings["selector"], "article");
    assert!(!saved.respect_robots_txt);
    assert_eq!(saved.extraction_delay_ms, Some(500));
    assert_eq!(saved.fetch_interval_minutes, 30);
    let personal = subscriptions::content_settings_for_user(state.db.pool(), user.id, feed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(personal.mode.as_str(), "article_page");
    assert!(personal.lifetime.days().is_none());
    assert_eq!(
        subscriptions::youtube_mode_for_user(state.db.pool(), user.id, feed.id)
            .await
            .unwrap(),
        Some(rustss_core::youtube::SubscriptionYouTubeMode::LinksOnly)
    );
    let subscription = subscriptions::find(state.db.pool(), user.id, feed.id)
        .await
        .unwrap()
        .unwrap();
    let enclosure_settings = rustss_core::feed_content::SubscriptionEnclosureSettings::from_json(
        subscription.settings.as_ref(),
    );
    assert_eq!(
        enclosure_settings.delivery,
        rustss_core::feed_content::SubscriptionEnclosureDelivery::Proxy
    );
    assert_eq!(
        enclosure_settings.load,
        rustss_core::feed_content::SubscriptionEnclosureLoad::LinksOnly
    );
    assert_eq!(
        jobs::count_active_by_dedup(state.db.pool(), &format!("fetch_feed:{}", feed.id))
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn users_organize_feeds_into_one_folder_and_multiple_tags() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();
    let state = AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    );
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!(
            "https://organization-ui.example/{}.xml",
            uuid::Uuid::now_v7()
        ),
        "Organized feed",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(state.db.pool(), user.id, feed.id, None)
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::get("/feeds")
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
        .clone()
        .oneshot(
            Request::post("/feeds/categories")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!("csrf_token={csrf}&name=Work")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let work = categories::list_for_user(state.db.pool(), user.id)
        .await
        .unwrap()
        .into_iter()
        .find(|category| category.name == "Work")
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::post("/feeds/categories")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "csrf_token={csrf}&name=Research&parent_id={}",
                    work.id
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let research = categories::list_for_user(state.db.pool(), user.id)
        .await
        .unwrap()
        .into_iter()
        .find(|category| category.name == "Research")
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/feeds/{}/organization", feed.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "csrf_token={csrf}&tags=Rust%2C+Work&folder_id={}",
                    research.id
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Feed organization saved."));
    assert!(body.contains("Folder: Work / Research"));
    assert!(body.contains("#rust"));
    let mut organizations = subscriptions::list_organization(state.db.pool(), user.id)
        .await
        .unwrap();
    let organization = organizations.pop().unwrap();
    assert_eq!(
        organization.tags,
        vec!["rust".to_string(), "work".to_string()]
    );
    assert_eq!(organization.category_id, Some(research.id));

    articles::upsert(
        state.db.pool(),
        &NewArticle {
            feed_id: feed.id,
            url: format!(
                "https://organization-ui.example/article/{}",
                uuid::Uuid::now_v7()
            ),
            comments_url: None,
            guid: None,
            title: "Folder article".to_string(),
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
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/articles?category={}", work.id))
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_string(response).await;
    assert!(body.contains("Folder: Work"));
    assert!(body.contains("Folder article"));
    let other_user = users::create(
        state.db.pool(),
        &format!("foreign-folder-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let foreign = categories::create(state.db.pool(), other_user.id, "Foreign", None)
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/articles?category={}", foreign.id))
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let icon = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 1];
    feed_icons::record_success(
        state.db.pool(),
        feed.id,
        "https://organization-ui.example/icon.png",
        "image/png",
        &icon,
    )
    .await
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::get("/feeds")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_string(response).await;
    assert!(body.contains(&format!("/feeds/{}/icon", feed.id)));
    assert!(!body.contains("https://organization-ui.example/icon.png"));
    let response = app
        .oneshot(
            Request::get(format!("/feeds/{}/icon", feed.id))
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        icon
    );
}

#[tokio::test]
async fn bulk_feed_actions_are_scoped_and_preserve_personal_organization() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();
    let state = AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    );
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE users SET role = 'user' WHERE id = $1")
        .bind(user.id)
        .execute(state.db.pool())
        .await
        .unwrap();
    let folder = categories::create(state.db.pool(), user.id, "Bulk destination", None)
        .await
        .unwrap();
    let (first, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!("https://bulk-ui.example/{}/first.xml", uuid::Uuid::now_v7()),
        "Bulk first",
        None,
    )
    .await
    .unwrap();
    let (second, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!(
            "https://bulk-ui.example/{}/second.xml",
            uuid::Uuid::now_v7()
        ),
        "Bulk second",
        None,
    )
    .await
    .unwrap();
    let (foreign, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!(
            "https://bulk-ui.example/{}/foreign.xml",
            uuid::Uuid::now_v7()
        ),
        "Bulk foreign",
        None,
    )
    .await
    .unwrap();
    for feed in [&first, &second] {
        subscriptions::subscribe(state.db.pool(), user.id, feed.id, None)
            .await
            .unwrap();
    }
    let other = users::create(
        state.db.pool(),
        &format!("bulk-foreign-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(state.db.pool(), other.id, foreign.id, None)
        .await
        .unwrap();
    subscriptions::update_organization(
        state.db.pool(),
        user.id,
        first.id,
        &["kept".to_string()],
        None,
    )
    .await
    .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::get("/feeds")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let csrf = find(&set_cookies(&response), "rustss_csrf")
        .unwrap()
        .to_string();
    let page = body_string(response).await;
    assert!(page.contains("bulk-feed-actions"));
    assert!(page.contains("data-feed-drag"));
    assert!(page.contains("data-feed-drop"));
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/feeds/categories/{}/collapse", folder.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!("csrf_token={csrf}&collapsed=true")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let saved_folder = categories::list_for_user(state.db.pool(), user.id)
        .await
        .unwrap()
        .into_iter()
        .find(|category| category.id == folder.id)
        .unwrap();
    assert!(saved_folder.collapsed);
    let response = app
        .clone()
        .oneshot(
            Request::post("/feeds/refresh-everything")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = $1")
        .bind(user.id)
        .execute(state.db.pool())
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::get("/feeds")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response)
        .await
        .contains("Reset schedules and refresh every source"));

    let response = app
        .clone()
        .oneshot(
            Request::post("/feeds/bulk-move")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "csrf_token={csrf}&feed_id={}&feed_id={}&folder_id={}",
                    first.id, second.id, folder.id
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response).await.contains("Moved 2 feed(s)"));

    let response = app
        .clone()
        .oneshot(
            Request::post("/feeds/bulk-tags")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "csrf_token={csrf}&feed_id={}&feed_id={}&tag_operation=add&tags=project%2Cpriority",
                    first.id, second.id
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response)
        .await
        .contains("Updated tags for 2 feed(s)"));
    let organizations = subscriptions::list_organization(state.db.pool(), user.id)
        .await
        .unwrap();
    let first_organization = organizations
        .iter()
        .find(|organization| organization.feed_id == first.id)
        .unwrap();
    let second_organization = organizations
        .iter()
        .find(|organization| organization.feed_id == second.id)
        .unwrap();
    assert_eq!(first_organization.category_id, Some(folder.id));
    assert_eq!(second_organization.category_id, Some(folder.id));
    assert_eq!(
        first_organization.tags,
        vec![
            "kept".to_string(),
            "project".to_string(),
            "priority".to_string()
        ]
    );

    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/feeds/{}/folder", first.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!("csrf_token={csrf}&folder_id=")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let first_organization = subscriptions::list_organization(state.db.pool(), user.id)
        .await
        .unwrap()
        .into_iter()
        .find(|organization| organization.feed_id == first.id)
        .unwrap();
    assert_eq!(first_organization.category_id, None);
    assert_eq!(
        first_organization.tags,
        vec![
            "kept".to_string(),
            "project".to_string(),
            "priority".to_string()
        ]
    );

    let response = app
        .clone()
        .oneshot(
            Request::post("/feeds/refresh-selected")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "csrf_token={csrf}&feed_id={}&feed_id={}",
                    first.id, second.id
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response)
        .await
        .contains("Queued 2 feed refresh(es)"));
    for feed_id in [first.id, second.id] {
        assert_eq!(
            jobs::count_active_by_dedup(state.db.pool(), &format!("fetch_feed:{feed_id}"))
                .await
                .unwrap(),
            1
        );
    }

    let response = app
        .oneshot(
            Request::post("/feeds/refresh-selected")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "csrf_token={csrf}&feed_id={}",
                    foreign.id
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn opml_preview_import_url_and_export_round_trip() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();
    let state = AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    )
    .with_fetch(SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap());
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let feeds_page = app
        .clone()
        .oneshot(
            Request::get("/feeds")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = find(&set_cookies(&feeds_page), "rustss_csrf")
        .unwrap()
        .to_string();
    let run = uuid::Uuid::now_v7();
    let empty_boundary = format!("rustss-empty-{run}");
    let empty_multipart = format!(
        "--{empty_boundary}\r\nContent-Disposition: form-data; name=\"csrf_token\"\r\n\r\n{csrf}\r\n\
         --{empty_boundary}\r\nContent-Disposition: form-data; name=\"opml_url\"\r\n\r\n\r\n\
         --{empty_boundary}\r\nContent-Disposition: form-data; name=\"opml_file\"; filename=\"\"\r\nContent-Type: application/octet-stream\r\n\r\n\r\n\
         --{empty_boundary}--\r\n"
    );
    let empty_preview = app
        .clone()
        .oneshot(
            Request::post("/feeds/opml/preview")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={empty_boundary}"),
                )
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(empty_multipart))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(empty_preview.status(), StatusCode::OK);
    let empty_preview = body_string(empty_preview).await;
    assert!(empty_preview.contains("Import needs attention"));
    assert!(empty_preview.contains("Choose either one OPML file or one OPML URL"));

    let first_url = format!("https://opml-{run}.example/one.xml");
    let second_url = format!("https://opml-{run}.example/two.xml");
    let document = format!(
        r#"<?xml version="1.0"?><opml version="2.0"><head><title>Portable library</title></head><body>
        <outline text="Work"><outline text="News"><outline text="One" xmlUrl="{first_url}" htmlUrl="https://one.example/"/></outline></outline>
        <outline text="Personal"><outline text="News"><outline text="Two" xmlUrl="{second_url}"/></outline></outline>
        <outline text="Duplicate" xmlUrl="{first_url}"/>
        </body></opml>"#
    );
    let boundary = format!("rustss-{run}");
    let multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"csrf_token\"\r\n\r\n{csrf}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"opml_url\"\r\n\r\n\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"opml_file\"; filename=\"subscriptions.opml\"\r\nContent-Type: text/x-opml\r\n\r\n{document}\r\n\
         --{boundary}--\r\n"
    );
    let preview = app
        .clone()
        .oneshot(
            Request::post("/feeds/opml/preview")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(multipart))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::OK);
    let preview = body_string(preview).await;
    assert!(preview.contains("Preview Portable library"));
    assert!(preview.contains("2 subscription(s) ready"));
    assert!(preview.contains("1 invalid or duplicate item(s) will be skipped"));
    assert!(preview.contains("Work / News"));
    assert!(preview.contains("Personal / News"));
    let entries = attribute_values(&preview, "name=\"entry\" value=\"");
    assert_eq!(entries.len(), 2);

    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("csrf_token", &csrf);
    serializer.append_pair("existing_policy", "keep");
    for entry in &entries {
        serializer.append_pair("entry", entry);
    }
    let imported = app
        .clone()
        .oneshot(
            Request::post("/feeds/opml/import")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(serializer.finish()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(imported.status(), StatusCode::OK);
    let imported = body_string(imported).await;
    assert!(imported.contains("Import complete"));
    assert!(imported.contains("New subscriptions"));

    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    let subscribed_feeds = feeds::list_for_user(state.db.pool(), user.id)
        .await
        .unwrap();
    assert!(subscribed_feeds.iter().any(|feed| feed.url == first_url));
    assert!(subscribed_feeds.iter().any(|feed| feed.url == second_url));
    let category_rows = categories::list_for_user(state.db.pool(), user.id)
        .await
        .unwrap();
    let news_count = category_rows
        .iter()
        .filter(|category| category.name == "News")
        .count();
    assert_eq!(
        news_count, 2,
        "same-named folders under different parents must survive"
    );

    let exported = app
        .clone()
        .oneshot(
            Request::get("/feeds/opml/export")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(exported.status(), StatusCode::OK);
    assert_eq!(
        exported.headers().get("content-type").unwrap(),
        "text/x-opml; charset=utf-8"
    );
    let exported = body_string(exported).await;
    let round_trip = rustss_feeds::parse_opml(exported.as_bytes(), 2_000).unwrap();
    assert!(round_trip.subscriptions.len() >= 2);
    assert!(round_trip
        .subscriptions
        .iter()
        .any(|subscription| subscription.category_path == ["Work", "News"]));
    assert!(round_trip
        .subscriptions
        .iter()
        .any(|subscription| subscription.category_path == ["Personal", "News"]));

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/subscriptions.opml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(document))
        .mount(&server)
        .await;
    let boundary = format!("rustss-url-{run}");
    let multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"csrf_token\"\r\n\r\n{csrf}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"opml_url\"\r\n\r\n{}/subscriptions.opml\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"opml_file\"; filename=\"\"\r\nContent-Type: application/octet-stream\r\n\r\n\r\n\
         --{boundary}--\r\n",
        server.uri()
    );
    let url_preview = app
        .clone()
        .oneshot(
            Request::post("/feeds/opml/preview")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(multipart))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(url_preview.status(), StatusCode::OK);
    let url_preview = body_string(url_preview).await;
    assert!(url_preview.contains("2 subscription(s) ready"));
    assert!(url_preview.contains("0 new · 2 already subscribed"));
    assert!(url_preview.contains("Already subscribed"));

    let feed_ids: Vec<_> = feeds::list_for_user(state.db.pool(), user.id)
        .await
        .unwrap()
        .into_iter()
        .filter(|feed| feed.url == first_url || feed.url == second_url)
        .map(|feed| feed.id)
        .collect();
    assert_eq!(feed_ids.len(), 2);
    sqlx::query("DELETE FROM job_queue WHERE dedup_key = ANY($1)")
        .bind(
            feed_ids
                .iter()
                .map(|feed_id| format!("fetch_feed:{feed_id}"))
                .collect::<Vec<_>>(),
        )
        .execute(state.db.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user.id)
        .execute(state.db.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM feeds WHERE id = ANY($1)")
        .bind(&feed_ids)
        .execute(state.db.pool())
        .await
        .unwrap();
}

fn attribute_values(document: &str, marker: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut remainder = document;
    while let Some(index) = remainder.find(marker) {
        remainder = &remainder[index + marker.len()..];
        let Some(end) = remainder.find('"') else {
            break;
        };
        values.push(remainder[..end].to_string());
        remainder = &remainder[end + 1..];
    }
    values
}

/// Minimal application/x-www-form-urlencoded encoding for our test URL values.
fn urlencoding_min(s: &str) -> String {
    s.replace('%', "%25")
        .replace('&', "%26")
        .replace('=', "%3D")
        .replace('?', "%3F")
        .replace(':', "%3A")
        .replace('/', "%2F")
}
