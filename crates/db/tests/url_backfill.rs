use rustss_core::tracking_url::TrackingUrlPolicy;
use rustss_db::{articles, feeds, url_backfill, Db, NewArticle};
use uuid::Uuid;

async fn article(db: &Db, feed_id: Uuid, url: String, title: &str) -> Uuid {
    articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id,
            url,
            comments_url: None,
            guid: Some(Uuid::now_v7().to_string()),
            title: title.to_string(),
            author: None,
            content: format!("<p>{title}</p>"),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap()
    .0
    .id
}

#[tokio::test]
async fn legacy_url_backfill_updates_only_collision_free_keys() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let scope = Uuid::now_v7();
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://backfill.example/{scope}/feed"),
        "Backfill",
        None,
    )
    .await
    .unwrap();
    let safe_id = article(
        &db,
        feed.id,
        format!("https://backfill.example/{scope}/safe?utm_source=old&id=1"),
        "safe",
    )
    .await;
    let canonical_id = article(
        &db,
        feed.id,
        format!("https://backfill.example/{scope}/collision"),
        "canonical",
    )
    .await;
    let colliding_id = article(
        &db,
        feed.id,
        format!("https://backfill.example/{scope}/collision?utm_source=old"),
        "collision",
    )
    .await;

    let plan = url_backfill::plan(db.pool(), &TrackingUrlPolicy::default())
        .await
        .unwrap();
    assert!(plan
        .changes
        .iter()
        .any(|change| change.article_id == safe_id));
    assert!(plan
        .collisions
        .iter()
        .any(|change| change.article_id == colliding_id));
    let scoped = url_backfill::UrlBackfillPlan {
        changes: plan
            .changes
            .into_iter()
            .filter(|change| change.article_id == safe_id)
            .collect(),
        collisions: Vec::new(),
        unchanged: 0,
    };
    assert_eq!(url_backfill::apply(db.pool(), &scoped).await.unwrap(), 1);
    let updated = articles::find_by_id(db.pool(), safe_id)
        .await
        .unwrap()
        .unwrap();
    assert!(updated.url.ends_with("?id=1"));
    assert_eq!(
        articles::find_by_id(db.pool(), canonical_id)
            .await
            .unwrap()
            .unwrap()
            .url,
        format!("https://backfill.example/{scope}/collision")
    );
}
