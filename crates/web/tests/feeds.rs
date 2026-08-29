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
    assert_eq!(subs.len(), 1);
    let feed = &subs[0];
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
async fn admin_can_set_shared_article_page_policy() {
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
    assert!(body.contains("Content source"));
    assert!(body.contains("Ignore robots.txt for this feed"));

    let response = app
        .oneshot(
            Request::post(format!("/feeds/{}/settings", feed.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "csrf_token={csrf}&content_mode=article_page&selector=article&respect_robots_txt=false&extraction_delay_ms=500&fetch_interval_minutes=30"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response)
        .await
        .contains("Shared source policy saved"));

    let saved = feeds::find_by_id(state.db.pool(), feed.id)
        .await
        .unwrap()
        .unwrap();
    let settings = saved.extraction_settings.unwrap();
    assert_eq!(settings["mode"], "article_page");
    assert_eq!(settings["selector"], "article");
    assert!(!saved.respect_robots_txt);
    assert_eq!(saved.extraction_delay_ms, Some(500));
    assert_eq!(saved.fetch_interval_minutes, 30);
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

/// Minimal application/x-www-form-urlencoded encoding for our test URL values.
fn urlencoding_min(s: &str) -> String {
    s.replace('%', "%25")
        .replace('&', "%26")
        .replace('=', "%3D")
        .replace('?', "%3F")
        .replace(':', "%3A")
        .replace('/', "%2F")
}
