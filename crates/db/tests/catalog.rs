use rustss_db::models::NewArticle;
use rustss_db::{articles, catalog, feeds, subscriptions, users, Db};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url)
        .await
        .expect("connect to TEST_DATABASE_URL");
    db.migrate().await.expect("run migrations");
    Some(db)
}

fn unique(value: &str) -> String {
    format!("{value}-{}", Uuid::now_v7())
}

fn draft<'a>(
    category: &'a str,
    country: &'a str,
    language: &'a str,
    published: bool,
) -> catalog::CatalogDraft<'a> {
    catalog::CatalogDraft {
        title: "Test catalog source",
        description: Some("An independently curated test source."),
        category,
        country,
        language,
        featured: true,
        sort_order: 7,
        published,
    }
}

#[tokio::test]
async fn catalog_filters_subscribes_and_reports_rss_stats() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let category = unique("Category");
    let country = unique("Country");
    let language = unique("lang");
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://catalog.example/{}/feed.xml", Uuid::now_v7()),
        "Shared feed",
        None,
    )
    .await
    .unwrap();
    let entry = catalog::upsert(pool, feed.id, draft(&category, &country, &language, true))
        .await
        .unwrap();
    let user = users::create(
        pool,
        &format!("{}@example.com", unique("catalog-user")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();

    let starters = catalog::list_published_for_user(pool, user.id, None, None, None)
        .await
        .unwrap();
    assert!(starters.len() >= 41);
    assert!(starters.iter().any(|entry| entry.title == "Go Blog"));
    assert!(starters
        .iter()
        .any(|entry| entry.title == "Al Jazeera English"));

    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let bodies = ["short", "medium length", "longer RSS body"];
    let mut created = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        let (article, _) = articles::upsert(
            pool,
            &NewArticle {
                feed_id: feed.id,
                url: format!("https://catalog.example/{}/{index}", Uuid::now_v7()),
                guid: Some(unique("guid")),
                title: format!("Article {index}"),
                author: None,
                content: (*body).to_string(),
                excerpt: None,
                image_url: None,
                published_at: Some(base + Duration::hours(24 * index as i64)),
            },
        )
        .await
        .unwrap();
        created.push(article);
    }
    articles::set_extracted_content(
        pool,
        created[0].id,
        created[0].content_revision,
        "This deliberately much longer public page body must not affect RSS statistics.",
        None,
    )
    .await
    .unwrap();

    let entries = catalog::list_published_for_user(
        pool,
        user.id,
        Some(&category),
        Some(&country),
        Some(&language),
    )
    .await
    .unwrap();
    assert_eq!(entries.len(), 1);
    let listed = &entries[0];
    assert_eq!(listed.id, entry.id);
    assert_eq!(listed.article_count, 3);
    assert!(!listed.subscribed);
    let expected_average =
        bodies.iter().map(|body| body.len() as f64).sum::<f64>() / bodies.len() as f64;
    assert!((listed.average_rss_content_length.unwrap() - expected_average).abs() < 0.01);
    assert!((listed.average_interval_hours.unwrap() - 24.0).abs() < 0.01);
    assert_eq!(listed.last_post_at, Some(base + Duration::hours(48)));

    let subscription = catalog::subscribe_published_many(pool, user.id, &[feed.id])
        .await
        .unwrap();
    assert_eq!(subscription.created, 1);
    assert!(subscriptions::is_subscribed(pool, user.id, feed.id)
        .await
        .unwrap());
    let repeated = catalog::subscribe_published_many(pool, user.id, &[feed.id])
        .await
        .unwrap();
    assert_eq!(repeated.created, 0);

    let (draft_feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://catalog.example/{}/draft.xml", Uuid::now_v7()),
        "Draft feed",
        None,
    )
    .await
    .unwrap();
    let _ = catalog::upsert(
        pool,
        draft_feed.id,
        draft(&category, &country, &language, false),
    )
    .await
    .unwrap();
    assert!(
        catalog::subscribe_published_many(pool, user.id, &[draft_feed.id])
            .await
            .is_err()
    );
    assert!(!subscriptions::is_subscribed(pool, user.id, draft_feed.id)
        .await
        .unwrap());
    assert!(catalog::list_admin(pool)
        .await
        .unwrap()
        .iter()
        .any(|candidate| candidate.feed_id == draft_feed.id && !candidate.published));
    assert!(
        catalog::update(pool, entry.id, draft(&category, &country, &language, false))
            .await
            .unwrap()
    );
}
