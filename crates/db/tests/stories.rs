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
    let vector = vec![0.02_f32; 384];
    embeddings::upsert(pool, left.id, &vector, &model, left.content_revision)
        .await
        .unwrap();
    embeddings::upsert(pool, right.id, &vector, &model, right.content_revision)
        .await
        .unwrap();

    let left_cluster = stories::cluster_article(pool, left.id).await.unwrap();
    let right_cluster = stories::cluster_article(pool, right.id).await.unwrap();
    assert_eq!(left_cluster, right_cluster);
    assert!(left_cluster.is_some());
}
