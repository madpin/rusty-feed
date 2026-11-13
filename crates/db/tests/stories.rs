//! Shared story clustering. Requires `TEST_DATABASE_URL`.

use rusty_feed_db::models::NewArticle;
use rusty_feed_db::{articles, embeddings, feeds, stories, Db};
use time::OffsetDateTime;
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

#[tokio::test]
async fn identical_embeddings_share_one_story() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let suffix = Uuid::now_v7();
    let model: String =
        sqlx::query_scalar("SELECT model FROM instance_embedding_space WHERE singleton")
            .fetch_optional(pool)
            .await
            .unwrap()
            .unwrap_or_else(|| "story-cluster-test".to_string());
    let (left_feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://example.com/stories/{suffix}-a.xml"),
        "Gazette",
        None,
    )
    .await
    .unwrap();
    let (right_feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://example.com/stories/{suffix}-b.xml"),
        "Herald",
        None,
    )
    .await
    .unwrap();
    let published = Some(OffsetDateTime::now_utc());
    let (left, _) = articles::upsert(
        pool,
        &NewArticle {
            feed_id: left_feed.id,
            url: format!("https://example.com/stories/{suffix}-a"),
            comments_url: None,
            guid: Some(format!("story-{suffix}-a")),
            title: "Harbor closed overnight".into(),
            author: None,
            content: "<p>The harbor closed.</p>".into(),
            excerpt: Some("The harbor closed.".into()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: published,
        },
    )
    .await
    .unwrap();
    let (right, _) = articles::upsert(
        pool,
        &NewArticle {
            feed_id: right_feed.id,
            url: format!("https://example.com/stories/{suffix}-b"),
            comments_url: None,
            guid: Some(format!("story-{suffix}-b")),
            title: "Port authority shuts the harbor".into(),
            author: None,
            content: "<p>The port authority shut the harbor.</p>".into(),
            excerpt: Some("The port authority shut the harbor.".into()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: published,
        },
    )
    .await
    .unwrap();
    let (unrelated_feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://example.com/stories/{suffix}-c.xml"),
        "Ledger",
        None,
    )
    .await
    .unwrap();
    let (unrelated, _) = articles::upsert(
        pool,
        &NewArticle {
            feed_id: unrelated_feed.id,
            url: format!("https://example.com/stories/{suffix}-c"),
            comments_url: None,
            guid: Some(format!("story-{suffix}-c")),
            title: "Wheat futures rise".into(),
            author: None,
            content: "<p>Wheat futures rose.</p>".into(),
            excerpt: Some("Wheat futures rose.".into()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: published,
        },
    )
    .await
    .unwrap();
    // One-hot directions stay orthogonal to each other and to vectors left by
    // earlier runs, so only the two copies of this story clear 0.82.
    let slot = (suffix.as_u128() % 380) as usize;
    let other_slot = (slot + 200) % 384;
    let mut same = vec![0.0_f32; 384];
    same[slot] = 1.0;
    let mut other = vec![0.0_f32; 384];
    other[other_slot] = 1.0;
    embeddings::upsert(pool, left.id, &same, &model, left.content_revision)
        .await
        .unwrap();
    embeddings::upsert(pool, right.id, &same, &model, right.content_revision)
        .await
        .unwrap();
    embeddings::upsert(
        pool,
        unrelated.id,
        &other,
        &model,
        unrelated.content_revision,
    )
    .await
    .unwrap();

    assert!(stories::cluster_article(pool, unrelated.id)
        .await
        .unwrap()
        .is_none());
    let left_cluster = stories::cluster_article(pool, left.id).await.unwrap();
    let right_cluster = stories::cluster_article(pool, right.id).await.unwrap();
    assert_eq!(left_cluster, right_cluster);
    assert!(left_cluster.is_some());
    assert!(stories::cluster_article(pool, unrelated.id)
        .await
        .unwrap()
        .is_none());
}
