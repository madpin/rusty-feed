//! Article foreign-key index coverage (FND-235): every table that references
//! `articles(id)` must have an index leading with the referencing column so
//! deleting an article does not scan the table, and deletes must still cascade.
//! Tests skip when `TEST_DATABASE_URL` is unavailable.

use rusty_feed_db::{
    articles, bookmarks, feeds, read_articles, seen_articles, subscriptions, users, Db, NewArticle,
};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect to test database");
    db.migrate().await.expect("run migrations");
    Some(db)
}

#[tokio::test]
async fn every_article_foreign_key_has_a_leading_index() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let foreign_keys: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT c.conrelid::regclass::text, a.attname::text, \
                (SELECT string_agg(i.indexrelid::regclass::text, ', ') \
                   FROM pg_index i \
                  WHERE i.indrelid = c.conrelid AND i.indkey[0] = c.conkey[1]) \
         FROM pg_constraint c \
         JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = c.conkey[1] \
         WHERE c.contype = 'f' AND c.confrelid = 'articles'::regclass \
         ORDER BY 1, 2",
    )
    .fetch_all(db.pool())
    .await
    .expect("list article foreign keys");

    for table in [
        "read_articles",
        "seen_articles",
        "bookmarks",
        "article_feedback",
        "public_article_shares",
        "public_article_share_events",
        "integration_deliveries",
        "story_clusters",
    ] {
        assert!(
            foreign_keys.iter().any(|(name, _, _)| name == table),
            "{table} should reference articles"
        );
    }
    let missing: Vec<_> = foreign_keys
        .iter()
        .filter(|(_, _, indexes)| indexes.is_none())
        .map(|(table, column, _)| format!("{table}.{column}"))
        .collect();
    assert!(
        missing.is_empty(),
        "article foreign keys without a leading index: {missing:?}"
    );
}

async fn count(db: &Db, sql: &str, article_id: Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(article_id)
        .fetch_one(db.pool())
        .await
        .expect("count dependent rows")
}

#[tokio::test]
async fn deleting_an_article_still_cascades_to_personal_state() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let user_id = users::create(
        db.pool(),
        &format!("fk-index-{}@example.test", Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .expect("create user")
    .id;
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://fk-index.example/{}/feed.xml", Uuid::now_v7()),
        "FK feed",
        None,
    )
    .await
    .expect("create feed");
    subscriptions::subscribe(db.pool(), user_id, feed.id, None)
        .await
        .expect("subscribe");
    let mut ids = Vec::new();
    for title in ["Doomed", "Survivor"] {
        let (article, _) = articles::upsert(
            db.pool(),
            &NewArticle {
                feed_id: feed.id,
                url: format!("https://fk-index.example/articles/{}", Uuid::now_v7()),
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
        .expect("create article");
        ids.push(article.id);
    }
    let profile_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO integration_profiles (id, user_id, name, provider, endpoint_url) \
         VALUES ($1, $2, 'Webhook', 'webhook', 'https://hooks.example/fk')",
    )
    .bind(profile_id)
    .bind(user_id)
    .execute(db.pool())
    .await
    .expect("integration profile");
    for article_id in &ids {
        read_articles::mark_read(db.pool(), user_id, *article_id)
            .await
            .expect("read");
        seen_articles::mark_seen(db.pool(), user_id, *article_id)
            .await
            .expect("seen");
        bookmarks::add(db.pool(), user_id, *article_id)
            .await
            .expect("bookmark");
        sqlx::query(
            "INSERT INTO article_feedback (id, user_id, article_id, feedback_type, feedback_value) \
             VALUES ($1, $2, $3, 'explicit', 1.0)",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(article_id)
        .execute(db.pool())
        .await
        .expect("feedback");
        let share_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO public_article_shares \
                 (id, user_id, article_id, token_hash, token_ciphertext, slug) \
             VALUES ($1, $2, $3, $4, 'ciphertext', 'shared-story')",
        )
        .bind(share_id)
        .bind(user_id)
        .bind(article_id)
        .bind(format!("{}{}", article_id.simple(), article_id.simple()))
        .execute(db.pool())
        .await
        .expect("public share");
        sqlx::query(
            "INSERT INTO public_article_share_events (id, share_id, user_id, article_id, action) \
             VALUES ($1, $2, $3, $4, 'updated')",
        )
        .bind(Uuid::now_v7())
        .bind(share_id)
        .bind(user_id)
        .bind(article_id)
        .execute(db.pool())
        .await
        .expect("share event");
        sqlx::query(
            "INSERT INTO integration_deliveries (id, profile_id, user_id, article_id, event_type) \
             VALUES ($1, $2, $3, $4, 'article.saved')",
        )
        .bind(Uuid::now_v7())
        .bind(profile_id)
        .bind(user_id)
        .bind(article_id)
        .execute(db.pool())
        .await
        .expect("delivery");
        sqlx::query(
            "INSERT INTO story_clusters (id, title, representative_article_id, model) \
             VALUES ($1, 'Story', $2, 'test')",
        )
        .bind(Uuid::now_v7())
        .bind(article_id)
        .execute(db.pool())
        .await
        .expect("story cluster");
    }

    let (doomed, survivor) = (ids[0], ids[1]);
    sqlx::query("DELETE FROM articles WHERE id = $1")
        .bind(doomed)
        .execute(db.pool())
        .await
        .expect("delete article");

    let per_table = [
        "SELECT count(*) FROM read_articles WHERE article_id = $1",
        "SELECT count(*) FROM seen_articles WHERE article_id = $1",
        "SELECT count(*) FROM bookmarks WHERE article_id = $1",
        "SELECT count(*) FROM article_feedback WHERE article_id = $1",
        "SELECT count(*) FROM public_article_shares WHERE article_id = $1",
        "SELECT count(*) FROM public_article_share_events WHERE article_id = $1",
        "SELECT count(*) FROM integration_deliveries WHERE article_id = $1",
        "SELECT count(*) FROM story_clusters WHERE representative_article_id = $1",
    ];
    for sql in per_table {
        assert_eq!(
            count(&db, sql, doomed).await,
            0,
            "{sql} for deleted article"
        );
        assert_eq!(count(&db, sql, survivor).await, 1, "{sql} for kept article");
    }
    // Delivery history is kept with the article reference cleared.
    let orphaned: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM integration_deliveries \
         WHERE profile_id = $1 AND article_id IS NULL",
    )
    .bind(profile_id)
    .fetch_one(db.pool())
    .await
    .expect("count cleared deliveries");
    assert_eq!(orphaned, 1);
}
