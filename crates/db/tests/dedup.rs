//! Integration tests for the multi-user deduplication invariants (M2).
//!
//! These require a Postgres+pgvector instance. They run when `TEST_DATABASE_URL`
//! is set (CI provides one; locally: `docker compose --env-file .env -f
//! docker/compose.yaml up -d postgres` then export it) and are skipped otherwise
//! so `cargo test` stays green without a database.
//!
//! Isolation: each test uses unique URLs/emails and asserts on URL-scoped
//! queries, so tests can share one database and run in parallel.

use rusty_feed_core::feed_content::{
    ContentLifetime, SubscriptionContentMode, SubscriptionContentSettings,
};
use rusty_feed_db::models::{NewArticle, NewArticleEnclosure};
use rusty_feed_db::{articles, bookmarks, feeds, jobs, subscriptions, users, Db, UpsertAction};
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
        comments_url: None,
        guid: Some(unique("guid")),
        title: "Original title".into(),
        author: Some("Reporter".into()),
        content: "Body v1".into(),
        excerpt: None,
        image_url: None,
        enclosures: Vec::new(),
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
async fn retention_uses_configured_age_and_preserves_every_users_bookmarks() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let user = users::create(pool, &unique("retention@x.com"), None, None, "user", None)
        .await
        .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://retention.example/{}.xml", unique("feed")),
        "Retention",
        None,
    )
    .await
    .unwrap();
    let old_date = time::OffsetDateTime::now_utc() - time::Duration::days(36_600);
    let make = |suffix: &str| NewArticle {
        feed_id: feed.id,
        url: format!("https://retention.example/{suffix}"),
        comments_url: None,
        guid: Some(unique(suffix)),
        title: suffix.to_string(),
        author: None,
        content: "Stored RSS content".to_string(),
        excerpt: None,
        image_url: None,
        enclosures: Vec::new(),
        published_at: Some(old_date),
    };
    let (saved, _) = articles::upsert(pool, &make(&unique("saved")))
        .await
        .unwrap();
    let (expired, _) = articles::upsert(pool, &make(&unique("expired")))
        .await
        .unwrap();
    bookmarks::add(pool, user.id, saved.id).await.unwrap();

    assert!(
        articles::delete_expired_unbookmarked(
            pool,
            36_500,
            rusty_feed_core::config::RetentionAgeBasis::Published,
        )
        .await
        .unwrap()
            >= 1
    );
    assert!(articles::find_by_id(pool, saved.id)
        .await
        .unwrap()
        .is_some());
    assert!(articles::find_by_id(pool, expired.id)
        .await
        .unwrap()
        .is_none());

    bookmarks::remove(pool, user.id, saved.id).await.unwrap();
    assert!(
        articles::delete_expired_unbookmarked(
            pool,
            36_500,
            rusty_feed_core::config::RetentionAgeBasis::Published,
        )
        .await
        .unwrap()
            >= 1
    );
}

#[tokio::test]
async fn tracking_pixel_reports_are_shared_and_source_specific() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://example.com/{}.xml", unique("privacy-feed")),
        "Privacy feed",
        None,
    )
    .await
    .unwrap();
    let (article, _) = articles::upsert(
        pool,
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://example.com/{}", unique("privacy-article")),
            comments_url: None,
            guid: None,
            title: "Privacy report".to_string(),
            author: None,
            content: r#"<p>RSS body</p><img src="/pixel.gif" width="1" height="1">"#.to_string(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(article.feed_blocked_tracking_pixels, 1);
    assert_eq!(article.article_page_blocked_tracking_pixels, 0);

    articles::set_extracted_content_with_privacy(
        pool,
        article.id,
        article.content_revision,
        "<p>Public page</p>",
        None,
        ContentLifetime::Days(30),
        3,
    )
    .await
    .unwrap()
    .unwrap();
    let stored = articles::find_by_id(pool, article.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.feed_blocked_tracking_pixels, 1);
    assert_eq!(stored.article_page_blocked_tracking_pixels, 3);
}

#[tokio::test]
async fn canonical_url_dedup_preserves_and_updates_publisher_provenance() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://privacy.example/{}.xml", unique("feed")),
        "Privacy",
        None,
    )
    .await
    .unwrap();
    let canonical = format!("https://privacy.example/post/{}", unique("article"));
    let incoming = NewArticle {
        feed_id: feed.id,
        url: canonical.clone(),
        comments_url: None,
        guid: Some(unique("privacy-guid")),
        title: "Private link".into(),
        author: None,
        content: "Body".into(),
        excerpt: None,
        image_url: None,
        enclosures: Vec::new(),
        published_at: None,
    };
    let first_original = format!("{canonical}?utm_source=first");
    let (created, action) =
        articles::upsert_with_original_url(pool, &incoming, Some(&first_original))
            .await
            .unwrap();
    assert_eq!(action, UpsertAction::Created);
    assert_eq!(created.url, canonical);
    assert_eq!(created.original_url, first_original);

    let second_original = format!("{}?utm_source=second", incoming.url);
    let (updated, action) =
        articles::upsert_with_original_url(pool, &incoming, Some(&second_original))
            .await
            .unwrap();
    assert_eq!(action, UpsertAction::Updated);
    assert_eq!(updated.id, created.id);
    assert_eq!(updated.original_url, second_original);
    assert_eq!(
        articles::count_by_url(pool, &incoming.url).await.unwrap(),
        1
    );
}

#[tokio::test]
async fn enclosure_metadata_is_shared_ordered_and_idempotently_replaced() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://media.example/{}.xml", unique("feed")),
        "Media",
        None,
    )
    .await
    .unwrap();
    let mut incoming = NewArticle {
        feed_id: feed.id,
        url: format!("https://media.example/{}", unique("episode")),
        comments_url: None,
        guid: Some(unique("episode-guid")),
        title: "Episode".into(),
        author: None,
        content: "Show notes".into(),
        excerpt: None,
        image_url: None,
        enclosures: vec![
            NewArticleEnclosure {
                url: "https://cdn.example/episode.mp3".into(),
                media_kind: "audio".into(),
                mime_type: Some("audio/mpeg".into()),
                title: Some("Full episode".into()),
                size_bytes: Some(42_000),
                duration_seconds: Some(600),
                width: None,
                height: None,
            },
            NewArticleEnclosure {
                url: "https://cdn.example/poster.webp".into(),
                media_kind: "image".into(),
                mime_type: Some("image/webp".into()),
                title: Some("Episode artwork".into()),
                size_bytes: None,
                duration_seconds: None,
                width: Some(800),
                height: Some(450),
            },
        ],
        published_at: None,
    };

    let (article, created) = articles::upsert(pool, &incoming).await.unwrap();
    assert_eq!(created, UpsertAction::Created);
    let stored = articles::list_enclosures(pool, article.id).await.unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].position, 0);
    assert_eq!(stored[0].duration_seconds, Some(600));
    assert_eq!(stored[1].position, 1);

    let (_, skipped) = articles::upsert(pool, &incoming).await.unwrap();
    assert_eq!(skipped, UpsertAction::Skipped);

    incoming.enclosures.remove(0);
    let (_, updated) = articles::upsert(pool, &incoming).await.unwrap();
    assert_eq!(updated, UpsertAction::Updated);
    let replaced = articles::list_enclosures(pool, article.id).await.unwrap();
    assert_eq!(replaced.len(), 1);
    assert_eq!(replaced[0].position, 0);
    assert_eq!(replaced[0].media_kind, "image");
}

#[tokio::test]
async fn attachment_only_articles_do_not_queue_public_page_extraction() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://media.example/{}.json", unique("attachment-feed")),
        "Attachment feed",
        None,
    )
    .await
    .unwrap();
    let (article, _) = articles::upsert(
        pool,
        &NewArticle {
            feed_id: feed.id,
            url: format!("urn:rusty-feed:article:{}:{}", feed.id, unique("episode")),
            comments_url: None,
            guid: Some(unique("attachment-guid")),
            title: "Attachment only".into(),
            author: None,
            content: "Show notes".into(),
            excerpt: None,
            image_url: None,
            enclosures: vec![NewArticleEnclosure {
                url: "https://cdn.example/episode.mp3".into(),
                media_kind: "audio".into(),
                mime_type: Some("audio/mpeg".into()),
                title: None,
                size_bytes: None,
                duration_seconds: None,
                width: None,
                height: None,
            }],
            published_at: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        jobs::enqueue_extractions_for_feed(pool, feed.id, 10)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        jobs::count_active_by_dedup(
            pool,
            &format!("extract:{}:{}", article.id, article.content_revision)
        )
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn extracted_content_survives_unchanged_refresh_and_stale_write_is_rejected() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://example.com/{}.xml", unique("feed")),
        "Example",
        None,
    )
    .await
    .unwrap();
    let mut incoming = NewArticle {
        feed_id: feed.id,
        url: format!("https://example.com/{}", unique("article")),
        comments_url: None,
        guid: Some(unique("guid")),
        title: "Source title".to_string(),
        author: None,
        content: "RSS fallback v1".to_string(),
        excerpt: Some("RSS excerpt v1".to_string()),
        image_url: None,
        enclosures: Vec::new(),
        published_at: None,
    };
    let (created, _) = articles::upsert(pool, &incoming).await.unwrap();
    let extracted_revision = articles::set_extracted_content(
        pool,
        created.id,
        created.content_revision,
        "Full public page body",
        Some("Full excerpt"),
        ContentLifetime::Days(30),
    )
    .await
    .unwrap()
    .unwrap();

    assert!(articles::set_summary(
        pool,
        created.id,
        extracted_revision,
        "Old summary",
        &[],
        &[],
        "test-model",
    )
    .await
    .unwrap());

    let (unchanged, action) = articles::upsert(pool, &incoming).await.unwrap();
    assert_eq!(action, UpsertAction::Skipped);
    assert_eq!(unchanged.content, "Full public page body");
    assert_eq!(unchanged.content_source, "article_page");
    assert_eq!(unchanged.feed_content.as_deref(), Some("RSS fallback v1"));
    incoming.comments_url = Some("https://news.ycombinator.com/item?id=42".to_string());
    let (metadata_only, action) = articles::upsert(pool, &incoming).await.unwrap();
    assert_eq!(action, UpsertAction::Updated);
    assert_eq!(metadata_only.content, "Full public page body");
    assert_eq!(metadata_only.content_revision, extracted_revision);
    assert_eq!(
        metadata_only.comments_url.as_deref(),
        Some("https://news.ycombinator.com/item?id=42")
    );
    assert!(articles::get_summary(pool, metadata_only.id)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        articles::set_extracted_content(
            pool,
            created.id,
            created.content_revision,
            "Stale page body",
            None,
            ContentLifetime::Days(30),
        )
        .await
        .unwrap(),
        None
    );

    incoming.content = "RSS fallback v2".to_string();
    let (changed, action) = articles::upsert(pool, &incoming).await.unwrap();
    assert_eq!(action, UpsertAction::Updated);
    assert_eq!(changed.content, "RSS fallback v2");
    assert_eq!(changed.content_source, "feed");
    assert_eq!(changed.extraction_status, "none");
    assert_eq!(changed.feed_content.as_deref(), Some("RSS fallback v2"));
    assert!(changed.content_revision > extracted_revision);
    assert!(articles::get_summary(pool, changed.id)
        .await
        .unwrap()
        .is_none());
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
            comments_url: None,
            guid: None,
            title: "Post".into(),
            author: None,
            content: "content".into(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
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

#[tokio::test]
async fn subscriber_page_requests_merge_to_one_longest_shared_lifetime() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://example.com/{}.xml", unique("lifetime")),
        "Lifetime feed",
        None,
    )
    .await
    .unwrap();
    let first = users::create(
        pool,
        &format!("{}@example.com", unique("lifetime-a")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let second = users::create(
        pool,
        &format!("{}@example.com", unique("lifetime-b")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(pool, first.id, feed.id, None)
        .await
        .unwrap();
    subscriptions::subscribe(pool, second.id, feed.id, None)
        .await
        .unwrap();
    subscriptions::update_content_settings(
        pool,
        first.id,
        feed.id,
        SubscriptionContentSettings {
            mode: SubscriptionContentMode::ArticlePage,
            lifetime: ContentLifetime::Days(7),
        },
    )
    .await
    .unwrap();
    subscriptions::update_content_settings(
        pool,
        second.id,
        feed.id,
        SubscriptionContentSettings {
            mode: SubscriptionContentMode::ArticlePage,
            lifetime: ContentLifetime::Infinite,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        subscriptions::requested_article_page_lifetime(pool, feed.id)
            .await
            .unwrap(),
        Some(ContentLifetime::Infinite)
    );
    subscriptions::update_content_settings(
        pool,
        second.id,
        feed.id,
        SubscriptionContentSettings {
            mode: SubscriptionContentMode::Live,
            lifetime: ContentLifetime::Infinite,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        subscriptions::requested_article_page_lifetime(pool, feed.id)
            .await
            .unwrap(),
        Some(ContentLifetime::Days(7))
    );
    subscriptions::update_content_settings(
        pool,
        first.id,
        feed.id,
        SubscriptionContentSettings {
            mode: SubscriptionContentMode::Live,
            lifetime: ContentLifetime::Days(7),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        subscriptions::requested_article_page_lifetime(pool, feed.id)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn public_page_lifetime_expires_only_page_copy_and_infinite_is_retained() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://example.com/{}.xml", unique("expiry")),
        "Expiry feed",
        None,
    )
    .await
    .unwrap();
    let (article, _) = articles::upsert(
        pool,
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://example.com/{}", unique("expiry-article")),
            comments_url: None,
            guid: None,
            title: "Expiry article".into(),
            author: None,
            content: "RSS survives".into(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    articles::set_extracted_content(
        pool,
        article.id,
        article.content_revision,
        "Finite public page",
        None,
        ContentLifetime::Days(1),
    )
    .await
    .unwrap()
    .unwrap();
    sqlx::query(
        "UPDATE articles SET article_page_expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(article.id)
    .execute(pool)
    .await
    .unwrap();
    articles::clear_expired_article_page_content(pool)
        .await
        .unwrap();
    let restored = articles::find_by_id(pool, article.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.content, "RSS survives");
    assert!(restored.article_page_content.is_none());

    articles::set_extracted_content(
        pool,
        article.id,
        restored.content_revision,
        "Infinite public page",
        None,
        ContentLifetime::Infinite,
    )
    .await
    .unwrap()
    .unwrap();
    articles::clear_expired_article_page_content(pool)
        .await
        .unwrap();
    let retained = articles::find_by_id(pool, article.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retained.article_page_content.as_deref(),
        Some("Infinite public page")
    );
    assert!(retained.article_page_expires_at.is_none());
    assert_eq!(retained.feed_content.as_deref(), Some("RSS survives"));

    sqlx::query("UPDATE articles SET article_page_processing_version = 0 WHERE id = $1")
        .bind(article.id)
        .execute(pool)
        .await
        .unwrap();
    let stale = articles::list_needing_extraction(pool, feed.id, 10)
        .await
        .unwrap();
    assert!(stale.iter().any(|candidate| candidate.id == article.id));
    articles::set_extracted_content(
        pool,
        article.id,
        retained.content_revision,
        "Reprocessed infinite page",
        None,
        ContentLifetime::Infinite,
    )
    .await
    .unwrap()
    .unwrap();
    let reprocessed = articles::find_by_id(pool, article.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reprocessed.article_page_processing_version,
        rusty_feed_core::html::ARTICLE_PAGE_PROCESSING_VERSION
    );
    assert_eq!(
        reprocessed.article_page_content.as_deref(),
        Some("Reprocessed infinite page")
    );
}

#[tokio::test]
async fn extraction_backfill_prioritizes_untried_content_and_skips_recent_failures() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://example.com/{}.xml", unique("backfill")),
        "Backfill feed",
        None,
    )
    .await
    .unwrap();
    let mut article_ids = Vec::new();
    for label in ["untried", "recent-failure"] {
        let (article, _) = articles::upsert(
            pool,
            &NewArticle {
                feed_id: feed.id,
                url: format!(
                    "https://example.com/{}/{}",
                    unique("backfill-article"),
                    label
                ),
                comments_url: None,
                guid: None,
                title: label.to_string(),
                author: None,
                content: format!("RSS body {label}"),
                excerpt: None,
                image_url: None,
                enclosures: Vec::new(),
                published_at: None,
            },
        )
        .await
        .unwrap();
        article_ids.push(article.id);
    }
    let old_failure_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO articles \
         (id, feed_id, url, original_url, title, content, content_hash, feed_content, \
          feed_content_hash, extraction_status, created_at, updated_at) \
         VALUES ($1, $2, $3, $3, 'old-failure', 'RSS body old-failure', $4, \
                 'RSS body old-failure', $4, 'failed', \
                 now() - interval '20 minutes', now() - interval '20 minutes')",
    )
    .bind(old_failure_id)
    .bind(feed.id)
    .bind(format!(
        "https://example.com/{}",
        unique("backfill-old-failure")
    ))
    .bind(rusty_feed_core::hash::content_hash("RSS body old-failure"))
    .execute(pool)
    .await
    .unwrap();
    article_ids.insert(1, old_failure_id);
    sqlx::query("UPDATE articles SET extraction_status = 'failed' WHERE id = $1")
        .bind(article_ids[2])
        .execute(pool)
        .await
        .unwrap();

    assert_eq!(
        jobs::enqueue_extractions_for_feed(pool, feed.id, 3)
            .await
            .unwrap(),
        2
    );
    let queued: Vec<Uuid> = sqlx::query_scalar(
        "SELECT (payload ->> 'article_id')::uuid FROM job_queue \
         WHERE kind = 'extract_content' AND (payload ->> 'article_id')::uuid = ANY($1)",
    )
    .bind(&article_ids)
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(queued.contains(&article_ids[0]));
    assert!(queued.contains(&article_ids[1]));
    assert!(!queued.contains(&article_ids[2]));

    sqlx::query(
        "DELETE FROM job_queue WHERE kind = 'extract_content' \
         AND (payload ->> 'article_id')::uuid = ANY($1)",
    )
    .bind(&article_ids)
    .execute(pool)
    .await
    .unwrap();
}
