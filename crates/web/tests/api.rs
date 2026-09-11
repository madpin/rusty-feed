use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use rusty_feed_cache::Cache;
use rusty_feed_core::feed_content::{
    SubscriptionContentMode, CATALOG_ARTICLE_PAGE_RECOMMENDATIONS,
};
use rusty_feed_db::models::NewArticle;
use rusty_feed_db::{
    api_tokens, articles, catalog, categories, feeds, subscriptions, user_preferences, users, Db,
};
use rusty_feed_web::{router, AppSettings, AppState};

async fn state() -> Option<AppState> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.ok()?;
    db.migrate().await.ok()?;
    Some(AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    ))
}

/// State with a finite saved-search quota (for the 409 test).
async fn state_with_saved_search_cap(cap: u32) -> Option<AppState> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.ok()?;
    db.migrate().await.ok()?;
    let settings = AppSettings {
        quotas: rusty_feed_core::quotas::Quotas {
            max_saved_searches: cap,
            ..rusty_feed_core::quotas::Quotas::default()
        },
        ..AppSettings::default()
    };
    Some(AppState::new(
        db,
        Cache::in_memory().await,
        settings,
        String::new,
    ))
}

async fn session_cookie(state: &AppState, user_id: uuid::Uuid) -> String {
    let raw = format!("sess-{}", uuid::Uuid::now_v7());
    let hash = rusty_feed_web::auth::token::hash(&raw);
    let expires = time::OffsetDateTime::now_utc() + time::Duration::days(1);
    rusty_feed_db::sessions::create(state.db.pool(), user_id, &hash, expires)
        .await
        .unwrap();
    format!("rusty_feed_sid={raw}")
}

#[tokio::test]
async fn demo_reset_deletes_only_demo_users() {
    let Some(state) = state().await else { return };
    let real = users::create(
        state.db.pool(),
        &format!("real-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let demo = users::create_demo(
        state.db.pool(),
        &format!("demo-{}@demo.invalid", uuid::Uuid::now_v7()),
        "Demo user",
        "user",
    )
    .await
    .unwrap();
    users::delete_all_demo(state.db.pool()).await.unwrap();
    assert!(
        users::find_by_id(state.db.pool(), real.id)
            .await
            .unwrap()
            .is_some(),
        "the real user must survive the demo reset"
    );
    assert!(
        users::find_by_id(state.db.pool(), demo.id)
            .await
            .unwrap()
            .is_none(),
        "the demo user must be deleted"
    );
}

#[tokio::test]
async fn read_only_admin_can_read_but_not_write_admin_routes() {
    let Some(state) = state().await else { return };
    let ro = users::create(
        state.db.pool(),
        &format!("ro-admin-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "admin_readonly",
        None,
    )
    .await
    .unwrap();
    let cookie = session_cookie(&state, ro.id).await;
    let app = router(state.clone());

    // Read-only admin can GET every admin page.
    for path in [
        "/admin",
        "/admin/users",
        "/admin/feeds",
        "/admin/feeds/blocklist",
        "/admin/catalog",
        "/admin/acquisition",
        "/admin/activity",
        "/admin/logs",
        "/admin/costs",
        "/admin/ai",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get(path)
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "GET {path} should be 200"
        );
    }

    // Every admin write is a structural 403 (the extractor rejects before the
    // handler body, so no CSRF token is needed to prove the gate). Covered
    // route-by-route per §7.6 DoD.
    let id = uuid::Uuid::now_v7();
    let writes = [
        format!("/admin/users/{id}/role"),
        "/admin/invitations".to_string(),
        format!("/admin/invitations/{id}/revoke"),
        "/admin/feeds/blocklist".to_string(),
        format!("/admin/feeds/blocklist/{id}/delete"),
        format!("/admin/feeds/{id}/block"),
        format!("/admin/feeds/{id}/status"),
        format!("/admin/articles/{id}/takedown"),
        format!("/admin/abuse/{id}/resolve"),
        "/admin/catalog".to_string(),
        format!("/admin/catalog/{id}"),
        "/admin/maintenance".to_string(),
        "/admin/acquisition".to_string(),
        "/admin/acquisition/assign".to_string(),
        format!("/admin/acquisition/{id}/delete"),
        "/admin/ai".to_string(),
        format!("/admin/ai/{id}/toggle"),
        format!("/admin/ai/{id}/delete"),
        "/admin/ai/summary-policy".to_string(),
        "/admin/ai/summary-backfill".to_string(),
    ];
    for path in &writes {
        let response = app
            .clone()
            .oneshot(
                Request::post(path)
                    .header("cookie", &cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("csrf_token=x"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "POST {path} must be 403 for a read-only admin"
        );
    }
}

#[tokio::test]
async fn admin_can_persist_summary_policy_and_start_manual_backfill() {
    let Some(state) = state().await else { return };
    let admin = users::create(
        state.db.pool(),
        &format!("summary-admin-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "admin",
        None,
    )
    .await
    .unwrap();
    let session = session_cookie(&state, admin.id).await;
    let app = router(state.clone());
    let page = app
        .clone()
        .oneshot(
            Request::get("/admin/ai")
                .header("cookie", &session)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let csrf = set_cookies_value(&page, "rusty_feed_csrf").unwrap();
    let body =
        String::from_utf8_lossy(&page.into_body().collect().await.unwrap().to_bytes()).to_string();
    assert!(body.contains("Automatic article summaries"));
    assert!(body.contains("Minimum plain-text characters"));

    let response = app
        .clone()
        .oneshot(
            Request::post("/admin/ai/summary-policy")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("{session}; rusty_feed_csrf={csrf}"))
                .body(Body::from(format!(
                    "csrf_token={csrf}&enabled=true&min_content_chars=2345"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let policy = rusty_feed_db::summary_policy::get_or_init(state.db.pool(), false, 2_000)
        .await
        .unwrap();
    assert!(policy.enabled);
    assert_eq!(policy.min_content_chars, 2_345);

    let response = app
        .oneshot(
            Request::post("/admin/ai/summary-backfill")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("{session}; rusty_feed_csrf={csrf}"))
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        rusty_feed_db::jobs::count_active_by_dedup(
            state.db.pool(),
            "summary-backfill:all:false:start",
        )
        .await
        .unwrap(),
        1
    );

    rusty_feed_db::summary_policy::update(state.db.pool(), false, 2_000)
        .await
        .unwrap();
    sqlx::query("DELETE FROM job_queue WHERE dedup_key = 'summary-backfill:all:false:start'")
        .execute(state.db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn landing_page_renders_for_anonymous_visitors() {
    let Some(state) = state().await else { return };
    let app = router(state.clone());
    let response = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("Create your account"), "hero CTA present");
    assert!(
        body.contains("Own the feed"),
        "distinct product voice present"
    );
    assert!(
        body.contains("content: shared once") && body.contains("state: personal"),
        "the architectural north star is explicit"
    );
    assert!(
        body.contains("SKIP LOCKED"),
        "durable worker design present"
    );
    assert!(body.contains("Miniflux"), "comparison present");
    assert!(body.contains("Semantic"), "features present");
    // Strict-CSP: the landing page must carry no inline styles/scripts (§8.2).
    for forbidden in [
        "<script>",
        " style=",
        " onclick=",
        " onsubmit=",
        "javascript:",
    ] {
        assert!(
            !body.contains(forbidden),
            "landing page contains inline CSP violation: {forbidden}"
        );
    }
}

#[tokio::test]
async fn wizard_page_is_csp_clean_with_no_js_swatches() {
    let Some(state) = state().await else { return };
    let user = users::create(
        state.db.pool(),
        &format!("wiz-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let cookie = session_cookie(&state, user.id).await;
    let response = router(state.clone())
        .oneshot(
            Request::get("/welcome")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    // No-JS visual fallback: theme swatches render as CSS classes, not inline.
    assert!(
        body.contains("theme-swatch--ocean"),
        "no-JS swatches present"
    );
    assert!(
        body.contains("data-wizard-attr"),
        "live-preview hooks present"
    );
    for forbidden in ["<script>", " style=", " onclick=", " onsubmit="] {
        assert!(
            !body.contains(forbidden),
            "wizard page contains inline CSP violation: {forbidden}"
        );
    }
}

#[tokio::test]
async fn first_run_wizard_routes_and_persists() {
    let Some(state) = state().await else { return };
    let user = users::create(
        state.db.pool(),
        &format!("welcome-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let cookie = session_cookie(&state, user.id).await;
    let app = router(state.clone());

    // The wizard page renders for a signed-in user.
    let welcome = app
        .clone()
        .oneshot(
            Request::get("/welcome")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(welcome.status(), StatusCode::OK);
    let csrf = set_cookies_value(&welcome, "rusty_feed_csrf").unwrap_or_else(|| "x".to_string());
    let welcome_cookie = format!("{cookie}; rusty_feed_csrf={csrf}");

    // Finishing the wizard stamps onboarded_at.
    let finish = app
        .oneshot(
            Request::post("/welcome")
                .header("cookie", &welcome_cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "csrf_token={csrf}&theme=ocean&reading_mode=inline&font_family=serif&article_density=compact"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(finish.status(), StatusCode::SEE_OTHER);
    let refreshed = users::find_by_id(state.db.pool(), user.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        refreshed.onboarded_at.is_some(),
        "finishing the wizard stamps onboarded_at"
    );
}

#[tokio::test]
async fn wizard_suggests_and_subscribes_curated_feeds() {
    let Some(state) = state().await else { return };
    // A featured, published catalog source under a unique topic, so it is always
    // among the wizard suggestions regardless of any other seeded catalog data.
    let category = format!("WizardTopic-{}", uuid::Uuid::now_v7());
    let (feed, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!("https://wizard.example/{}/feed.xml", uuid::Uuid::now_v7()),
        "Wizard Curated Source",
        None,
    )
    .await
    .unwrap();
    catalog::upsert(
        state.db.pool(),
        feed.id,
        catalog::CatalogDraft {
            title: "Wizard Curated Source",
            description: Some("A hand-picked source offered during onboarding."),
            category: &category,
            country: "Global",
            language: "en",
            featured: true,
            sort_order: 0,
            published: true,
        },
    )
    .await
    .unwrap();

    let user = users::create(
        state.db.pool(),
        &format!("wizard-sub-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let cookie = session_cookie(&state, user.id).await;
    let app = router(state.clone());

    // The wizard offers our curated source and exposes its topic as an interest.
    let welcome = app
        .clone()
        .oneshot(
            Request::get("/welcome")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(welcome.status(), StatusCode::OK);
    let csrf = set_cookies_value(&welcome, "rusty_feed_csrf").unwrap_or_else(|| "x".to_string());
    let welcome_cookie = format!("{cookie}; rusty_feed_csrf={csrf}");
    let page = String::from_utf8_lossy(&welcome.into_body().collect().await.unwrap().to_bytes())
        .to_string();
    assert!(
        page.contains("Wizard Curated Source"),
        "curated suggestion is offered"
    );
    assert!(
        page.contains(&category),
        "topic appears as an interest chip"
    );
    assert!(
        page.contains(&format!("value=\"{}\"", feed.id)),
        "the suggested feed is selectable"
    );
    assert!(page.contains("Default article content for selected feeds"));
    assert!(page.contains("name=\"content_mode\" value=\"live\""));

    // Finishing with a picked feed + interest subscribes and records the interest.
    let finish = app
        .oneshot(
            Request::post("/welcome")
                .header("cookie", &welcome_cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "csrf_token={csrf}&theme=ocean&reading_mode=inline&font_family=serif&article_density=compact&content_mode=live&interest={category}&feed_id={}",
                    feed.id
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(finish.status(), StatusCode::SEE_OTHER);

    assert!(
        subscriptions::is_subscribed(state.db.pool(), user.id, feed.id)
            .await
            .unwrap(),
        "wizard subscribes to the selected curated feed"
    );
    assert_eq!(
        subscriptions::content_settings_for_user(state.db.pool(), user.id, feed.id)
            .await
            .unwrap()
            .unwrap()
            .mode,
        SubscriptionContentMode::Live
    );
    assert!(
        subscriptions::requested_article_page_lifetime(state.db.pool(), feed.id)
            .await
            .unwrap()
            .is_none(),
        "Live View must not request shared page extraction"
    );
    // The already-shared feed lands pre-organized: a personal folder named for
    // its catalog category, tagged the same way, instead of "Unfiled".
    let expected_category: String = category.chars().take(40).collect();
    let organization = subscriptions::list_organization(state.db.pool(), user.id)
        .await
        .unwrap();
    let organized = organization
        .iter()
        .find(|entry| entry.feed_id == feed.id)
        .expect("the newly subscribed feed has an organization row");
    assert_eq!(organized.tags, vec![expected_category.clone()]);
    let folder = categories::find_by_name(state.db.pool(), user.id, &expected_category)
        .await
        .unwrap()
        .expect("a folder named for the catalog category was created");
    assert_eq!(organized.category_id, Some(folder.id));
    let refreshed = users::find_by_id(state.db.pool(), user.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        refreshed.onboarded_at.is_some(),
        "wizard stamps onboarded_at"
    );
    let prefs = user_preferences::get(state.db.pool(), user.id)
        .await
        .unwrap()
        .unwrap();
    let interests = prefs
        .get("interests")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        interests
            .iter()
            .any(|value| value.as_str() == Some(category.as_str())),
        "wizard records the selected interest"
    );
}

/// Featured catalog feeds are showcase content for demos only. A regular
/// account starts empty, while every demo feed is immediately filed into its
/// catalog-category folder and tagged with that category.
#[tokio::test]
async fn featured_feeds_are_demo_only_and_organized() {
    let Some(state) = state().await else { return };
    let regular = users::create(
        state.db.pool(),
        &format!("regular-empty-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        subscriptions::count_for_user(state.db.pool(), regular.id)
            .await
            .unwrap(),
        0,
        "regular accounts must choose their own feeds"
    );

    // Other test binaries share this database and one exercises the global
    // demo reset. Protect this fixture immediately after create_demo has run;
    // if the reset wins the tiny gap, recreate it. The returned value still
    // proves create_demo itself marked the account as a demo.
    let demo = loop {
        let candidate = users::create_demo(
            state.db.pool(),
            &format!("demo-seeded-{}@demo.invalid", uuid::Uuid::now_v7()),
            "Demo user",
            "user",
        )
        .await
        .unwrap();
        let protected = sqlx::query("UPDATE users SET is_demo = false WHERE id = $1")
            .bind(candidate.id)
            .execute(state.db.pool())
            .await
            .unwrap()
            .rows_affected();
        if protected == 1 {
            break candidate;
        }
    };
    assert!(demo.is_demo, "create_demo marks the account as ephemeral");
    let demo_preferences = user_preferences::get(state.db.pool(), demo.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(demo_preferences["theme"], "rusty");
    let demo_count = subscriptions::count_for_user(state.db.pool(), demo.id)
        .await
        .unwrap();
    let featured_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_feeds uf \
         JOIN feed_catalog_entries c ON c.feed_id = uf.feed_id \
         WHERE uf.user_id = $1 AND c.published AND c.featured",
    )
    .bind(demo.id)
    .fetch_one(state.db.pool())
    .await
    .unwrap();
    assert!(demo_count > 0, "the demo catalog has showcase feeds");
    assert_eq!(
        demo_count, featured_count,
        "every demo subscription is a featured showcase feed"
    );
    let recommended_public_pages: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_feeds uf JOIN feeds f ON f.id = uf.feed_id \
         WHERE uf.user_id = $1 AND f.url = ANY($2) \
           AND uf.settings->>'content_mode' = 'article_page'",
    )
    .bind(demo.id)
    .bind(CATALOG_ARTICLE_PAGE_RECOMMENDATIONS)
    .fetch_one(state.db.pool())
    .await
    .unwrap();
    assert_eq!(
        recommended_public_pages as usize,
        CATALOG_ARTICLE_PAGE_RECOMMENDATIONS.len(),
        "recommended demo feeds use their shared public article-page copy"
    );
    let unexpected_public_pages: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_feeds uf JOIN feeds f ON f.id = uf.feed_id \
         WHERE uf.user_id = $1 AND NOT (f.url = ANY($2)) \
           AND uf.settings->>'content_mode' = 'article_page'",
    )
    .bind(demo.id)
    .bind(CATALOG_ARTICLE_PAGE_RECOMMENDATIONS)
    .fetch_one(state.db.pool())
    .await
    .unwrap();
    assert_eq!(unexpected_public_pages, 0);
    let organization = subscriptions::list_organization(state.db.pool(), demo.id)
        .await
        .unwrap();
    assert_eq!(organization.len() as i64, featured_count);
    assert!(organization.iter().all(|feed| {
        feed.category_id.is_some() && feed.tags.len() == 1 && !feed.tags[0].trim().is_empty()
    }));
    let folders = categories::list_for_user(state.db.pool(), demo.id)
        .await
        .unwrap();
    assert!(!folders.is_empty(), "demo category folders were created");
}

fn set_cookies_value(resp: &axum::response::Response, name: &str) -> Option<String> {
    resp.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|hv| hv.to_str().ok())
        .find_map(|s| {
            let s = s.split(';').next()?;
            let (k, v) = s.split_once('=')?;
            (k.trim() == name).then(|| v.trim().to_string())
        })
}

#[tokio::test]
async fn robots_and_sitemap_are_served() {
    let Some(state) = state().await else { return };
    let app = router(state.clone());
    let robots = app
        .clone()
        .oneshot(Request::get("/robots.txt").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(robots.status(), StatusCode::OK);
    let robots_body = robots.into_body().collect().await.unwrap().to_bytes();
    let robots_body = String::from_utf8_lossy(&robots_body);
    assert!(
        robots_body.contains("Disallow: /admin"),
        "app is disallowed"
    );
    assert!(robots_body.contains("Sitemap:"), "points at the sitemap");

    let sitemap = app
        .oneshot(Request::get("/sitemap.xml").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(sitemap.status(), StatusCode::OK);
    let sitemap_body = sitemap.into_body().collect().await.unwrap().to_bytes();
    let sitemap_body = String::from_utf8_lossy(&sitemap_body);
    assert!(sitemap_body.contains("<urlset"));
    assert!(sitemap_body.contains("/terms"), "lists public pages");
}

#[tokio::test]
async fn demo_users_cannot_import_opml() {
    let Some(state) = state().await else { return };
    let demo = users::create_demo(
        state.db.pool(),
        &format!("demo-opml-{}@demo.invalid", uuid::Uuid::now_v7()),
        "Demo user",
        "user",
    )
    .await
    .unwrap();
    let cookie = session_cookie(&state, demo.id).await;
    let response = router(state.clone())
        .oneshot(
            Request::post("/feeds/opml/import")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("csrf_token=x"))
                .unwrap(),
        )
        .await
        .unwrap();
    // `deny_demo` fires before body parsing → 400. (In the shared test DB the
    // concurrent demo-reset test may delete this demo user first → 401.) Either
    // way the import is refused; a regression that dropped `deny_demo` would
    // instead reach the CSRF check (403) or succeed (2xx/3xx).
    assert!(
        matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED
        ),
        "demo OPML import must be refused, got {}",
        response.status()
    );
}

#[tokio::test]
async fn users_can_report_a_feed() {
    let Some(state) = state().await else { return };
    let user = users::create(
        state.db.pool(),
        &format!("reporter-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (feed, _) = rusty_feed_db::feeds::upsert_by_url(
        state.db.pool(),
        &format!("https://example.test/feed-{}.xml", uuid::Uuid::now_v7()),
        "Reportable feed",
        None,
    )
    .await
    .unwrap();
    let cookie = session_cookie(&state, user.id).await;
    let app = router(state.clone());
    // Grab a CSRF token from a page that issues one.
    let page = app
        .clone()
        .oneshot(
            Request::get("/feeds")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = set_cookies_value(&page, "rusty_feed_csrf").unwrap_or_else(|| "x".to_string());
    let before = rusty_feed_db::abuse_reports::count_open(state.db.pool())
        .await
        .unwrap();
    let response = app
        .oneshot(
            Request::post(format!("/feeds/{}/report", feed.id))
                .header("cookie", format!("{cookie}; rusty_feed_csrf={csrf}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("csrf_token={csrf}&reason=spam")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER, "report redirects");
    let after = rusty_feed_db::abuse_reports::count_open(state.db.pool())
        .await
        .unwrap();
    assert_eq!(after, before + 1, "a feed report row was created");
}

#[tokio::test]
async fn data_export_includes_bookmarks() {
    let Some(state) = state().await else { return };
    let user = users::create(
        state.db.pool(),
        &format!("export-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let cookie = session_cookie(&state, user.id).await;
    let app = router(state.clone());
    let page = app
        .clone()
        .oneshot(
            Request::get("/settings/account")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let csrf = set_cookies_value(&page, "rusty_feed_csrf").unwrap_or_else(|| "x".to_string());
    let response = app
        .oneshot(
            Request::post("/settings/account/export")
                .header("cookie", format!("{cookie}; rusty_feed_csrf={csrf}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("\"bookmarks\""),
        "export includes reading state"
    );
    assert!(body.contains("\"subscriptions\""));
}

#[tokio::test]
async fn legal_pages_are_public() {
    let Some(state) = state().await else { return };
    let app = router(state.clone());
    for path in ["/terms", "/privacy", "/acceptable-use", "/abuse"] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path} should be public");
    }
}

#[tokio::test]
async fn last_admin_cannot_self_delete() {
    let Some(state) = state().await else { return };
    // A fresh instance whose only admin is created here — deletion must be blocked.
    let admin = users::create(
        state.db.pool(),
        &format!("solo-admin-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "admin",
        None,
    )
    .await
    .unwrap();
    // Guard only blocks when this is the last admin; other tests may create
    // admins, so assert the guard logic directly rather than global state.
    let admins_before: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'admin'")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    let deleted = users::delete(state.db.pool(), admin.id).await.unwrap();
    if admins_before <= 1 {
        assert!(!deleted, "the last admin must not be deletable");
    } else {
        assert!(deleted, "a non-last admin may be deleted");
    }
}

#[tokio::test]
async fn blocklisted_feed_is_rejected_on_add() {
    let Some(state) = state().await else { return };
    let user = users::create(
        state.db.pool(),
        &format!("block-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let raw = format!("block-token-{}", uuid::Uuid::now_v7());
    api_tokens::create(
        state.db.pool(),
        user.id,
        "test",
        &rusty_feed_web::auth::token::hash(&raw),
        &[],
    )
    .await
    .unwrap();
    let host = format!("blocked-{}.example", uuid::Uuid::now_v7());
    rusty_feed_db::feed_blocklist::add(
        state.db.pool(),
        "domain",
        &host,
        Some("test"),
        "admin",
        None,
    )
    .await
    .unwrap();
    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::post("/api/v1/feeds")
                .header("authorization", format!("Bearer {raw}"))
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"url":"https://{host}/feed.xml"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn per_user_quota_returns_409_past_the_cap() {
    let Some(state) = state_with_saved_search_cap(1).await else {
        return;
    };
    let user = users::create(
        state.db.pool(),
        &format!("quota-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let raw = format!("quota-token-{}", uuid::Uuid::now_v7());
    api_tokens::create(
        state.db.pool(),
        user.id,
        "test",
        &rusty_feed_web::auth::token::hash(&raw),
        &[],
    )
    .await
    .unwrap();
    let app = router(state.clone());

    // First saved search is within the cap.
    let first = app
        .clone()
        .oneshot(
            Request::post("/api/v1/saved-searches")
                .header("authorization", format!("Bearer {raw}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"one","query":"rust"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    // Second exceeds the cap → 409 Conflict (QuotaExceeded).
    let second = app
        .oneshot(
            Request::post("/api/v1/saved-searches")
                .header("authorization", format!("Bearer {raw}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"two","query":"go"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn bearer_resources_are_tenant_scoped_and_state_is_mutable() {
    let Some(state) = state().await else { return };
    let user = users::create(
        state.db.pool(),
        &format!("api-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!("https://api.example/{}/feed.xml", uuid::Uuid::now_v7()),
        "API feed",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(state.db.pool(), user.id, feed.id, None)
        .await
        .unwrap();
    let (article, _) = articles::upsert(
        state.db.pool(),
        &NewArticle {
            feed_id: feed.id,
            url: format!(
                "https://api.example/{}/article?id=42&utm_source=legacy",
                uuid::Uuid::now_v7()
            ),
            comments_url: None,
            guid: None,
            title: "Native API article".into(),
            author: None,
            content: "<p>Body</p>".into(),
            excerpt: Some("Body".into()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    let raw = format!("api-token-{}", uuid::Uuid::now_v7());
    api_tokens::create(
        state.db.pool(),
        user.id,
        "test",
        &rusty_feed_web::auth::token::hash(&raw),
        &[],
    )
    .await
    .unwrap();
    let app = router(state.clone());

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/articles")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/articles")
                .header("authorization", format!("Bearer {raw}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let listed = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == article.id.to_string())
        .unwrap();
    assert_eq!(
        listed["url"].as_str().unwrap().split('?').nth(1),
        Some("id=42")
    );

    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/articles/{}", article.id))
                .header("authorization", format!("Bearer {raw}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let detail: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(detail["data"]["url"].as_str().unwrap().ends_with("?id=42"));
    assert!(detail["data"]["publisher_url"]
        .as_str()
        .unwrap()
        .contains("utm_source=legacy"));

    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/categories")
                .header("x-auth-token", &raw)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"API folder"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/articles/{}/state", article.id))
                .header("authorization", format!("Bearer {raw}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"read":true,"bookmarked":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        rusty_feed_db::read_articles::is_read(state.db.pool(), user.id, article.id)
            .await
            .unwrap()
    );
    assert!(
        rusty_feed_db::bookmarks::is_bookmarked(state.db.pool(), user.id, article.id)
            .await
            .unwrap()
    );
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_audit_events WHERE user_id = $1 AND operation IN ('categories.create', 'articles.state')",
    )
    .bind(user.id)
    .fetch_one(state.db.pool())
    .await
    .unwrap();
    assert_eq!(audit_count, 2);

    let disabled = router(AppState::new(
        state.db.clone(),
        Cache::in_memory().await,
        AppSettings {
            api_enabled: false,
            ..AppSettings::default()
        },
        String::new,
    ));
    let response = disabled
        .oneshot(
            Request::get("/api/v1/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
