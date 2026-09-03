use rusty_feed_core::feed_content::{
    ContentLifetime, FeedContentMode, SubscriptionContentSettings,
};
use rusty_feed_db::models::NewArticle;
use rusty_feed_db::{articles, catalog, categories, feeds, subscriptions, users, Db};
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
    assert!(starters.len() >= 200);
    let verified_seed_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM feed_catalog_entries WHERE sort_order BETWEEN 980 AND 2000",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(verified_seed_count, 103);
    assert!(starters.iter().any(|entry| entry.title == "Go Blog"));
    assert!(starters
        .iter()
        .any(|entry| entry.title == "Al Jazeera English"));
    for (country, language, title) in [
        ("Brazil", "pt", "Agência Brasil — Últimas"),
        ("South Africa", "en", "GroundUp News"),
        ("Spain", "es", "Xataka"),
        ("Argentina", "es", "Chequeado"),
        ("Germany", "de", "netzpolitik.org"),
        ("Hungary", "hu", "Telex — Legfrissebb"),
        ("Poland", "pl", "OKO.press"),
        ("Russia", "ru", "Meduza"),
        ("Brazil", "pt", "Folha de S.Paulo (Em Cima da Hora)"),
        ("South Africa", "en", "Daily Maverick"),
        ("Poland", "pl", "Newsweek Polska"),
        ("Hungary", "en", "Daily News Hungary (English)"),
        ("Ireland", "en", "TheJournal.ie"),
        ("Global", "en", "Hugging Face Blog"),
        ("Global", "en", "ScienceDaily"),
        ("Global", "en", "xkcd"),
        ("Global", "en", "Financial Times"),
        ("Global", "en", "Coursera Blog"),
    ] {
        assert!(
            starters.iter().any(|entry| {
                entry.country == country && entry.language == language && entry.title == title
            }),
            "missing curated source for {country}"
        );
    }

    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let bodies = ["short", "medium length", "longer RSS body"];
    let mut created = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        let (article, _) = articles::upsert(
            pool,
            &NewArticle {
                feed_id: feed.id,
                url: format!("https://catalog.example/{}/{index}", Uuid::now_v7()),
                comments_url: None,
                guid: Some(unique("guid")),
                title: format!("Article {index}"),
                author: None,
                content: (*body).to_string(),
                excerpt: None,
                image_url: None,
                enclosures: Vec::new(),
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
        ContentLifetime::Days(30),
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

    let subscription = catalog::subscribe_published_many_with_content(
        pool,
        user.id,
        &[catalog::CatalogSubscriptionChoice {
            feed_id: feed.id,
            content: SubscriptionContentSettings {
                mode: FeedContentMode::ArticlePage,
                lifetime: ContentLifetime::Days(30),
            },
        }],
    )
    .await
    .unwrap();
    assert_eq!(subscription.created, 1);
    assert!(subscriptions::is_subscribed(pool, user.id, feed.id)
        .await
        .unwrap());
    assert_eq!(
        subscriptions::content_settings_for_user(pool, user.id, feed.id)
            .await
            .unwrap()
            .unwrap()
            .mode,
        FeedContentMode::ArticlePage
    );
    let repeated = catalog::subscribe_published_many(pool, user.id, &[feed.id])
        .await
        .unwrap();
    assert_eq!(repeated.created, 0);
    assert_eq!(
        subscriptions::content_settings_for_user(pool, user.id, feed.id)
            .await
            .unwrap()
            .unwrap()
            .mode,
        FeedContentMode::ArticlePage
    );

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

#[tokio::test]
async fn subscribing_organizes_new_feeds_by_category_without_disturbing_existing_ones() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let category = unique("Organize category");
    let country = unique("Organize country");
    let language = unique("organize-lang");
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://catalog.example/{}/organize.xml", Uuid::now_v7()),
        "Organize feed",
        None,
    )
    .await
    .unwrap();
    catalog::upsert(pool, feed.id, draft(&category, &country, &language, true))
        .await
        .unwrap();
    let (already_subscribed_feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://catalog.example/{}/already.xml", Uuid::now_v7()),
        "Already subscribed feed",
        None,
    )
    .await
    .unwrap();
    catalog::upsert(
        pool,
        already_subscribed_feed.id,
        draft(&category, &country, &language, true),
    )
    .await
    .unwrap();
    let user = users::create(
        pool,
        &format!("{}@example.com", unique("organize-user")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();

    // The user already subscribed to one of the two feeds earlier and filed it
    // into their own folder/tags by hand.
    let manual_folder = categories::create(pool, user.id, "My own folder", None)
        .await
        .unwrap();
    catalog::subscribe_published_many(pool, user.id, &[already_subscribed_feed.id])
        .await
        .unwrap();
    subscriptions::update_organization(
        pool,
        user.id,
        already_subscribed_feed.id,
        &["hand-picked".to_string()],
        Some(manual_folder.id),
    )
    .await
    .unwrap();

    // Now they subscribe to both feeds together (one brand-new, one already
    // subscribed) — e.g. selecting curated suggestions in the wizard/catalog.
    let result =
        catalog::subscribe_published_many(pool, user.id, &[feed.id, already_subscribed_feed.id])
            .await
            .unwrap();
    assert_eq!(result.created, 1);
    assert_eq!(result.created_feed_ids, vec![feed.id]);
    catalog::organize_new_subscriptions(pool, user.id, &result.created_feed_ids)
        .await
        .unwrap();

    // Mirrors the 40-char cap `organize_new_subscriptions` applies to the
    // category before using it as a folder name/tag.
    let expected_category: String = category.chars().take(40).collect();
    let organization = subscriptions::list_organization(pool, user.id)
        .await
        .unwrap();
    let new_one = organization
        .iter()
        .find(|entry| entry.feed_id == feed.id)
        .unwrap();
    assert_eq!(new_one.tags, vec![expected_category.clone()]);
    let new_folder = categories::find_by_name(pool, user.id, &expected_category)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new_one.category_id, Some(new_folder.id));

    // The already-subscribed feed's own manual organization must be untouched.
    let existing_one = organization
        .iter()
        .find(|entry| entry.feed_id == already_subscribed_feed.id)
        .unwrap();
    assert_eq!(existing_one.tags, vec!["hand-picked".to_string()]);
    assert_eq!(existing_one.category_id, Some(manual_folder.id));
}

#[tokio::test]
async fn content_probe_uses_only_current_three_rss_bodies() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let category = unique("Probe category");
    let country = unique("Probe country");
    let language = unique("probe-lang");
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://catalog.example/{}/probe.xml", Uuid::now_v7()),
        "Probe feed",
        None,
    )
    .await
    .unwrap();
    catalog::upsert(pool, feed.id, draft(&category, &country, &language, true))
        .await
        .unwrap();

    let base = OffsetDateTime::from_unix_timestamp(1_710_000_000).unwrap();
    let bodies = ["old", "second", "third-body", "newest-body"];
    let mut created = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        let (article, _) = articles::upsert(
            pool,
            &NewArticle {
                feed_id: feed.id,
                url: format!("https://catalog.example/{}/probe/{index}", Uuid::now_v7()),
                comments_url: None,
                guid: Some(unique("probe-guid")),
                title: format!("Probe article {index}"),
                author: None,
                content: (*body).to_string(),
                excerpt: None,
                image_url: None,
                enclosures: Vec::new(),
                published_at: Some(base + Duration::hours(index as i64)),
            },
        )
        .await
        .unwrap();
        created.push(article);
    }
    articles::set_extracted_content(
        pool,
        created[3].id,
        created[3].content_revision,
        "A long stored public-page body must not become an RSS probe measurement.",
        None,
        ContentLifetime::Days(30),
    )
    .await
    .unwrap();

    let candidates = catalog::content_probe_candidates(pool, feed.id)
        .await
        .unwrap();
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>(),
        vec![created[3].id, created[2].id, created[1].id]
    );
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.rss_body_chars)
            .collect::<Vec<_>>(),
        vec![
            Some(bodies[3].len() as i32),
            Some(bodies[2].len() as i32),
            Some(bodies[1].len() as i32),
        ]
    );

    let samples = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| catalog::ContentProbeWrite {
            article_id: candidate.id,
            rss_body_chars: candidate.rss_body_chars,
            processed_body_chars: [Some(600), Some(300), None][index],
            status: [
                catalog::ContentProbeStatus::Full,
                catalog::ContentProbeStatus::Partial,
                catalog::ContentProbeStatus::Http451,
            ][index],
        })
        .collect::<Vec<_>>();
    catalog::replace_content_probe(pool, feed.id, &candidates, &samples)
        .await
        .unwrap();

    let stats = catalog::content_probe_stats(pool, feed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stats.sampled_article_count, 3);
    assert_eq!(stats.full_content_count, 1);
    assert_eq!(stats.partial_content_count, 1);
    assert_eq!(stats.http_451_count, 1);
    assert_eq!(stats.average_processed_body_chars, Some(450));
    assert_eq!(
        stats.average_rss_body_chars,
        Some(((bodies[3].len() + bodies[2].len() + bodies[1].len()) / 3) as i32)
    );
    assert!(!catalog::probe_needs_refresh(pool, feed.id).await.unwrap());

    let (newest, _) = articles::upsert(
        pool,
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://catalog.example/{}/newest", Uuid::now_v7()),
            comments_url: None,
            guid: Some(unique("probe-newest-guid")),
            title: "Newer probe article".to_string(),
            author: None,
            content: "brand-new RSS body".to_string(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: Some(base + Duration::hours(10)),
        },
    )
    .await
    .unwrap();
    assert_ne!(newest.id, created[3].id);
    assert!(catalog::probe_needs_refresh(pool, feed.id).await.unwrap());
}
