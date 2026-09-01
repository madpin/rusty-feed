//! M11 integration tests: saved-search scan + match. Require `TEST_DATABASE_URL`.

use std::time::Duration;

use serde_json::json;
use uuid::Uuid;

use rusty_feed_agents::{Agent, AgentContext, SavedSearchMatchAgent, SavedSearchScanAgent};
use rusty_feed_db::models::NewArticle;
use rusty_feed_db::{
    articles, embeddings, feeds, jobs, notifications, saved_search_matches, saved_searches,
    subscriptions, users, Db,
};
use rusty_feed_embed::{Embedder, HashEmbedder};
use rusty_feed_feeds::SafeFetchClient;

async fn connect() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn ctx(db: Db) -> AgentContext {
    AgentContext::new(db, SafeFetchClient::new(Duration::from_secs(5)).unwrap())
}

async fn add_embedded_article(db: &Db, feed_id: Uuid, title: &str, content: &str) -> Uuid {
    let (a, _) = articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id,
            url: format!("https://ex.com/{}", Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: title.into(),
            author: None,
            content: content.into(),
            excerpt: Some(content.into()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    let e = HashEmbedder::default();
    let v = e.embed_one(&format!("{title} {content}")).await.unwrap();
    embeddings::upsert(db.pool(), a.id, &v, e.model(), a.content_revision)
        .await
        .unwrap();
    a.id
}

#[tokio::test]
async fn match_agent_finds_relevant_articles_and_notifies() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let user = users::create(
        db.pool(),
        &format!("ss-{}@ex.com", Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap()
    .id;
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://ex.com/{}.xml", Uuid::now_v7()),
        "F",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(db.pool(), user, feed.id, None)
        .await
        .unwrap();

    let rust = add_embedded_article(
        &db,
        feed.id,
        "Async Rust",
        "concurrency async await rust programming tokio",
    )
    .await;
    let _pasta =
        add_embedded_article(&db, feed.id, "Pasta", "italian pasta tomato cooking recipe").await;

    // Query requires both rust and async; notify at a low threshold for the test.
    let ss = saved_searches::create(db.pool(), user, "Rust async", "rust +async", 0.3, true, 0.5)
        .await
        .unwrap();

    let stats = SavedSearchMatchAgent::default()
        .run(&ctx(db.clone()), json!({ "saved_search_id": ss.id }))
        .await
        .unwrap();
    assert!(stats["new_matches"].as_i64().unwrap() >= 1);

    // Only the Rust article matched (Pasta fails the boolean filter).
    let matches = saved_search_matches::list_for_search(db.pool(), ss.id, 10, 0)
        .await
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, rust);

    // A notification was created (score ≥ notify_threshold).
    assert!(notifications::unread_count(db.pool(), user).await.unwrap() >= 1);

    // Re-running is idempotent: no new matches, no duplicate notifications.
    let again = SavedSearchMatchAgent::default()
        .run(&ctx(db.clone()), json!({ "saved_search_id": ss.id }))
        .await
        .unwrap();
    assert_eq!(again["new_matches"].as_i64().unwrap(), 0);
    assert_eq!(
        notifications::unread_count(db.pool(), user).await.unwrap(),
        1
    );
}

#[tokio::test]
async fn scan_enqueues_one_match_job_per_search() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let user = users::create(
        db.pool(),
        &format!("sc-{}@ex.com", Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap()
    .id;
    let ss = saved_searches::create(db.pool(), user, "Q", "rust", 0.6, false, 0.85)
        .await
        .unwrap();

    SavedSearchScanAgent
        .run(&ctx(db.clone()), json!({}))
        .await
        .unwrap();

    let dedup = format!("saved_search_match:{}", ss.id);
    assert_eq!(
        jobs::count_active_by_dedup(db.pool(), &dedup)
            .await
            .unwrap(),
        1
    );
}
