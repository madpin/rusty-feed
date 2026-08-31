//! M11 web test: saved-search create/list/delete. Requires `TEST_DATABASE_URL`.

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

use rustss_cache::Cache;
use rustss_db::{saved_searches, users, Db};
use rustss_web::{router, AppSettings, AppState};

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
    let email = format!("ss-{}%40example.com", uuid::Uuid::now_v7());
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
async fn create_list_and_delete_saved_search() {
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

    let resp = app
        .clone()
        .oneshot(
            Request::get("/searches")
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

    // Create.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/searches")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!(
                    "name=My+Rust+Alert&query=rust+%2Basync&notify=true&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let user = users::find_by_email(db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    let searches = saved_searches::list_for_user(db.pool(), user.id)
        .await
        .unwrap();
    assert_eq!(searches.len(), 1);
    assert_eq!(searches[0].name, "My Rust Alert");
    assert!(searches[0].notify_on_match);
    let id = searches[0].id;

    // It appears on the list page.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/searches")
                .header("cookie", format!("rustss_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_string(resp).await.contains("My Rust Alert"));

    // Delete.
    let resp = app
        .oneshot(
            Request::post(format!("/searches/{id}/delete"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("rustss_sid={sid}; rustss_csrf={csrf}"))
                .body(Body::from(format!("csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(saved_searches::list_for_user(db.pool(), user.id)
        .await
        .unwrap()
        .is_empty());
}
