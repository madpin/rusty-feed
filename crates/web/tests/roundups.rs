//! Roundup HTTP authorization, graceful-state, queue, and rendering coverage.

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

use rusty_feed_cache::Cache;
use rusty_feed_core::ai_budget::AiBudget;
use rusty_feed_core::roundup::{select_candidates, RoundupKey, RoundupPeriod};
use rusty_feed_db::{
    articles, cost_ledger, feeds, llm_profiles, roundup_schedules, roundups, subscriptions, users,
    Db, NewArticle,
};
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

fn set_cookies(response: &Response<Body>) -> Vec<(String, String)> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| {
            let first = value.split(';').next()?;
            let (name, value) = first.split_once('=')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

fn cookie<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, value)| value.as_str())
}

async fn body_string(response: Response<Body>) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

async fn register(app: &Router) -> (String, String) {
    let email = format!("roundup-web-{}%40example.test", Uuid::now_v7());
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
    let sid = cookie(&set_cookies(&response), "rusty_feed_sid")
        .unwrap()
        .to_string();
    (sid, email.replace("%40", "@"))
}

async fn csrf_for(app: &Router, sid: &str, path: &str) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::get(path)
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    cookie(&set_cookies(&response), "rusty_feed_csrf")
        .unwrap()
        .to_string()
}

async fn seed_articles(
    state: &AppState,
    user_id: Uuid,
    count: usize,
) -> (rusty_feed_db::Feed, Vec<rusty_feed_db::Article>) {
    let (feed, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!("https://roundup-web.example/{}.xml", Uuid::now_v7()),
        "Shared Roundup feed",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(
        state.db.pool(),
        user_id,
        feed.id,
        Some("Personal feed rename"),
    )
    .await
    .unwrap();
    let mut articles = Vec::new();
    for index in 0..count {
        let (article, _) = articles::upsert(
            state.db.pool(),
            &NewArticle {
                feed_id: feed.id,
                url: format!("https://roundup-web.example/{index}/{}", Uuid::now_v7()),
                comments_url: None,
                guid: None,
                title: format!("Web source article {index}"),
                author: None,
                content: format!("Full body {index}"),
                excerpt: Some(format!("Shared excerpt {index}")),
                image_url: None,
                enclosures: Vec::new(),
                published_at: None,
            },
        )
        .await
        .unwrap();
        articles.push(article);
    }
    (feed, articles)
}

async fn enable_personal_provider(state: &AppState, user_id: Uuid) {
    llm_profiles::create(
        state.db.pool(),
        Some(user_id),
        "user",
        "Test personal provider",
        "openai",
        "test-roundup-model",
        "https://api.openai.com",
        None,
        true,
        1,
    )
    .await
    .unwrap();
}

async fn post_roundup(
    app: &Router,
    sid: &str,
    csrf: &str,
    feed_id: Uuid,
    period: &str,
) -> Response<Body> {
    app.clone()
        .oneshot(
            Request::post(format!("/roundups/feed/{feed_id}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!("csrf_token={csrf}&period={period}")))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn roundup_requires_authentication() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let response = router(state)
        .oneshot(
            Request::get(format!("/roundups/feed/{}", Uuid::now_v7()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn roundup_rejects_bad_csrf_and_explains_empty_and_unavailable_states() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    let (feed, _) = seed_articles(&state, user.id, 0).await;
    let library_path = format!("/articles?feed={}&unread=0", feed.id);
    let response = app
        .clone()
        .oneshot(
            Request::get(&library_path)
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = cookie(&set_cookies(&response), "rusty_feed_csrf")
        .unwrap()
        .to_string();
    let library = body_string(response).await;
    assert!(library.contains(&format!("/roundups/feed/{}", feed.id)));
    assert!(library.contains("Summarize"));
    assert!(library.contains("value=\"day\""));
    assert!(library.contains("value=\"week\""));

    let rejected = app
        .clone()
        .oneshot(
            Request::post(format!("/roundups/feed/{}", feed.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from("csrf_token=wrong-csrf&period=day"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);

    let response = post_roundup(&app, &sid, &csrf, feed.id, "day").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.contains("notice=too-few"));
    let response = app
        .clone()
        .oneshot(
            Request::get(location)
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response).await.contains("at least 3 articles"));

    let (full_feed, _) = seed_articles(&state, user.id, 3).await;
    let response = post_roundup(&app, &sid, &csrf, full_feed.id, "day").await;
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.contains("notice=unavailable"));
    let response = app
        .clone()
        .oneshot(
            Request::get(location)
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response)
        .await
        .contains("No AI provider is configured"));
}

#[tokio::test]
async fn roundup_enqueues_interactive_work_polls_and_renders_shared_sources() {
    let Some(mut state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    std::sync::Arc::make_mut(&mut state.settings).summarization_enabled = true;
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    enable_personal_provider(&state, user.id).await;
    let (feed, articles) = seed_articles(&state, user.id, 3).await;
    let csrf = csrf_for(&app, &sid, &format!("/articles?feed={}", feed.id)).await;

    let response = post_roundup(&app, &sid, &csrf, feed.id, "week").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.contains("period=week"));
    assert!(location.contains("input_hash="));
    assert!(location.contains("model_version=test-roundup-model"));

    let (lane, payload): (String, serde_json::Value) = sqlx::query_as(
        "SELECT queue_name, payload FROM job_queue WHERE kind = 'roundup' \
         AND payload ->> 'user_id' = $1",
    )
    .bind(user.id.to_string())
    .fetch_one(state.db.pool())
    .await
    .unwrap();
    assert_eq!(lane, "interactive");
    assert_eq!(payload["article_ids"].as_array().unwrap().len(), 3);
    assert!(!payload.to_string().contains("Personal feed rename"));

    let response = app
        .clone()
        .oneshot(
            Request::get(&location)
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let page = body_string(response).await;
    assert!(page.contains("Waiting for its turn"));
    assert!(page.contains("data-roundup-status-url"));
    let status_path = location.replacen('?', "/status?", 1);
    let response = app
        .clone()
        .oneshot(
            Request::get(&status_path)
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_string(response).await.contains("\"state\":\"queued\""));

    let candidates = roundups::candidates_for_user(
        state.db.pool(),
        user.id,
        roundups::RoundupScope::Feed(feed.id),
        RoundupPeriod::Week,
    )
    .await
    .unwrap();
    let selected = select_candidates(
        candidates,
        RoundupPeriod::Week,
        time::OffsetDateTime::now_utc(),
    );
    let ids: Vec<_> = selected.iter().map(|candidate| candidate.id).collect();
    let key = RoundupKey::new(&ids, RoundupPeriod::Week, "test-roundup-model");
    let body = format!(
        "A short intro.\n\n## Main theme\n\nCohesive news [1](/articles/{})",
        articles[0].id
    );
    roundups::insert(
        state.db.pool(),
        roundups::NewRoundup {
            key: &key,
            provider: "openai",
            model: "test-roundup-model",
            headline: "A completed shared Roundup",
            body_markdown: &body,
            source_ids: &[articles[0].id],
        },
    )
    .await
    .unwrap()
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::get(&status_path)
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
            Request::get(&location)
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let page = body_string(response).await;
    assert!(page.contains("A completed shared Roundup"));
    assert!(page.contains("Sources"));
    assert!(page.contains("Original article"));
    assert!(page.contains(&format!("/articles/{}", articles[0].id)));
}

#[tokio::test]
async fn roundup_budget_gate_is_an_in_page_explanation_and_queues_nothing() {
    let Some(mut state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    {
        let settings = std::sync::Arc::make_mut(&mut state.settings);
        settings.summarization_enabled = true;
        settings.ai_budget = AiBudget::from_config(0.0, 0.0, 5, 0);
    }
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    enable_personal_provider(&state, user.id).await;
    let (feed, _) = seed_articles(&state, user.id, 3).await;
    cost_ledger::record(
        state.db.pool(),
        Some(user.id),
        "openai",
        "test-roundup-model",
        "roundup",
        10,
        10,
        0,
        true,
    )
    .await
    .unwrap();
    let csrf = csrf_for(&app, &sid, &format!("/articles?feed={}", feed.id)).await;
    let response = post_roundup(&app, &sid, &csrf, feed.id, "day").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.contains("notice=budget"));
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM job_queue WHERE kind = 'roundup' AND payload ->> 'user_id' = $1",
    )
    .bind(user.id.to_string())
    .fetch_one(state.db.pool())
    .await
    .unwrap();
    assert_eq!(queued, 0, "the weighted BYO-key cap still blocks new work");
    let response = app
        .clone()
        .oneshot(
            Request::get(location)
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response).await.contains("AI call limit"));
}

async fn post_manager_form(
    app: &Router,
    sid: &str,
    csrf: &str,
    path: &str,
    fields: String,
) -> Response<Body> {
    app.clone()
        .oneshot(
            Request::post(path)
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!("csrf_token={csrf}&{fields}")))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn roundup_manager_requires_authentication() {
    let Some(state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let response = router(state)
        .oneshot(Request::get("/roundups").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn roundup_manager_enforces_csrf_ownership_validation_cap_and_history() {
    let Some(mut state) = test_state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    std::sync::Arc::make_mut(&mut state.settings).roundup_schedule_limit = 1;
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    let (feed, _) = seed_articles(&state, user.id, 0).await;
    let csrf = csrf_for(&app, &sid, "/roundups").await;
    let page = app
        .clone()
        .oneshot(
            Request::get("/roundups")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = body_string(page).await;
    assert!(html.contains("New Roundup"));
    assert!(html.contains("No repeating Roundups yet"));
    assert!(html.contains("No Roundups in your history yet"));
    assert!(html.contains("nav-roundups"));

    let rejected = app
        .clone()
        .oneshot(
            Request::post("/roundups")
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("rusty_feed_sid={sid}; rusty_feed_csrf={csrf}"),
                )
                .body(Body::from(format!(
                    "csrf_token=wrong-csrf&scope=feed:{}&period=day&repeat=1&local_time=07:00",
                    feed.id
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    let created = post_manager_form(
        &app,
        &sid,
        &csrf,
        "/roundups",
        format!(
            "scope=feed:{}&period=day&repeat=1&local_time=07:00&email_delivery=1",
            feed.id
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let schedule = roundup_schedules::list_for_user(state.db.pool(), user.id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(schedule.scope_id, feed.id);
    assert_eq!(schedule.local_hour, 7);
    assert_eq!(schedule.timezone, "UTC");
    assert!(schedule.email_delivery);

    let (second_feed, _) = seed_articles(&state, user.id, 0).await;
    let capped = post_manager_form(
        &app,
        &sid,
        &csrf,
        "/roundups",
        format!(
            "scope=feed:{}&period=day&repeat=1&local_time=08:00",
            second_feed.id
        ),
    )
    .await;
    assert_eq!(capped.status(), StatusCode::SEE_OTHER);
    assert!(capped
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .contains("notice=cap"));
    assert_eq!(
        roundup_schedules::count_for_user(state.db.pool(), user.id)
            .await
            .unwrap(),
        1
    );

    let invalid = post_manager_form(
        &app,
        &sid,
        &csrf,
        "/roundups",
        format!(
            "scope=feed:{}&period=day&repeat=1&local_time=07:30",
            second_feed.id
        ),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let invalid_period = post_manager_form(
        &app,
        &sid,
        &csrf,
        "/roundups",
        format!(
            "scope=feed:{}&period=month&repeat=1&local_time=07:00",
            second_feed.id
        ),
    )
    .await;
    assert_eq!(invalid_period.status(), StatusCode::BAD_REQUEST);

    let manual = post_manager_form(
        &app,
        &sid,
        &csrf,
        "/roundups",
        format!("scope=feed:{}&period=day&local_time=07:00", feed.id),
    )
    .await;
    assert_eq!(manual.status(), StatusCode::SEE_OTHER);
    let history_page = app
        .clone()
        .oneshot(
            Request::get("/roundups")
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = body_string(history_page).await;
    assert!(html.contains("Fewer than 3 articles"));
    assert!(html.contains("Skipped"));

    let (other_sid, other_email) = register(&app).await;
    let other = users::find_by_email(state.db.pool(), &other_email)
        .await
        .unwrap()
        .unwrap();
    let other_csrf = csrf_for(&app, &other_sid, "/roundups").await;
    let foreign_scope = post_manager_form(
        &app,
        &other_sid,
        &other_csrf,
        "/roundups",
        format!(
            "scope=feed:{}&period=day&repeat=1&local_time=07:00",
            feed.id
        ),
    )
    .await;
    assert_eq!(foreign_scope.status(), StatusCode::NOT_FOUND);
    let foreign_edit = app
        .clone()
        .oneshot(
            Request::get(format!("/roundups?edit={}", schedule.id))
                .header("cookie", format!("rusty_feed_sid={other_sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_edit.status(), StatusCode::NOT_FOUND);
    let foreign_toggle = post_manager_form(
        &app,
        &other_sid,
        &other_csrf,
        &format!("/roundups/schedules/{}/toggle", schedule.id),
        String::new(),
    )
    .await;
    assert_eq!(foreign_toggle.status(), StatusCode::NOT_FOUND);
    let foreign_update = post_manager_form(
        &app,
        &other_sid,
        &other_csrf,
        &format!("/roundups/schedules/{}", schedule.id),
        format!(
            "scope=feed:{}&period=day&repeat=1&local_time=09:00",
            feed.id
        ),
    )
    .await;
    assert_eq!(foreign_update.status(), StatusCode::NOT_FOUND);
    let foreign_delete = post_manager_form(
        &app,
        &other_sid,
        &other_csrf,
        &format!("/roundups/schedules/{}/delete", schedule.id),
        String::new(),
    )
    .await;
    assert_eq!(foreign_delete.status(), StatusCode::NOT_FOUND);
    assert!(
        roundup_schedules::find_for_user(state.db.pool(), user.id, schedule.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(roundup_schedules::list_for_user(state.db.pool(), other.id)
        .await
        .unwrap()
        .is_empty());

    let toggled = post_manager_form(
        &app,
        &sid,
        &csrf,
        &format!("/roundups/schedules/{}/toggle", schedule.id),
        String::new(),
    )
    .await;
    assert_eq!(toggled.status(), StatusCode::SEE_OTHER);
    assert!(
        !roundup_schedules::find_for_user(state.db.pool(), user.id, schedule.id)
            .await
            .unwrap()
            .unwrap()
            .enabled
    );
    let edit = app
        .clone()
        .oneshot(
            Request::get(format!("/roundups?edit={}", schedule.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_string(edit).await.contains("Edit repeating Roundup"));
    let updated = post_manager_form(
        &app,
        &sid,
        &csrf,
        &format!("/roundups/schedules/{}", schedule.id),
        format!(
            "scope=feed:{}&period=day&repeat=1&local_time=08:00",
            feed.id
        ),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::SEE_OTHER);
    let updated_schedule = roundup_schedules::find_for_user(state.db.pool(), user.id, schedule.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated_schedule.local_hour, 8);
    assert!(!updated_schedule.email_delivery);
    let deleted = post_manager_form(
        &app,
        &sid,
        &csrf,
        &format!("/roundups/schedules/{}/delete", schedule.id),
        String::new(),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::SEE_OTHER);
    assert!(
        roundup_schedules::find_for_user(state.db.pool(), user.id, schedule.id)
            .await
            .unwrap()
            .is_none()
    );
}
