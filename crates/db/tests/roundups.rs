//! Roundup candidate scoping against real PostgreSQL.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use rusty_feed_core::roundup::RoundupPeriod;
use rusty_feed_db::{articles, categories, feeds, roundups, subscriptions, users, Db, NewArticle};

async fn add_article(db: &Db, feed_id: Uuid, title: &str, published_at: OffsetDateTime) -> Uuid {
    articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id,
            url: format!("https://roundup.example/{}", Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: title.into(),
            author: None,
            content: format!("{title}: FULL BODY MUST NEVER ENTER THE ROUNDUP PROMPT"),
            excerpt: Some("A short shared excerpt.".into()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: Some(published_at),
        },
    )
    .await
    .unwrap()
    .0
    .id
}

#[tokio::test]
async fn folder_candidates_are_recursive_windowed_and_tenant_scoped() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url).await.unwrap();
    db.migrate().await.unwrap();
    let alice = users::create(
        db.pool(),
        &format!("roundup-alice-{}@example.test", Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let bob = users::create(
        db.pool(),
        &format!("roundup-bob-{}@example.test", Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();

    let mut feed_ids = Vec::new();
    for name in [
        "Parent shared feed",
        "Child shared feed",
        "Outside shared feed",
    ] {
        let (feed, _) = feeds::upsert_by_url(
            db.pool(),
            &format!("https://roundup.example/{}.xml", Uuid::now_v7()),
            name,
            None,
        )
        .await
        .unwrap();
        subscriptions::subscribe(
            db.pool(),
            alice.id,
            feed.id,
            Some("PERSONAL NAME MUST NOT LEAK"),
        )
        .await
        .unwrap();
        feed_ids.push(feed.id);
    }
    let parent = categories::create(db.pool(), alice.id, "Parent folder", None)
        .await
        .unwrap();
    let child = categories::create(db.pool(), alice.id, "Child folder", Some(parent.id))
        .await
        .unwrap();
    subscriptions::update_organization(db.pool(), alice.id, feed_ids[0], &[], Some(parent.id))
        .await
        .unwrap();
    subscriptions::update_organization(db.pool(), alice.id, feed_ids[1], &[], Some(child.id))
        .await
        .unwrap();

    let now = OffsetDateTime::now_utc();
    let parent_article = add_article(&db, feed_ids[0], "Parent article", now).await;
    let child_article = add_article(&db, feed_ids[1], "Child article", now).await;
    let outside_article = add_article(&db, feed_ids[2], "Outside article", now).await;
    let old_article = add_article(
        &db,
        feed_ids[0],
        "Older than a day",
        now - Duration::hours(25),
    )
    .await;

    let day = roundups::candidates_for_user(
        db.pool(),
        alice.id,
        roundups::RoundupScope::Folder(parent.id),
        RoundupPeriod::Day,
    )
    .await
    .unwrap();
    let day_ids: Vec<_> = day.iter().map(|candidate| candidate.id).collect();
    assert!(day_ids.contains(&parent_article));
    assert!(day_ids.contains(&child_article));
    assert!(!day_ids.contains(&outside_article));
    assert!(!day_ids.contains(&old_article));
    assert!(day.iter().all(|candidate| {
        candidate.feed_title.ends_with("shared feed")
            && !candidate.feed_title.contains("PERSONAL")
            && !candidate.summary_or_excerpt.contains("FULL BODY")
            && candidate.summary_or_excerpt == "A short shared excerpt."
    }));

    let week = roundups::candidates_for_user(
        db.pool(),
        alice.id,
        roundups::RoundupScope::Folder(parent.id),
        RoundupPeriod::Week,
    )
    .await
    .unwrap();
    assert!(week.iter().any(|candidate| candidate.id == old_article));

    let feed_only = roundups::candidates_for_user(
        db.pool(),
        alice.id,
        roundups::RoundupScope::Feed(feed_ids[1]),
        RoundupPeriod::Day,
    )
    .await
    .unwrap();
    assert_eq!(
        feed_only
            .iter()
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>(),
        vec![child_article]
    );
    let foreign = roundups::candidates_for_user(
        db.pool(),
        bob.id,
        roundups::RoundupScope::Folder(parent.id),
        RoundupPeriod::Day,
    )
    .await
    .unwrap();
    assert!(foreign.is_empty());

    let sources = roundups::sources_by_ids(db.pool(), &[child_article, parent_article])
        .await
        .unwrap();
    assert_eq!(sources[0].id, child_article);
    assert_eq!(sources[1].id, parent_article);
    assert_eq!(sources[0].feed_title, "Child shared feed");
}
