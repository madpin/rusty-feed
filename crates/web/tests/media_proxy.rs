//! Shared media proxy authorization, caching, and range behavior.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rusty_feed_cache::Cache;
use rusty_feed_db::{articles, feeds, subscriptions, users, Db, NewArticle, NewArticleEnclosure};
use rusty_feed_feeds::SafeFetchClient;
use rusty_feed_web::{router, AppSettings, AppState};

fn set_cookies(response: &Response<Body>) -> Vec<(String, String)> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| {
            let (name, value) = value.split(';').next()?.split_once('=')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

fn cookie<'a>(cookies: &'a [(String, String)], name: &str) -> Option<&'a str> {
    cookies
        .iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, value)| value.as_str())
}

async fn register(app: &Router) -> (String, String) {
    let email = format!("media-{}%40example.com", Uuid::now_v7());
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
async fn media_is_article_authorized_range_capable_and_shared() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let publisher = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/episode.mp3"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/mpeg")
                .set_body_bytes(b"0123456789"),
        )
        .expect(1)
        .mount(&publisher)
        .await;
    Mock::given(method("GET"))
        .and(path("/hero.jpg"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/jpeg")
                .set_body_bytes(b"private-header-image"),
        )
        .expect(1)
        .mount(&publisher)
        .await;

    let state = AppState::new(
        db,
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    )
    .with_fetch(SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap());
    let app = router(state.clone());
    let (sid, email) = register(&app).await;
    let (second_sid, second_email) = register(&app).await;
    let user = users::find_by_email(state.db.pool(), &email)
        .await
        .unwrap()
        .unwrap();
    let second_user = users::find_by_email(state.db.pool(), &second_email)
        .await
        .unwrap()
        .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        state.db.pool(),
        &format!("https://media.example/{}.xml", Uuid::now_v7()),
        "Media",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(state.db.pool(), user.id, feed.id, None)
        .await
        .unwrap();
    let media_url = format!("{}/episode.mp3", publisher.uri());
    let (article, _) = articles::upsert(
        state.db.pool(),
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://media.example/article/{}", Uuid::now_v7()),
            comments_url: None,
            guid: Some(Uuid::now_v7().to_string()),
            title: "Episode".to_string(),
            author: None,
            content: "<p>Listen</p>".to_string(),
            excerpt: None,
            image_url: Some(format!("{}/hero.jpg", publisher.uri())),
            enclosures: vec![NewArticleEnclosure {
                url: media_url.clone(),
                media_kind: "audio".to_string(),
                mime_type: Some("audio/mpeg".to_string()),
                title: None,
                size_bytes: Some(10),
                duration_seconds: None,
                width: None,
                height: None,
            }],
            published_at: None,
        },
    )
    .await
    .unwrap();
    let audio = [
        rusty_feed_core::article_audio::ArticleAudio::new(
            "Umbriel",
            &media_url,
            "audio/mpeg",
            &article.url,
        )
        .unwrap(),
        rusty_feed_core::article_audio::ArticleAudio::new(
            "Gacrux",
            &format!("{}/not-audio", publisher.uri()),
            "audio/mpeg",
            &article.url,
        )
        .unwrap(),
        rusty_feed_core::article_audio::ArticleAudio::new(
            "Private",
            &format!("{}/private.mp3", publisher.uri()),
            "audio/mpeg",
            &article.url,
        )
        .unwrap(),
    ];
    articles::set_extracted_content_with_metadata(
        state.db.pool(),
        article.id,
        article.content_revision,
        "<p>Public page body</p>",
        None,
        rusty_feed_core::feed_content::ContentLifetime::Days(1),
        articles::ExtractedContentMetadata {
            blocked_tracking_pixels: 0,
            audio: &audio,
        },
    )
    .await
    .unwrap()
    .unwrap();
    let audio_route = format!("/articles/{}/audio/0", article.id);
    for cookie_header in [None, Some(format!("rusty_feed_sid={second_sid}"))] {
        let mut request = Request::get(&audio_route);
        let expected = if let Some(value) = cookie_header {
            request = request.header("cookie", value);
            StatusCode::NOT_FOUND
        } else {
            StatusCode::UNAUTHORIZED
        };
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    for view in [
        format!("/articles/{}?content=article_page", article.id),
        format!("/articles/{}/panel?content=article_page", article.id),
        format!("/articles/{}/content?content=article_page", article.id),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get(view)
                    .header("cookie", format!("rusty_feed_sid={sid}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("Listen to article"));
        assert!(html.contains(&format!("src=\"{audio_route}\"")));
        assert!(html.contains("Gacrux"));
        assert!(!html.contains(&format!("src=\"{media_url}\"")));
    }
    assert!(
        publisher.received_requests().await.unwrap().is_empty(),
        "rendering must not fetch narration"
    );
    let route = format!("/articles/{}/media/0", article.id);
    let hero_route = format!("/articles/{}/hero", article.id);

    let response = app
        .clone()
        .oneshot(Request::get(&route).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .clone()
        .oneshot(Request::get(&hero_route).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .clone()
        .oneshot(
            Request::get(&route)
                .header("cookie", format!("rusty_feed_sid={second_sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(
            Request::get(&hero_route)
                .header("cookie", format!("rusty_feed_sid={second_sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    subscriptions::subscribe(state.db.pool(), second_user.id, feed.id, None)
        .await
        .unwrap();
    let second_state = AppState::new(
        state.db.clone(),
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    )
    .with_fetch(SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap());
    let second_app = router(second_state);

    let (response, second_response) = tokio::join!(
        app.clone().oneshot(
            Request::get(&route)
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .header("range", "bytes=2-5")
                .body(Body::empty())
                .unwrap(),
        ),
        second_app.clone().oneshot(
            Request::get(&route)
                .header("cookie", format!("rusty_feed_sid={second_sid}"))
                .body(Body::empty())
                .unwrap(),
        )
    );
    let response = response.unwrap();
    let second_response = second_response.unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()["content-type"], "audio/mpeg");
    assert_eq!(response.headers()["content-range"], "bytes 2-5/10");
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        &b"2345"[..]
    );
    assert_eq!(second_response.status(), StatusCode::OK);
    assert_eq!(
        second_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
        &b"0123456789"[..]
    );

    let (response, second_response) = tokio::join!(
        app.clone().oneshot(
            Request::get(&hero_route)
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        ),
        second_app.oneshot(
            Request::get(&hero_route)
                .header("cookie", format!("rusty_feed_sid={second_sid}"))
                .body(Body::empty())
                .unwrap(),
        )
    );
    for response in [response.unwrap(), second_response.unwrap()] {
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "image/jpeg");
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            &b"private-header-image"[..]
        );
    }

    let response = app
        .clone()
        .oneshot(
            Request::get(format!("{route}?download=1"))
                .header("cookie", format!("rusty_feed_sid={second_sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"episode.mp3\""
    );

    let response = app
        .clone()
        .oneshot(
            Request::get(&route)
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .header("range", "bytes=20-")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(response.headers()["content-range"], "bytes */10");
    let response = app
        .clone()
        .oneshot(
            Request::get(&audio_route)
                .header("cookie", format!("rusty_feed_sid={second_sid}"))
                .header("range", "bytes=2-5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()["content-type"], "audio/mpeg");
    assert_eq!(response.headers()["content-range"], "bytes 2-5/10");
    // The enclosure and narration share a single URL cache across readers.
    assert_eq!(
        publisher
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.url.path() == "/episode.mp3")
            .count(),
        1
    );
    Mock::given(method("GET"))
        .and(path("/not-audio"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_string("<script>bad()</script>"),
        )
        .expect(1)
        .mount(&publisher)
        .await;
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/articles/{}/audio/1", article.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let safe_app = router(AppState::new(
        state.db.clone(),
        Cache::in_memory().await,
        AppSettings::default(),
        String::new,
    ));
    let response = safe_app
        .oneshot(
            Request::get(format!("/articles/{}/audio/2", article.id))
                .header("cookie", format!("rusty_feed_sid={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::BAD_GATEWAY,
        "SafeFetchClient must block private narration targets"
    );
    assert!(!publisher
        .received_requests()
        .await
        .unwrap()
        .iter()
        .any(|request| request.url.path() == "/private.mp3"));
    publisher.verify().await;
}
