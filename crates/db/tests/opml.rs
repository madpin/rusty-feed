use rustss_db::{categories, feeds, subscriptions, users, Db};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url)
        .await
        .expect("connect to TEST_DATABASE_URL");
    db.migrate().await.expect("run migrations");
    Some(db)
}

#[tokio::test]
async fn opml_import_preserves_sibling_scoped_paths_and_personal_isolation() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let run = Uuid::now_v7();
    let user = users::create(
        pool,
        &format!("opml-{run}@example.com"),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let other_user = users::create(
        pool,
        &format!("opml-other-{run}@example.com"),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let work = categories::create(pool, user.id, "Work", None)
        .await
        .unwrap();
    let personal = categories::create(pool, user.id, "Personal", None)
        .await
        .unwrap();
    let work_news = categories::create(pool, user.id, "News", Some(work.id))
        .await
        .unwrap();
    let personal_news = categories::create(pool, user.id, "News", Some(personal.id))
        .await
        .unwrap();
    assert_ne!(work_news.id, personal_news.id);

    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://opml-{run}.example/feed.xml"),
        "Shared feed",
        Some("https://opml.example/"),
    )
    .await
    .unwrap();
    assert_eq!(
        subscriptions::import_subscription(
            pool,
            user.id,
            feed.id,
            Some("Imported name"),
            Some(work_news.id),
            false,
        )
        .await
        .unwrap(),
        subscriptions::ImportSubscriptionOutcome::Created
    );
    assert_eq!(
        subscriptions::import_subscription(
            pool,
            user.id,
            feed.id,
            Some("Replacement name"),
            Some(personal_news.id),
            false,
        )
        .await
        .unwrap(),
        subscriptions::ImportSubscriptionOutcome::Unchanged
    );
    let organization = subscriptions::list_organization(pool, user.id)
        .await
        .unwrap()
        .into_iter()
        .find(|organization| organization.feed_id == feed.id)
        .unwrap();
    assert_eq!(organization.category_id, Some(work_news.id));

    assert_eq!(
        subscriptions::import_subscription(
            pool,
            user.id,
            feed.id,
            Some("Replacement name"),
            Some(personal_news.id),
            true,
        )
        .await
        .unwrap(),
        subscriptions::ImportSubscriptionOutcome::Updated
    );
    let subscription = subscriptions::find(pool, user.id, feed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        subscription.custom_name.as_deref(),
        Some("Replacement name")
    );
    let organization = subscriptions::list_organization(pool, user.id)
        .await
        .unwrap()
        .into_iter()
        .find(|organization| organization.feed_id == feed.id)
        .unwrap();
    assert_eq!(organization.category_id, Some(personal_news.id));
    assert_eq!(
        organization.custom_name.as_deref(),
        Some("Replacement name")
    );
    assert!(!subscriptions::is_subscribed(pool, other_user.id, feed.id)
        .await
        .unwrap());
    let urls = vec![feed.url.clone()];
    assert_eq!(
        subscriptions::subscribed_urls(pool, user.id, &urls)
            .await
            .unwrap(),
        urls
    );
    assert!(
        subscriptions::subscribed_urls(pool, other_user.id, std::slice::from_ref(&feed.url))
            .await
            .unwrap()
            .is_empty()
    );

    sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind([user.id, other_user.id])
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM feeds WHERE id = $1")
        .bind(feed.id)
        .execute(pool)
        .await
        .unwrap();
}
