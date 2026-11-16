//! Feed-ingest behaviour preserved by FND-240: duplicate resolution precedence
//! (GUID -> URL -> hash), lock-free unchanged refreshes, and serialised
//! enclosure writes. Requires `TEST_DATABASE_URL`; skipped otherwise.

use rusty_feed_core::{
    hash::content_hash, html::sanitize_article_html_with_policies,
    tracking_pixel::TrackingPixelPolicy, tracking_url::TrackingUrlPolicy,
};
use rusty_feed_db::models::{NewArticle, NewArticleEnclosure, MAX_ARTICLE_ENCLOSURES};
use rusty_feed_db::{articles, feeds, Db, UpsertAction};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn make_feed(db: &Db) -> Uuid {
    feeds::upsert_by_url(
        db.pool(),
        &format!("https://ingest.example/{}.xml", Uuid::now_v7()),
        "Ingest",
        None,
    )
    .await
    .unwrap()
    .0
    .id
}

fn article(feed_id: Uuid, url: &str, guid: Option<&str>, content: &str) -> NewArticle {
    NewArticle {
        feed_id,
        url: url.to_string(),
        comments_url: None,
        guid: guid.map(str::to_string),
        title: "Ingest".into(),
        author: None,
        content: content.to_string(),
        excerpt: None,
        image_url: None,
        enclosures: Vec::new(),
        published_at: None,
    }
}

fn enclosure(url: &str) -> NewArticleEnclosure {
    NewArticleEnclosure {
        url: url.to_string(),
        media_kind: "audio".into(),
        mime_type: Some("audio/mpeg".into()),
        title: None,
        size_bytes: Some(10),
        duration_seconds: None,
        width: None,
        height: None,
    }
}

/// The pre-FND-240 `find_duplicate`: up to three sequential lookups.
async fn old_find_duplicate(db: &Db, new: &NewArticle) -> Option<Uuid> {
    let pool = db.pool();
    if let Some(guid) = new.guid.as_deref() {
        let hit: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM articles WHERE feed_id = $1 AND guid = $2")
                .bind(new.feed_id)
                .bind(guid)
                .fetch_optional(pool)
                .await
                .unwrap();
        if hit.is_some() {
            return hit;
        }
    }
    let hit: Option<Uuid> = sqlx::query_scalar("SELECT id FROM articles WHERE url = $1")
        .bind(&new.url)
        .fetch_optional(pool)
        .await
        .unwrap();
    if hit.is_some() {
        return hit;
    }
    if new.guid.is_none() {
        let sanitized = sanitize_article_html_with_policies(
            &new.content,
            &new.url,
            &TrackingUrlPolicy::default(),
            &TrackingPixelPolicy::default(),
        );
        return sqlx::query_scalar(
            "SELECT id FROM articles WHERE feed_id = $1 AND feed_content_hash = $2",
        )
        .bind(new.feed_id)
        .bind(content_hash(&sanitized.html))
        .fetch_optional(pool)
        .await
        .unwrap();
    }
    None
}

#[tokio::test]
async fn find_duplicate_matches_the_old_three_query_precedence() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let tag = Uuid::now_v7().simple().to_string();
    let url = |name: &str| format!("https://ingest.example/{tag}/{name}");
    let guid = |name: &str| format!("{tag}-{name}");
    let (feed_1, feed_2) = (make_feed(&db).await, make_feed(&db).await);

    let (a, _) = articles::upsert(
        pool,
        &article(feed_1, &url("a"), Some(&guid("g1")), "<p>alpha</p>"),
    )
    .await
    .unwrap();
    let (b, _) = articles::upsert(
        pool,
        &article(feed_1, &url("b"), Some(&guid("g2")), "<p>beta</p>"),
    )
    .await
    .unwrap();
    let (c, _) = articles::upsert(pool, &article(feed_1, &url("c"), None, "<p>gamma</p>"))
        .await
        .unwrap();
    let (d, _) = articles::upsert(
        pool,
        &article(feed_2, &url("d"), Some(&guid("g1")), "<p>delta</p>"),
    )
    .await
    .unwrap();
    for stored in [&a, &b, &c, &d] {
        assert!(stored.feed_content_hash.is_some());
    }

    let cases: Vec<(&str, NewArticle, Option<Uuid>)> = vec![
        (
            "guid match wins over a url match on another article",
            article(feed_1, &url("b"), Some(&guid("g1")), "<p>x</p>"),
            Some(a.id),
        ),
        (
            "guid match wins when the url belongs to a null-guid article of the same feed",
            article(feed_1, &url("c"), Some(&guid("g1")), "<p>x</p>"),
            Some(a.id),
        ),
        (
            "url-only match with an unknown guid",
            article(feed_1, &url("b"), Some(&guid("unknown")), "<p>x</p>"),
            Some(b.id),
        ),
        (
            "same guid in another feed is a different article",
            article(feed_2, &url("fresh"), Some(&guid("g2")), "<p>x</p>"),
            None,
        ),
        (
            "guid is feed scoped: feed 2 resolves its own g1",
            article(feed_2, &url("a"), Some(&guid("g1")), "<p>x</p>"),
            Some(d.id),
        ),
        (
            "no guid, url match",
            article(feed_1, &url("c"), None, "<p>unrelated</p>"),
            Some(c.id),
        ),
        (
            "no guid, hash-only match",
            article(feed_1, &url("brand-new"), None, "<p>gamma</p>"),
            Some(c.id),
        ),
        (
            "no guid, same content in another feed does not match",
            article(feed_2, &url("brand-new-2"), None, "<p>gamma</p>"),
            None,
        ),
        (
            "no guid, no match",
            article(feed_1, &url("brand-new-3"), None, "<p>nothing like it</p>"),
            None,
        ),
        (
            "guid and url both unknown",
            article(
                feed_1,
                &url("brand-new-4"),
                Some(&guid("nope")),
                "<p>gamma</p>",
            ),
            None,
        ),
        (
            "guid present never falls back to the content hash",
            article(
                feed_1,
                &url("brand-new-5"),
                Some(&guid("g3")),
                "<p>gamma</p>",
            ),
            None,
        ),
    ];
    for (name, new, expected) in cases {
        let old = old_find_duplicate(&db, &new).await;
        let found = articles::find_duplicate(pool, &new)
            .await
            .unwrap()
            .map(|stored| stored.id);
        assert_eq!(found, old, "{name}: differs from the old lookup");
        assert_eq!(found, expected, "{name}: unexpected article");
    }
}

#[tokio::test]
async fn unchanged_refresh_writes_nothing_and_takes_no_row_lock() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let feed = make_feed(&db).await;
    let tag = Uuid::now_v7().simple().to_string();
    let mut with_files = article(
        feed,
        &format!("https://ingest.example/{tag}/files"),
        Some(&format!("{tag}-files")),
        "<p>files</p>",
    );
    with_files.enclosures = vec![enclosure("https://ingest.example/1.mp3")];
    let plain = article(
        feed,
        &format!("https://ingest.example/{tag}/plain"),
        Some(&format!("{tag}-plain")),
        "<p>plain</p>",
    );

    for new in [&with_files, &plain] {
        let (stored, action) = articles::upsert(pool, new).await.unwrap();
        assert_eq!(action, UpsertAction::Created);

        let tuple = |table: &'static str| {
            let sql = if table == "articles" {
                "SELECT xmin::text || '/' || xmax::text || '/' || ctid::text FROM articles WHERE id = $1"
            } else {
                "SELECT COALESCE(string_agg(xmin::text || '/' || xmax::text || '/' || ctid::text, ',' ORDER BY position), '') \
                 FROM article_enclosures WHERE article_id = $1"
            };
            let id = stored.id;
            async move {
                sqlx::query_scalar::<_, String>(sql)
                    .bind(id)
                    .fetch_one(pool)
                    .await
                    .unwrap()
            }
        };
        let article_before = tuple("articles").await;
        let enclosures_before = tuple("enclosures").await;
        for _ in 0..3 {
            let (again, action) = articles::upsert(pool, new).await.unwrap();
            assert_eq!(action, UpsertAction::Skipped);
            assert_eq!(again.id, stored.id);
        }
        assert_eq!(
            tuple("articles").await,
            article_before,
            "unchanged refresh must not touch or lock the article tuple"
        );
        assert_eq!(tuple("enclosures").await, enclosures_before);
    }
}

#[tokio::test]
async fn changed_enclosures_are_still_replaced_and_truncated() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let feed = make_feed(&db).await;
    let tag = Uuid::now_v7().simple().to_string();
    let mut new = article(
        feed,
        &format!("https://ingest.example/{tag}/enc"),
        Some(&format!("{tag}-enc")),
        "<p>enc</p>",
    );
    let (stored, _) = articles::upsert(pool, &new).await.unwrap();

    new.enclosures = vec![
        enclosure("https://ingest.example/a.mp3"),
        enclosure("https://ingest.example/b.mp3"),
    ];
    let (_, action) = articles::upsert(pool, &new).await.unwrap();
    assert_eq!(
        action,
        UpsertAction::Updated,
        "added enclosures are a change"
    );
    assert_eq!(
        articles::list_enclosures(pool, stored.id)
            .await
            .unwrap()
            .len(),
        2
    );

    new.enclosures = vec![enclosure("https://ingest.example/b.mp3")];
    let (_, action) = articles::upsert(pool, &new).await.unwrap();
    assert_eq!(action, UpsertAction::Updated);
    let stored_enclosures = articles::list_enclosures(pool, stored.id).await.unwrap();
    assert_eq!(stored_enclosures.len(), 1);
    assert_eq!(stored_enclosures[0].url, "https://ingest.example/b.mp3");

    new.enclosures = Vec::new();
    let (_, action) = articles::upsert(pool, &new).await.unwrap();
    assert_eq!(
        action,
        UpsertAction::Updated,
        "removing the last enclosure is a change"
    );
    assert!(articles::list_enclosures(pool, stored.id)
        .await
        .unwrap()
        .is_empty());

    new.enclosures = (0..MAX_ARTICLE_ENCLOSURES + 3)
        .map(|index| enclosure(&format!("https://ingest.example/many-{index}.mp3")))
        .collect();
    articles::upsert(pool, &new).await.unwrap();
    assert_eq!(
        articles::list_enclosures(pool, stored.id)
            .await
            .unwrap()
            .len(),
        MAX_ARTICLE_ENCLOSURES
    );
    let (_, action) = articles::upsert(pool, &new).await.unwrap();
    assert_eq!(
        action,
        UpsertAction::Skipped,
        "truncated list is compared as stored"
    );
}

#[tokio::test]
async fn concurrent_enclosure_writers_end_in_one_consistent_list() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let feed = make_feed(&db).await;
    let tag = Uuid::now_v7().simple().to_string();
    let list_a: Vec<NewArticleEnclosure> = (0..3)
        .map(|index| enclosure(&format!("https://ingest.example/a-{index}.mp3")))
        .collect();
    let list_b: Vec<NewArticleEnclosure> = (0..2)
        .map(|index| enclosure(&format!("https://ingest.example/b-{index}.mp3")))
        .collect();

    for round in 0..25 {
        let base = article(
            feed,
            &format!("https://ingest.example/{tag}/race-{round}"),
            Some(&format!("{tag}-race-{round}")),
            "<p>race</p>",
        );
        let (stored, _) = articles::upsert(db.pool(), &base).await.unwrap();

        let mut with_a = base.clone();
        with_a.enclosures = list_a.clone();
        let mut with_b = base.clone();
        with_b.enclosures = list_b.clone();
        let (pool_a, pool_b) = (db.pool().clone(), db.pool().clone());
        let (first, second) = tokio::join!(
            tokio::spawn(async move { articles::upsert(&pool_a, &with_a).await }),
            tokio::spawn(async move { articles::upsert(&pool_b, &with_b).await }),
        );
        first.unwrap().expect("writer A must not fail");
        second.unwrap().expect("writer B must not fail");

        let stored_list = articles::list_enclosures(db.pool(), stored.id)
            .await
            .unwrap();
        let urls: Vec<&str> = stored_list.iter().map(|e| e.url.as_str()).collect();
        let positions: Vec<i32> = stored_list.iter().map(|e| e.position).collect();
        assert_eq!(positions, (0..stored_list.len() as i32).collect::<Vec<_>>());
        let expected_a: Vec<&str> = list_a.iter().map(|e| e.url.as_str()).collect();
        let expected_b: Vec<&str> = list_b.iter().map(|e| e.url.as_str()).collect();
        assert!(
            urls == expected_a || urls == expected_b,
            "round {round}: mixed or duplicated list {urls:?}"
        );
    }
}
