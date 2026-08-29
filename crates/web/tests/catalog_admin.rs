use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

use rustss_cache::Cache;
use rustss_db::{catalog, feeds, subscriptions, users, Db};
use rustss_web::{router, AppSettings, AppState};

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
    let csrf = cookie(&set_cookies(&response), "rustss_csrf")
        .unwrap()
        .to_string();
    let response = app
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
    (
        cookie(&set_cookies(&response), "rustss_sid")
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

    let response = app
        .clone()
        .oneshot(
            Request::get("/catalog")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let csrf = cookie(&set_cookies(&response), "rustss_csrf")
        .unwrap()
        .to_string();
    let page = body_string(response).await;
    assert!(page.contains("Discover feeds"));
    assert!(page.contains("Rust Blog"));
    assert!(page.contains("Catalog UI source"));

    let response = app
        .clone()
        .oneshot(
            Request::post("/catalog/subscribe")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "csrf_token={csrf}&feed_id={}&feed_id={}",
                    feed.id, second_feed.id
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response)
        .await
        .contains("Subscribed to 2 source"));
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

    let response = app
        .clone()
        .oneshot(
            Request::get("/admin")
                .header("cookie", format!("rustss_sid={sid}"))
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
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response).await.contains("Instance overview"));

    let response = app
        .clone()
        .oneshot(
            Request::get("/admin/catalog")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = cookie(&set_cookies(&response), "rustss_csrf")
        .unwrap()
        .to_string();
    assert!(body_string(response)
        .await
        .contains("Add a source to the magazine"));
    let admin_catalog_url = format!(
        "https://admin-catalog.example/{}/feed.xml",
        uuid::Uuid::now_v7()
    );
    let response = app
        .clone()
        .oneshot(
            Request::post("/admin/catalog")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
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
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response).await.contains("Shared source policy"));

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
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = cookie(&set_cookies(&response), "rustss_csrf")
        .unwrap()
        .to_string();
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/admin/users/{}/role", user.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
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
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
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
