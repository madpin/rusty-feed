//! M10 integration tests for the personalization service (feedback → patterns →
//! scoring, precedence, reset). Require `TEST_DATABASE_URL`; skipped otherwise.

use rustss_cache::Cache;
use rustss_db::models::NewArticle;
use rustss_db::{articles, feedback, feeds, patterns, Article, Db};
use rustss_search::personalize;
use rustss_search::ExplicitFeedback;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn make_user(db: &Db) -> Uuid {
    users_create(db).await
}

async fn users_create(db: &Db) -> Uuid {
    rustss_db::users::create(
        db.pool(),
        &format!("pz-{}@ex.com", Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap()
    .id
}

async fn make_article(db: &Db, title: &str, content: &str) -> Article {
    let url = format!("https://ex.com/{}.xml", Uuid::now_v7());
    let (feed, _) = feeds::upsert_by_url(db.pool(), &url, "F", None)
        .await
        .unwrap();
    let (a, _) = articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://ex.com/{}", Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: title.into(),
            author: None,
            content: content.into(),
            excerpt: None,
            image_url: None,
            published_at: None,
        },
    )
    .await
    .unwrap();
    articles::find_by_id(db.pool(), a.id)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn thumbs_up_raises_score_down_lowers_it() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let cache = Cache::in_memory().await;
    let user = make_user(&db).await;

    let liked = make_article(
        &db,
        "Async Rust",
        "concurrency with async await in rust programming",
    )
    .await;
    let disliked = make_article(
        &db,
        "Pasta",
        "authentic italian pasta tomato cooking recipe",
    )
    .await;

    // Neutral before any feedback.
    let before = personalize::score_article(&db, &cache, user, &liked)
        .await
        .unwrap();
    assert!((before.score - 0.5).abs() < 1e-9);

    personalize::record_explicit(&db, &cache, user, &liked, ExplicitFeedback::Up)
        .await
        .unwrap();
    personalize::record_explicit(&db, &cache, user, &disliked, ExplicitFeedback::Down)
        .await
        .unwrap();

    // Patterns were learned.
    assert!(patterns::count_for(db.pool(), user).await.unwrap() > 0);

    let liked_score = personalize::score_article(&db, &cache, user, &liked)
        .await
        .unwrap();
    let disliked_score = personalize::score_article(&db, &cache, user, &disliked)
        .await
        .unwrap();
    assert!(
        liked_score.score > 0.5,
        "liked article should score above neutral"
    );
    assert!(
        disliked_score.score < 0.5,
        "disliked article should score below neutral"
    );
    assert!(!liked_score.explanations.is_empty());
}

#[tokio::test]
async fn explicit_feedback_not_overwritten_by_implicit() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let cache = Cache::in_memory().await;
    let user = make_user(&db).await;
    let article = make_article(
        &db,
        "Topic",
        "some interesting article content about topics",
    )
    .await;

    personalize::record_explicit(&db, &cache, user, &article, ExplicitFeedback::Up)
        .await
        .unwrap();
    // An implicit bounce should be ignored (explicit wins).
    let applied = personalize::record_implicit(&db, &cache, user, &article, 1.0, 100.0, 0.25)
        .await
        .unwrap();
    assert!(!applied, "implicit must not override explicit");
    assert_eq!(
        feedback::existing_type(db.pool(), user, article.id)
            .await
            .unwrap()
            .as_deref(),
        Some("explicit")
    );
}

#[tokio::test]
async fn reset_learning_clears_everything() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let cache = Cache::in_memory().await;
    let user = make_user(&db).await;
    let article = make_article(
        &db,
        "Rust",
        "rust systems programming language memory safety",
    )
    .await;

    personalize::record_explicit(&db, &cache, user, &article, ExplicitFeedback::Up)
        .await
        .unwrap();
    assert!(patterns::count_for(db.pool(), user).await.unwrap() > 0);

    personalize::reset_learning(&db, &cache, user)
        .await
        .unwrap();
    assert_eq!(patterns::count_for(db.pool(), user).await.unwrap(), 0);
    assert!(feedback::existing_type(db.pool(), user, article.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn replacing_explicit_feedback_is_idempotent() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let cache = Cache::in_memory().await;
    let user = make_user(&db).await;
    let article = make_article(&db, "Rust", "rust concurrency runtime").await;
    let terms = vec!["rust".to_string()];

    personalize::record_explicit(&db, &cache, user, &article, ExplicitFeedback::Up)
        .await
        .unwrap();
    let initial = patterns::weights_for(db.pool(), user, &terms)
        .await
        .unwrap()[0]
        .1;

    personalize::record_explicit(&db, &cache, user, &article, ExplicitFeedback::Up)
        .await
        .unwrap();
    let repeated = patterns::weights_for(db.pool(), user, &terms)
        .await
        .unwrap()[0]
        .1;
    assert!((initial - repeated).abs() < f64::EPSILON);

    personalize::record_explicit(&db, &cache, user, &article, ExplicitFeedback::Down)
        .await
        .unwrap();
    let replaced = patterns::weights_for(db.pool(), user, &terms)
        .await
        .unwrap()[0]
        .1;
    assert!((initial + replaced).abs() < 1e-12);
    assert_eq!(
        feedback::get(db.pool(), user, article.id)
            .await
            .unwrap()
            .unwrap()
            .feedback_value,
        -1.0
    );
}
