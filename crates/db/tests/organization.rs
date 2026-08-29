use rustss_db::{articles, categories, feeds, read_articles, subscriptions, users, Db, NewArticle};

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
            published_at: None,
        },
    )
    .await
    .unwrap();
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
