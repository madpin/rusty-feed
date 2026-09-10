use rusty_feed_core::feed_content::{
    ContentLifetime, SubscriptionContentMode, SubscriptionContentSettings,
};
use rusty_feed_core::youtube::SubscriptionYouTubeMode;
use rusty_feed_db::{
    articles, categories, feeds, read_articles, subscriptions, users, Db, NewArticle,
};

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7())
}

async fn add_article(db: &Db, feed_id: uuid::Uuid, title: &str) {
    articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id,
            url: format!(
                "https://organization.example/{}/{}",
                uuid::Uuid::now_v7(),
                title
            ),
            comments_url: None,
            guid: None,
            title: title.to_string(),
            author: None,
            content: title.to_string(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn youtube_override_is_tenant_scoped_and_preserves_content_settings() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let user = users::create(
        db.pool(),
        &format!("{}@example.test", unique("youtube-settings")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let other = users::create(
        db.pool(),
        &format!("{}@example.test", unique("youtube-settings-other")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!(
            "https://youtube-settings.example/{}/feed",
            uuid::Uuid::now_v7()
        ),
        "Video feed",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(db.pool(), user.id, feed.id, None)
        .await
        .unwrap();
    subscriptions::subscribe(db.pool(), other.id, feed.id, None)
        .await
        .unwrap();
    subscriptions::update_content_and_youtube_settings(
        db.pool(),
        user.id,
        feed.id,
        SubscriptionContentSettings {
            mode: SubscriptionContentMode::ArticlePage,
            lifetime: ContentLifetime::Infinite,
        },
        SubscriptionYouTubeMode::LinksOnly,
    )
    .await
    .unwrap();

    assert_eq!(
        subscriptions::youtube_mode_for_user(db.pool(), user.id, feed.id)
            .await
            .unwrap(),
        Some(SubscriptionYouTubeMode::LinksOnly)
    );
    assert_eq!(
        subscriptions::youtube_mode_for_user(db.pool(), other.id, feed.id)
            .await
            .unwrap(),
        Some(SubscriptionYouTubeMode::Inherit)
    );
    let content = subscriptions::content_settings_for_user(db.pool(), user.id, feed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(content.mode, SubscriptionContentMode::ArticlePage);
    assert_eq!(content.lifetime, ContentLifetime::Infinite);
}

#[tokio::test]
async fn folders_and_tags_are_personal_single_folder_library_filters() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let alice = users::create(
        db.pool(),
        &format!("{}@example.test", unique("organization-alice")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let bob = users::create(
        db.pool(),
        &format!("{}@example.test", unique("organization-bob")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (rust_feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!(
            "https://organization.example/{}/rust.xml",
            uuid::Uuid::now_v7()
        ),
        "Rust feed",
        None,
    )
    .await
    .unwrap();
    let (web_feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!(
            "https://organization.example/{}/web.xml",
            uuid::Uuid::now_v7()
        ),
        "Web feed",
        None,
    )
    .await
    .unwrap();
    let (personal_feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!(
            "https://organization.example/{}/personal.xml",
            uuid::Uuid::now_v7()
        ),
        "Personal feed",
        None,
    )
    .await
    .unwrap();
    for feed in [&rust_feed, &web_feed, &personal_feed] {
        subscriptions::subscribe(db.pool(), alice.id, feed.id, None)
            .await
            .unwrap();
    }

    let folder = categories::create(db.pool(), alice.id, &unique("Technology"), None)
        .await
        .unwrap();
    let child = categories::create(db.pool(), alice.id, &unique("Rust"), Some(folder.id))
        .await
        .unwrap();
    let foreign = categories::create(db.pool(), bob.id, &unique("Foreign"), None)
        .await
        .unwrap();
    subscriptions::update_organization(
        db.pool(),
        alice.id,
        rust_feed.id,
        &["work".to_string()],
        Some(folder.id),
    )
    .await
    .unwrap();
    subscriptions::update_organization(
        db.pool(),
        alice.id,
        rust_feed.id,
        &["work".to_string(), "rust".to_string()],
        Some(child.id),
    )
    .await
    .unwrap();
    subscriptions::update_organization(
        db.pool(),
        alice.id,
        web_feed.id,
        &["work".to_string()],
        Some(child.id),
    )
    .await
    .unwrap();
    subscriptions::update_organization(
        db.pool(),
        alice.id,
        personal_feed.id,
        &["personal".to_string()],
        None,
    )
    .await
    .unwrap();
    assert!(subscriptions::update_organization(
        db.pool(),
        alice.id,
        rust_feed.id,
        &[],
        Some(foreign.id),
    )
    .await
    .is_err());
    assert_eq!(
        subscriptions::list_organization(db.pool(), alice.id)
            .await
            .unwrap()
            .into_iter()
            .find(|organization| organization.feed_id == rust_feed.id)
            .unwrap()
            .category_id,
        Some(child.id)
    );

    add_article(&db, rust_feed.id, "Rust article").await;
    add_article(&db, web_feed.id, "Web article").await;
    add_article(&db, personal_feed.id, "Personal article").await;

    let names = |items: Vec<articles::ArticleListItem>| {
        items
            .into_iter()
            .map(|item| item.title)
            .collect::<std::collections::HashSet<_>>()
    };
    let parent_items = articles::list_for_user(
        db.pool(),
        alice.id,
        articles::LibraryFilter {
            feed_id: None,
            category_id: Some(folder.id),
            unfiled: false,
            tag: None,
        },
        false,
        false,
        20,
        0,
    )
    .await
    .unwrap();
    assert_eq!(parent_items.len(), 2);
    assert_eq!(
        names(parent_items),
        std::collections::HashSet::from(["Rust article".to_string(), "Web article".to_string()])
    );
    let work_items = articles::list_for_user(
        db.pool(),
        alice.id,
        articles::LibraryFilter {
            feed_id: None,
            category_id: Some(folder.id),
            unfiled: false,
            tag: Some("work"),
        },
        false,
        false,
        20,
        0,
    )
    .await
    .unwrap();
    assert_eq!(
        names(work_items),
        std::collections::HashSet::from(["Rust article".to_string(), "Web article".to_string()])
    );
    let personal_items = articles::list_for_user(
        db.pool(),
        alice.id,
        articles::LibraryFilter {
            feed_id: None,
            category_id: None,
            unfiled: false,
            tag: Some("personal"),
        },
        false,
        false,
        20,
        0,
    )
    .await
    .unwrap();
    assert_eq!(
        names(personal_items),
        std::collections::HashSet::from(["Personal article".to_string()])
    );
    let unfiled_items = articles::list_for_user(
        db.pool(),
        alice.id,
        articles::LibraryFilter {
            feed_id: None,
            category_id: None,
            unfiled: true,
            tag: None,
        },
        false,
        false,
        20,
        0,
    )
    .await
    .unwrap();
    assert_eq!(
        names(unfiled_items),
        std::collections::HashSet::from(["Personal article".to_string()])
    );

    let folders = categories::list_with_feed_counts(db.pool(), alice.id)
        .await
        .unwrap();
    assert_eq!(
        folders
            .iter()
            .find(|candidate| candidate.id == folder.id)
            .unwrap()
            .feed_count,
        2
    );
    let tags = categories::list_tags(db.pool(), alice.id).await.unwrap();
    assert_eq!(
        tags.iter()
            .find(|candidate| candidate.name == "work")
            .unwrap()
            .feed_count,
        2
    );

    read_articles::mark_all_read(
        db.pool(),
        alice.id,
        None,
        Some(folder.id),
        false,
        Some("work"),
    )
    .await
    .unwrap();
    let unread_work = articles::list_for_user(
        db.pool(),
        alice.id,
        articles::LibraryFilter {
            feed_id: None,
            category_id: Some(folder.id),
            unfiled: false,
            tag: Some("work"),
        },
        true,
        false,
        20,
        0,
    )
    .await
    .unwrap();
    assert!(unread_work.is_empty());
    let unread_personal = articles::list_for_user(
        db.pool(),
        alice.id,
        articles::LibraryFilter {
            feed_id: None,
            category_id: None,
            unfiled: false,
            tag: Some("personal"),
        },
        true,
        false,
        20,
        0,
    )
    .await
    .unwrap();
    assert_eq!(unread_personal.len(), 1);
    read_articles::mark_all_read(db.pool(), alice.id, None, None, true, None)
        .await
        .unwrap();
    let unread_unfiled = articles::list_for_user(
        db.pool(),
        alice.id,
        articles::LibraryFilter {
            feed_id: None,
            category_id: None,
            unfiled: true,
            tag: None,
        },
        true,
        false,
        20,
        0,
    )
    .await
    .unwrap();
    assert!(unread_unfiled.is_empty());
}

#[tokio::test]
async fn bulk_organization_is_atomic_and_personal() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let alice = users::create(
        db.pool(),
        &format!("{}@example.test", unique("bulk-alice")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let bob = users::create(
        db.pool(),
        &format!("{}@example.test", unique("bulk-bob")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (first, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://bulk.example/{}/first.xml", uuid::Uuid::now_v7()),
        "First",
        None,
    )
    .await
    .unwrap();
    let (second, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://bulk.example/{}/second.xml", uuid::Uuid::now_v7()),
        "Second",
        None,
    )
    .await
    .unwrap();
    let (foreign, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://bulk.example/{}/foreign.xml", uuid::Uuid::now_v7()),
        "Foreign",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(db.pool(), alice.id, first.id, None)
        .await
        .unwrap();
    subscriptions::subscribe(db.pool(), alice.id, second.id, None)
        .await
        .unwrap();
    subscriptions::subscribe(db.pool(), bob.id, foreign.id, None)
        .await
        .unwrap();
    subscriptions::update_organization(
        db.pool(),
        alice.id,
        first.id,
        &["existing".to_string()],
        None,
    )
    .await
    .unwrap();
    subscriptions::update_organization(
        db.pool(),
        alice.id,
        second.id,
        &["second".to_string()],
        None,
    )
    .await
    .unwrap();
    let folder = categories::create(db.pool(), alice.id, &unique("Bulk"), None)
        .await
        .unwrap();

    assert_eq!(
        subscriptions::move_many_to_category(
            db.pool(),
            alice.id,
            &[first.id, second.id],
            Some(folder.id),
        )
        .await
        .unwrap(),
        2
    );
    assert!(
        subscriptions::all_subscribed(db.pool(), alice.id, &[first.id, second.id])
            .await
            .unwrap()
    );
    assert!(
        !subscriptions::all_subscribed(db.pool(), alice.id, &[first.id, foreign.id])
            .await
            .unwrap()
    );
    assert!(subscriptions::move_many_to_category(
        db.pool(),
        alice.id,
        &[first.id, foreign.id],
        None,
    )
    .await
    .is_err());

    assert_eq!(
        subscriptions::update_tags_many(
            db.pool(),
            alice.id,
            &[first.id, second.id],
            subscriptions::BulkTagOperation::Add,
            &["batch".to_string(), "existing".to_string()],
        )
        .await
        .unwrap(),
        2
    );
    assert_eq!(
        subscriptions::update_tags_many(
            db.pool(),
            alice.id,
            &[first.id, second.id],
            subscriptions::BulkTagOperation::Remove,
            &["existing".to_string()],
        )
        .await
        .unwrap(),
        2
    );
    assert_eq!(
        subscriptions::update_tags_many(
            db.pool(),
            alice.id,
            &[first.id, second.id],
            subscriptions::BulkTagOperation::Replace,
            &["final".to_string()],
        )
        .await
        .unwrap(),
        2
    );
    let organizations = subscriptions::list_organization(db.pool(), alice.id)
        .await
        .unwrap();
    for feed_id in [first.id, second.id] {
        let organization = organizations
            .iter()
            .find(|organization| organization.feed_id == feed_id)
            .unwrap();
        assert_eq!(organization.category_id, Some(folder.id));
        assert_eq!(organization.tags, vec!["final".to_string()]);
    }
}

/// A reader who queues several summaries and moves on needs a single filter
/// that finds exactly the ones now ready and not yet read — not everything
/// with a summary (including ones already read), and not everything unread
/// (including ones with no summary at all).
#[tokio::test]
async fn summary_only_filter_finds_ready_and_unread_but_nothing_else() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let alice = users::create(
        db.pool(),
        &format!("{}@example.com", unique("alice-summary")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://organization.example/{}.xml", uuid::Uuid::now_v7()),
        "Summary feed",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(db.pool(), alice.id, feed.id, None)
        .await
        .unwrap();

    async fn add(db: &Db, feed_id: uuid::Uuid, title: &str) -> rusty_feed_db::Article {
        articles::upsert(
            db.pool(),
            &NewArticle {
                feed_id,
                url: format!(
                    "https://organization.example/{}/{title}",
                    uuid::Uuid::now_v7()
                ),
                comments_url: None,
                guid: None,
                title: title.to_string(),
                author: None,
                content: title.to_string(),
                excerpt: None,
                image_url: None,
                enclosures: Vec::new(),
                published_at: None,
            },
        )
        .await
        .unwrap()
        .0
    }

    let ready_unread = add(&db, feed.id, "Ready and unread").await;
    assert!(articles::set_summary(
        db.pool(),
        ready_unread.id,
        ready_unread.content_revision,
        "A ready summary",
        &[],
        &[],
        "test-model",
    )
    .await
    .unwrap());

    let ready_read = add(&db, feed.id, "Ready but already read").await;
    assert!(articles::set_summary(
        db.pool(),
        ready_read.id,
        ready_read.content_revision,
        "Another ready summary",
        &[],
        &[],
        "test-model",
    )
    .await
    .unwrap());
    read_articles::mark_read(db.pool(), alice.id, ready_read.id)
        .await
        .unwrap();

    let _unread_no_summary = add(&db, feed.id, "Unread with no summary").await;

    // The "unread AI summaries" view combines both flags (the web layer forces
    // unread_only whenever summary_only is requested; this proves they compose
    // as independent, orthogonal predicates at the data layer).
    let filter = articles::LibraryFilter::default();
    let summary_only = articles::list_for_user(db.pool(), alice.id, filter, true, true, 20, 0)
        .await
        .unwrap();
    assert_eq!(
        summary_only
            .into_iter()
            .map(|item| item.title)
            .collect::<std::collections::HashSet<_>>(),
        std::collections::HashSet::from(["Ready and unread".to_string()])
    );

    // Without the summary filter, everything (including the read/no-summary
    // articles) is still there — the new predicate is additive, not a
    // side effect of unread-scoping alone.
    let unfiltered = articles::list_for_user(db.pool(), alice.id, filter, false, false, 20, 0)
        .await
        .unwrap();
    assert_eq!(unfiltered.len(), 3);
}
