//! PostgreSQL full-text and hybrid search integration coverage.
//! Tests skip when `TEST_DATABASE_URL` is unavailable.

use rustss_core::config::SearchTextConfig;
use rustss_db::models::NewArticle;
use rustss_db::{articles, embeddings, feeds, search, subscriptions, users, Db};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect to test database");
    db.migrate().await.expect("run migrations");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::now_v7())
}

async fn user(db: &Db, prefix: &str) -> Uuid {
    users::create(
        db.pool(),
        &format!("{}@example.test", unique(prefix)),
        None,
        None,
        "user",
        None,
    )
    .await
    .expect("create user")
    .id
}

async fn subscribed_feed(db: &Db, user_id: Uuid, prefix: &str) -> Uuid {
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://search.example/{}/feed.xml", unique(prefix)),
        prefix,
        None,
    )
    .await
    .expect("create feed");
    subscriptions::subscribe(db.pool(), user_id, feed.id, None)
        .await
        .expect("subscribe");
    feed.id
}

async fn article(db: &Db, feed_id: Uuid, title: &str, content: &str) -> rustss_db::Article {
    articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id,
            url: format!("https://search.example/articles/{}", Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: title.to_string(),
            author: None,
            content: content.to_string(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .expect("create article")
    .0
}

#[tokio::test]
async fn keyword_search_supports_phrases_exclusions_language_and_tenant_scope() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let reader = user(&db, "search-reader").await;
    let outsider = user(&db, "search-outsider").await;
    let reader_feed = subscribed_feed(&db, reader, "reader-feed").await;
    let outsider_feed = subscribed_feed(&db, outsider, "outsider-feed").await;

    let expected = article(
        &db,
        reader_feed,
        "Async Rust in Production",
        "A Tokio runtime field guide.",
    )
    .await;
    article(
        &db,
        reader_feed,
        "Async Rust for Beginners",
        "An introductory course.",
    )
    .await;
    article(
        &db,
        outsider_feed,
        "Async Rust in Production",
        "This private subscription must never leak.",
    )
    .await;

    let hits = search::keyword_for_user(
        db.pool(),
        reader,
        "\"async rust\" -beginners",
        SearchTextConfig::Simple,
        20,
    )
    .await
    .expect("search phrase");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, expected.id);
    assert!(hits[0].keyword_score.is_some());
    assert!(hits[0].semantic_score.is_none());

    let stemmed = article(
        &db,
        reader_feed,
        "Systems running smoothly",
        "Operational notes.",
    )
    .await;
    let simple = search::keyword_for_user(db.pool(), reader, "run", SearchTextConfig::Simple, 20)
        .await
        .expect("simple search");
    assert!(!simple.iter().any(|hit| hit.id == stemmed.id));
    let english = search::keyword_for_user(db.pool(), reader, "run", SearchTextConfig::English, 20)
        .await
        .expect("english search");
    assert!(english.iter().any(|hit| hit.id == stemmed.id));

    let alternatives = search::keyword_for_user(
        db.pool(),
        reader,
        "\"async rust\" OR \"systems running\"",
        SearchTextConfig::Simple,
        20,
    )
    .await
    .expect("OR search");
    assert!(alternatives.iter().any(|hit| hit.id == expected.id));
    assert!(alternatives.iter().any(|hit| hit.id == stemmed.id));
}

#[tokio::test]
async fn generated_vectors_follow_updates_and_title_matches_rank_higher() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let reader = user(&db, "search-update").await;
    let feed_id = subscribed_feed(&db, reader, "update-feed").await;
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT indexname FROM pg_indexes \
         WHERE schemaname = current_schema() \
           AND indexname = ANY($1) ORDER BY indexname",
    )
    .bind(vec!["articles_search_idx", "articles_search_simple_idx"])
    .fetch_all(db.pool())
    .await
    .expect("inspect full-text indexes");
    assert_eq!(
        indexes,
        vec![
            "articles_search_idx".to_string(),
            "articles_search_simple_idx".to_string()
        ]
    );
    let title_hit = article(
        &db,
        feed_id,
        "Postgres indexing",
        "A practical database guide.",
    )
    .await;
    let body_hit = article(
        &db,
        feed_id,
        "Database notes",
        "A guide to Postgres indexing.",
    )
    .await;

    let ranked = search::keyword_for_user(
        db.pool(),
        reader,
        "postgres indexing",
        SearchTextConfig::Simple,
        20,
    )
    .await
    .expect("rank keyword hits");
    let title_position = ranked
        .iter()
        .position(|hit| hit.id == title_hit.id)
        .unwrap();
    let body_position = ranked.iter().position(|hit| hit.id == body_hit.id).unwrap();
    assert!(title_position < body_position, "title weighting should win");

    let mut changed = NewArticle {
        feed_id,
        url: title_hit.url.clone(),
        comments_url: None,
        guid: None,
        title: "Cobalt release".to_string(),
        author: None,
        content: "Initial notes.".to_string(),
        excerpt: None,
        image_url: None,
        enclosures: Vec::new(),
        published_at: None,
    };
    articles::upsert(db.pool(), &changed)
        .await
        .expect("update searchable article");
    changed.title = "Amber release".to_string();
    articles::upsert(db.pool(), &changed)
        .await
        .expect("update generated vector");

    let amber = search::keyword_for_user(db.pool(), reader, "amber", SearchTextConfig::Simple, 20)
        .await
        .expect("search updated vector");
    assert!(amber.iter().any(|hit| hit.id == title_hit.id));
    let cobalt =
        search::keyword_for_user(db.pool(), reader, "cobalt", SearchTextConfig::Simple, 20)
            .await
            .expect("search previous vector");
    assert!(!cobalt.iter().any(|hit| hit.id == title_hit.id));
}

#[tokio::test]
async fn hybrid_search_keeps_keyword_only_articles_and_normalizes_cosine_scores() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let reader = user(&db, "search-hybrid").await;
    let feed_id = subscribed_feed(&db, reader, "hybrid-feed").await;
    let keyword_only = article(
        &db,
        feed_id,
        "Quasar observatory",
        "No embedding is available yet.",
    )
    .await;
    let semantic = article(
        &db,
        feed_id,
        "Astronomy report",
        "A meaning-based candidate.",
    )
    .await;
    let opposite = article(
        &db,
        feed_id,
        "Opposite vector",
        "A cosine normalization boundary.",
    )
    .await;
    let mut query_vector = vec![0.0_f32; 384];
    query_vector[0] = 1.0;
    let mut opposite_vector = vec![0.0_f32; 384];
    opposite_vector[0] = -1.0;
    embeddings::upsert(
        db.pool(),
        semantic.id,
        &query_vector,
        "search-test-model",
        semantic.content_revision,
    )
    .await
    .expect("store semantic vector");
    embeddings::upsert(
        db.pool(),
        opposite.id,
        &opposite_vector,
        "search-test-model",
        opposite.content_revision,
    )
    .await
    .expect("store opposite vector");

    let hits = search::hybrid_for_user(
        db.pool(),
        &search::HybridSearch {
            user_id: reader,
            query: "quasar",
            query_vector: &query_vector,
            model: "search-test-model",
            text_config: SearchTextConfig::Simple,
            semantic_weight: 0.6,
            limit: 20,
        },
    )
    .await
    .expect("hybrid search");
    assert!(hits.iter().any(|hit| {
        hit.id == keyword_only.id && hit.keyword_score.is_some() && hit.semantic_score.is_none()
    }));
    assert!(hits.iter().any(|hit| {
        hit.id == semantic.id && hit.semantic_score == Some(1.0) && hit.keyword_score.is_none()
    }));
    let opposite_hit = hits.iter().find(|hit| hit.id == opposite.id).unwrap();
    assert_eq!(opposite_hit.semantic_score, Some(0.0));
}
