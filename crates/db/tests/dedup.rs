//! Integration tests for the multi-user deduplication invariants (M2).
//!
//! These require a Postgres+pgvector instance. They run when `TEST_DATABASE_URL`
//! is set (CI provides one; locally: `docker compose -f docker/compose.yaml up -d
//! postgres` then export it) and are skipped otherwise so `cargo test` stays
//! green without a database.
//!
//! Isolation: each test uses unique URLs/emails and asserts on URL-scoped
//! queries, so tests can share one database and run in parallel.

use rustss_db::models::NewArticle;
use rustss_db::{articles, feeds, subscriptions, users, Db, UpsertAction};
use uuid::Uuid;

/// Connect + migrate (idempotent) if a test DB is configured; else `None`.
async fn test_db() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url)
        .await
        .expect("connect to TEST_DATABASE_URL");
    db.migrate().await.expect("run migrations");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::now_v7())
}

#[tokio::test]
async fn feed_shared_across_users_single_row() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let url = format!("https://example.com/{}.xml", unique("feed"));

    // Two users each "add" the same feed URL.
    let alice = users::create(pool, &unique("alice@x.com"), None, None, "user", None)
        .await
        .unwrap();
    let bob = users::create(pool, &unique("bob@x.com"), None, None, "user", None)
        .await
        .unwrap();

    let (feed1, created1) = feeds::upsert_by_url(pool, &url, "Example", None)
        .await
        .unwrap();
    let (feed2, created2) = feeds::upsert_by_url(pool, &url, "Example (again)", None)
        .await
        .unwrap();

    // FR-DEDUP-1: exactly one shared feed row; only the first call created it.
    assert!(created1, "first upsert should create");
    assert!(!created2, "second upsert must not create a duplicate");
    assert_eq!(feed1.id, feed2.id);
    assert_eq!(feeds::count_by_url(pool, &url).await.unwrap(), 1);

    // Two subscriptions to the one shared feed.
    let (_s1, sub_created1) = subscriptions::subscribe(pool, alice.id, feed1.id, None)
        .await
        .unwrap();
    let (_s2, sub_created2) = subscriptions::subscribe(pool, bob.id, feed1.id, None)
        .await
        .unwrap();
    assert!(sub_created1 && sub_created2);
    assert_eq!(
        subscriptions::count_for_feed(pool, feed1.id).await.unwrap(),
        2
    );

    // Re-subscribing is idempotent.
    let (_s1b, sub_created1b) = subscriptions::subscribe(pool, alice.id, feed1.id, None)
        .await
        .unwrap();
    assert!(!sub_created1b);
    assert_eq!(
        subscriptions::count_for_feed(pool, feed1.id).await.unwrap(),
        2
    );
}

#[tokio::test]
async fn article_ingest_is_idempotent_and_updates_on_change() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();

    let feed_url = format!("https://news.example.com/{}.xml", unique("feed"));
    let (feed, _) = feeds::upsert_by_url(pool, &feed_url, "News", None)
        .await
        .unwrap();

    let article_url = format!("https://news.example.com/{}", unique("post"));
    let mut new = NewArticle {
        feed_id: feed.id,
        url: article_url.clone(),
        guid: Some(unique("guid")),
        title: "Original title".into(),
        author: Some("Reporter".into()),
        content: "Body v1".into(),
        excerpt: None,
        image_url: None,
        published_at: None,
    };

    // First ingest creates.
    let (a1, action1) = articles::upsert(pool, &new).await.unwrap();
    assert_eq!(action1, UpsertAction::Created);

    // Re-ingesting identical content is skipped (no duplicate row).
    let (a2, action2) = articles::upsert(pool, &new).await.unwrap();
    assert_eq!(action2, UpsertAction::Skipped);
    assert_eq!(a1.id, a2.id);
    assert_eq!(articles::count_by_url(pool, &article_url).await.unwrap(), 1);

    // Changing the content triggers an update of the same row.
    new.content = "Body v2 (revised)".into();
    let (a3, action3) = articles::upsert(pool, &new).await.unwrap();
    assert_eq!(action3, UpsertAction::Updated);
    assert_eq!(a3.id, a1.id);
    assert_eq!(a3.content, "Body v2 (revised)");
    assert_eq!(articles::count_by_url(pool, &article_url).await.unwrap(), 1);

    // A title change also triggers an update.
    new.title = "Revised title".into();
    let (_a4, action4) = articles::upsert(pool, &new).await.unwrap();
    assert_eq!(action4, UpsertAction::Updated);
}

#[tokio::test]
async fn unsubscribe_removes_only_personal_state() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();

    let user = users::create(pool, &unique("carol@x.com"), None, None, "user", None)
        .await
        .unwrap();
    let feed_url = format!("https://blog.example.com/{}.xml", unique("feed"));
    let (feed, _) = feeds::upsert_by_url(pool, &feed_url, "Blog", None)
        .await
        .unwrap();
    // Ingest a shared article on the feed.
    let article_url = format!("https://blog.example.com/{}", unique("post"));
    articles::upsert(
        pool,
        &NewArticle {
            feed_id: feed.id,
            url: article_url.clone(),
            guid: None,
            title: "Post".into(),
            author: None,
            content: "content".into(),
            excerpt: None,
            image_url: None,
            published_at: None,
        },
    )
    .await
    .unwrap();

    subscriptions::subscribe(pool, user.id, feed.id, None)
        .await
        .unwrap();
    assert!(subscriptions::is_subscribed(pool, user.id, feed.id)
        .await
        .unwrap());

    // Unsubscribe removes the per-user row...
    let removed = subscriptions::unsubscribe(pool, user.id, feed.id)
        .await
        .unwrap();
    assert!(removed);
    assert!(!subscriptions::is_subscribed(pool, user.id, feed.id)
        .await
        .unwrap());

    // ...but the shared feed and its article remain for other subscribers.
    assert!(feeds::find_by_id(pool, feed.id).await.unwrap().is_some());
    assert_eq!(articles::count_by_url(pool, &article_url).await.unwrap(), 1);
}
