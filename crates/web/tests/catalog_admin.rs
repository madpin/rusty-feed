use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

use rusty_feed_cache::Cache;
use rusty_feed_core::feed_content::{ContentLifetime, SubscriptionContentMode};
use rusty_feed_db::{
    agent_runs, articles, catalog, feed_icons, feeds, jobs, subscriptions, users, Db, NewArticle,
};
use rusty_feed_web::{router, AppSettings, AppState};
use serde_json::json;

fn set_cookies(response: &Response<Body>) -> Vec<(String, String)> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next()?.split_once('='))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect()
}

fn cookie<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, value)| value.as_str())
}

async fn body_string(response: Response<Body>) -> String {
    String::from_utf8_lossy(&response.into_body().collect().await.unwrap().to_bytes()).to_string()
}

async fn register(app: &Router) -> (String, String) {
    let email = format!("catalog-{}%40example.com", uuid::Uuid::now_v7());
    let response = app
        .clone()
        .oneshot(Request::get("/register").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let csrf = cookie(&set_cookies(&response), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let response = app
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
    (
        cookie(&set_cookies(&response), "rusty_feed_sid")
            .unwrap()
            .to_string(),
        email.replace("%40", "@"),
    )
}

#[tokio::test]
async fn users_browse_select_catalog_and_admin_manages_instance() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let state = AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    );
    let app = router(state.clone());
    let response = app
        .clone()
        .oneshot(Request::get("/admin/logs").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let (sid, email) = register(&app).await;
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!(
            "https://catalog-ui.example/{}/feed.xml",
            uuid::Uuid::now_v7()
        ),
        "Catalog UI source",
        None,
    )
    .await
    .unwrap();
    let catalog_entry = catalog::upsert(
        state.db.pool(),
        feed.id,
        catalog::CatalogDraft {
            title: "Catalog UI source",
            description: Some("A test source for multi-select subscription."),
            category: "Testing",
            country: "Global",
            language: "en",
            featured: true,
            sort_order: 1,
            published: true,
        },
    )
    .await
    .unwrap();
    let (second_feed, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!(
            "https://catalog-ui.example/{}/second.xml",
            uuid::Uuid::now_v7()
        ),
        "Second catalog UI source",
        None,
    )
    .await
    .unwrap();
    let second_catalog_entry = catalog::upsert(
        state.db.pool(),
        second_feed.id,
        catalog::CatalogDraft {
            title: "Second catalog UI source",
            description: None,
            category: "Testing",
            country: "Global",
            language: "en",
            featured: false,
            sort_order: 2,
            published: true,
        },
    )
    .await
    .unwrap();
    let rust_feed = feeds::find_by_url(state.db.pool(), "https://blog.rust-lang.org/feed.xml")
        .await
        .unwrap()
        .unwrap();
    let icon = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0];
    feed_icons::record_success(
        state.db.pool(),
        rust_feed.id,
        "https://blog.rust-lang.org/icon.png",
        "image/png",
        &icon,
    )
    .await
    .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::get("/catalog")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let csrf = cookie(&set_cookies(&response), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let page = body_string(response).await;
    assert!(page.contains("Discover feeds"));
    assert!(page.contains("Rust Blog"));
    assert!(page.contains(&format!("/feeds/{}/icon", rust_feed.id)));
    assert!(page.contains("Go Blog"));
    assert!(page.contains("Al Jazeera English"));
    assert!(page.contains("Public article page"));
    assert!(page.contains("Recommended"));
    assert!(page.contains("Catalog UI source"));
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/feeds/{}/icon", rust_feed.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
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

    let response = app
        .clone()
        .oneshot(
            Request::post("/catalog/subscribe")
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!(
                    "csrf_token={csrf}&feed_id={}&feed_id={}&content_mode.{}=article_page&content_mode.{}=rss",
                    feed.id, second_feed.id, feed.id, second_feed.id
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response).await.contains("Subscribed to 2 feed"));
    assert!(
        subscriptions::is_subscribed(state.db.pool(), user.id, feed.id)
            .await
            .unwrap()
    );
    assert!(
        subscriptions::is_subscribed(state.db.pool(), user.id, second_feed.id)
            .await
            .unwrap()
    );
    assert_eq!(
        subscriptions::content_settings_for_user(state.db.pool(), user.id, feed.id)
            .await
            .unwrap()
            .unwrap()
            .mode,
        SubscriptionContentMode::ArticlePage
    );
    assert_eq!(
        subscriptions::content_settings_for_user(state.db.pool(), user.id, second_feed.id)
            .await
            .unwrap()
            .unwrap()
            .mode,
        SubscriptionContentMode::Rss
    );

    let response = app
        .clone()
        .oneshot(
            Request::get("/admin")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .clone()
        .oneshot(
            Request::get("/admin/logs")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    users::set_role(state.db.pool(), user.id, "admin")
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::get("/admin")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Instance overview"));
    assert!(body.contains("Installed app"));
    assert!(body.contains("/manifest.webmanifest"));
    feeds::record_fetch_failure(state.db.pool(), feed.id, feeds::FetchFailure::Network)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO feed_error_log (id, feed_id, error_type, error_message) \
         VALUES ($1, $2, 'legacy', 'RAW_FEED_LOG_SENTINEL')",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(feed.id)
    .execute(state.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agent_runs (id, agent_name, status, trigger, completed_at, duration_ms, stats, error_message) \
         VALUES ($1, 'fetch_feed', 'failed', 'scheduler', now(), 42, $2, 'RAW_RUN_LOG_SENTINEL')",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(json!({ "created": 2, "reason": "RAW_STATS_SENTINEL" }))
    .execute(state.db.pool())
    .await
    .unwrap();
    let worker_id = format!("admin-test-worker-{}", uuid::Uuid::now_v7());
    jobs::register_worker(state.db.pool(), &worker_id, 3, 1, true)
        .await
        .unwrap();
    let queue_kind = format!("admin_queue_test_{}", uuid::Uuid::now_v7());
    jobs::enqueue(state.db.pool(), &queue_kind, json!({}), None, None, 2)
        .await
        .unwrap();
    let failed = jobs::claim_next(
        state.db.pool(),
        &format!("{worker_id}#1"),
        std::slice::from_ref(&queue_kind),
    )
    .await
    .unwrap()
    .unwrap();
    jobs::fail(
        state.db.pool(),
        &failed,
        "summary provider returned HTTP 401",
        time::Duration::seconds(1),
    )
    .await
    .unwrap();
    let safe_run = agent_runs::start(state.db.pool(), "summarize_article", "interactive")
        .await
        .unwrap();
    agent_runs::finish(
        state.db.pool(),
        safe_run,
        "failed",
        41,
        None,
        Some("summary provider returned HTTP 401"),
    )
    .await
    .unwrap();
    jobs::enqueue(state.db.pool(), &queue_kind, json!({}), None, None, 2)
        .await
        .unwrap();
    let finished = jobs::claim_next(
        state.db.pool(),
        &format!("{worker_id}#0"),
        std::slice::from_ref(&queue_kind),
    )
    .await
    .unwrap()
    .unwrap();
    jobs::complete(state.db.pool(), finished.id).await.unwrap();
    jobs::enqueue(state.db.pool(), &queue_kind, json!({}), None, None, 2)
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::get("/admin/logs")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let logs = body_string(response).await;
    assert!(logs.contains("Background work &amp; logs"));
    assert!(logs.contains("Workers online"));
    assert!(logs.contains("Task slots"));
    assert!(logs.contains("Waiting and running tasks"));
    assert!(logs.contains("Tasks by type"));
    assert!(logs.contains("Just finished"));
    assert!(logs.contains(&worker_id));
    assert!(logs.contains(&queue_kind));
    assert!(logs.contains("Network request failed"));
    assert!(logs.contains("fetch_feed"));
    assert!(logs.contains("created: 2"));
    assert!(logs.contains("summary provider returned HTTP 401"));
    assert!(logs.contains("Priority slots"));
    assert!(logs.contains("Sanitized error"));
    assert!(!logs.contains("RAW_FEED_LOG_SENTINEL"));
    assert!(!logs.contains("RAW_RUN_LOG_SENTINEL"));
    assert!(!logs.contains("RAW_STATS_SENTINEL"));
    sqlx::query("DELETE FROM job_queue WHERE kind = $1")
        .bind(&queue_kind)
        .execute(state.db.pool())
        .await
        .unwrap();
    jobs::unregister_worker(state.db.pool(), &worker_id)
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::get("/admin/catalog")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = cookie(&set_cookies(&response), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    assert!(body_string(response)
        .await
        .contains("Add a feed to Discover"));
    let admin_catalog_url = format!(
        "https://admin-catalog.example/{}/feed.xml",
        uuid::Uuid::now_v7()
    );
    let response = app
        .clone()
        .oneshot(
            Request::post("/admin/catalog")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"))
                .body(Body::from(format!(
                    "csrf_token={csrf}&url={admin_catalog_url}&title=Admin+catalog+source&description=Editable+catalog+entry&category=Testing&country=Global&language=en&featured=true&sort_order=9"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response)
        .await
        .contains("Catalog entry saved and its first refresh was queued."));

    let (unsubscribed, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!(
            "https://admin-feed.example/{}/feed.xml",
            uuid::Uuid::now_v7()
        ),
        "Admin inventory source",
        None,
    )
    .await
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/feeds/{}/settings", unsubscribed.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response)
        .await
        .contains("Shared fetch controls"));

    let member = users::create(
        state.db.pool(),
        &format!("member-{}@example.com", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::get("/admin/users")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = cookie(&set_cookies(&response), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/admin/users/{}/role", user.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!("csrf_token={csrf}&role=user")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response)
        .await
        .contains("cannot demote your own administrator"));
    assert_eq!(
        users::find_by_id(state.db.pool(), user.id)
            .await
            .unwrap()
            .unwrap()
            .role,
        "admin"
    );

    let response = app
        .oneshot(
            Request::post(format!("/admin/users/{}/role", member.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!("csrf_token={csrf}&role=admin")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        users::find_by_id(state.db.pool(), member.id)
            .await
            .unwrap()
            .unwrap()
            .role,
        "admin"
    );
    for entry in [catalog_entry, second_catalog_entry] {
        assert!(catalog::update(
            state.db.pool(),
            entry.id,
            catalog::CatalogDraft {
                title: "Test catalog source",
                description: None,
                category: "Testing",
                country: "Global",
                language: "en",
                featured: false,
                sort_order: 0,
                published: false,
            },
        )
        .await
        .unwrap());
    }
}

#[tokio::test]
async fn catalog_preview_is_shared_safe_and_queues_one_probe() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let state = AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    );
    let app = router(state.clone());
    let (feed, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!(
            "https://catalog-preview.example/{}/feed.xml",
            uuid::Uuid::now_v7()
        ),
        "Catalog preview source",
        None,
    )
    .await
    .unwrap();
    catalog::upsert(
        state.db.pool(),
        feed.id,
        catalog::CatalogDraft {
            title: "Catalog preview source",
            description: Some("Preview description"),
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
    let mut articles_created = Vec::new();
    let article_run = uuid::Uuid::now_v7();
    for (index, content) in [
        "<p>RSS preview body one</p>",
        "<p>RSS preview body two</p>",
        "<p>RSS preview body three</p>",
        "<p>RSS preview body four</p>",
        "<p>RSS preview body five</p>",
        "<p>RSS preview body six</p>",
        "<p>RSS preview <strong>body seven</strong></p><img src=\"https://catalog-preview.example/leak.jpg\" alt=\"Remote image\"><script>CATALOG_SCRIPT_SENTINEL</script>",
    ]
    .iter()
    .enumerate()
    {
        let (article, _) = articles::upsert(
            state.db.pool(),
            &NewArticle {
                feed_id: feed.id,
                url: format!("https://catalog-preview.example/{article_run}/{index}"),
                comments_url: None,
                guid: Some(uuid::Uuid::now_v7().to_string()),
                title: format!("Preview article {index}"),
                author: None,
                content: (*content).to_string(),
                excerpt: None,
                image_url: None,
                enclosures: Vec::new(),
                published_at: None,
            },
        )
        .await
        .unwrap();
        articles_created.push(article);
    }
    articles::set_extracted_content(
        state.db.pool(),
        articles_created[2].id,
        articles_created[2].content_revision,
        "CATALOG_PRIVATE_PAGE_SENTINEL",
        None,
        ContentLifetime::Days(30),
    )
    .await
    .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/catalog/{}/preview", uuid::Uuid::now_v7()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let (sid, email) = register(&app).await;
    let (unpublished_feed, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!(
            "https://catalog-preview.example/{}/unpublished.xml",
            uuid::Uuid::now_v7()
        ),
        "Unpublished catalog preview source",
        None,
    )
    .await
    .unwrap();
    catalog::upsert(
        state.db.pool(),
        unpublished_feed.id,
        catalog::CatalogDraft {
            title: "Unpublished catalog preview source",
            description: None,
            category: "Testing",
            country: "Global",
            language: "en",
            featured: false,
            sort_order: 0,
            published: false,
        },
    )
    .await
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/catalog/{}/preview", unpublished_feed.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !subscriptions::is_subscribed(state.db.pool(), user.id, feed.id)
            .await
            .unwrap()
    );
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/catalog/{}/preview", feed.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let csrf = cookie(&set_cookies(&response), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let page = body_string(response).await;
    assert!(page.contains("RSS preview <strong>body seven</strong>"));
    assert_eq!(page.matches("class=\"preview-article\"").count(), 3);
    assert!(page.contains("Load more articles"));
    assert!(page.contains("articles=6"));
    assert!(page.contains("RSS preview"));
    assert!(page.contains("/preview?view=processed"));
    assert!(page.contains("aria-current=\"page\""));
    assert!(page.contains("Analyze up to 3 articles"));
    assert!(page.contains("Add to my feeds"));
    assert!(!page.contains("leak.jpg"));
    assert!(!page.contains("CATALOG_SCRIPT_SENTINEL"));
    assert!(!page.contains("CATALOG_PRIVATE_PAGE_SENTINEL"));

    let expanded = app
        .clone()
        .oneshot(
            Request::get(format!("/catalog/{}/preview?articles=6", feed.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(expanded.status(), StatusCode::OK);
    let expanded = body_string(expanded).await;
    assert_eq!(expanded.matches("class=\"preview-article\"").count(), 6);
    assert!(expanded.contains("Showing 6 recent articles"));
    assert!(expanded.contains("articles=9"));

    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/catalog/{}/preview/analyze", feed.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from("csrf_token=wrong"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::post(format!(
                    "/catalog/{}/preview/analyze?view=processed",
                    feed.id
                ))
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
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()["location"],
            format!(
                "/catalog/{}/preview?view=processed&articles=3&analysis=queued",
                feed.id
            )
        );
    }
    assert_eq!(
        jobs::count_active_by_dedup(
            state.db.pool(),
            &format!("catalog_content_probe:{}", feed.id),
        )
        .await
        .unwrap(),
        1
    );

    let candidates = catalog::content_probe_candidates(state.db.pool(), feed.id)
        .await
        .unwrap();
    let samples = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| catalog::ContentProbeWrite {
            article_id: candidate.id,
            rss_body_chars: candidate.rss_body_chars,
            processed_body_chars: [Some(600), None, None][index],
            status: [
                catalog::ContentProbeStatus::Full,
                catalog::ContentProbeStatus::Http402,
                catalog::ContentProbeStatus::Http451,
            ][index],
        })
        .collect::<Vec<_>>();
    catalog::replace_content_probe(state.db.pool(), feed.id, &candidates, &samples)
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/catalog/{}/preview?view=processed", feed.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let processed = body_string(response).await;
    assert!(processed.contains("Processed availability"));
    assert!(processed.contains("Readable public-body average"));
    assert!(processed.contains("Payment required (402)"));
    assert!(processed.contains("Unavailable for legal reasons (451)"));
    assert!(processed.contains("Processed preview is an availability report"));
    assert!(!processed.contains("RSS preview body one"));
    assert!(!processed.contains("CATALOG_PRIVATE_PAGE_SENTINEL"));
    assert!(!processed.contains("Analyze up to 3 articles"));
    assert!(
        !subscriptions::is_subscribed(state.db.pool(), user.id, feed.id)
            .await
            .unwrap()
    );
}
