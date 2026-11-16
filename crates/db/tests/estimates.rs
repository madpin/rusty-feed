//! Approximate admin totals (FND-239). Requires `TEST_DATABASE_URL`; skipped
//! otherwise. Fallback/decision behaviour on scratch tables is covered by the
//! unit tests in `crates/db/src/estimates.rs`.

use rusty_feed_db::estimates::{estimated_rows, EstimatedTable};
use rusty_feed_db::models::NewArticle;
use rusty_feed_db::{articles, feeds, Db};
use uuid::Uuid;

/// `ANALYZE` reads every page of a table below this many rows, so its statistic
/// is exact; above it the statistic is a sample extrapolation.
const ANALYZE_EXACT_ROWS: i64 = 30_000;

async fn test_db() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn assert_close(estimate: i64, exact: i64) {
    if exact <= ANALYZE_EXACT_ROWS {
        assert_eq!(estimate, exact);
    } else {
        assert!(
            (estimate - exact).abs() * 20 <= exact,
            "estimate {estimate} too far from exact {exact}"
        );
    }
}

#[tokio::test]
async fn estimate_matches_exact_count_right_after_analyze() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let (feed, _) = feeds::upsert_by_url(
        pool,
        &format!("https://estimates.example/{}.xml", Uuid::now_v7()),
        "Estimates",
        None,
    )
    .await
    .unwrap();
    for index in 0..5 {
        articles::upsert(
            pool,
            &NewArticle {
                feed_id: feed.id,
                url: format!("https://estimates.example/{}/{index}", Uuid::now_v7()),
                comments_url: None,
                guid: Some(Uuid::now_v7().to_string()),
                title: format!("Estimate {index}"),
                author: None,
                content: "Body".into(),
                excerpt: None,
                image_url: None,
                enclosures: Vec::new(),
                published_at: None,
            },
        )
        .await
        .unwrap();
    }

    sqlx::query("ANALYZE articles").execute(pool).await.unwrap();
    sqlx::query("ANALYZE user_feeds")
        .execute(pool)
        .await
        .unwrap();

    let exact = articles::count(pool).await.unwrap();
    assert!(exact >= 5);
    assert_close(articles::estimated_count(pool).await.unwrap(), exact);
    assert_close(
        estimated_rows(pool, EstimatedTable::Articles)
            .await
            .unwrap(),
        exact,
    );

    let exact_user_feeds: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM user_feeds")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_close(
        estimated_rows(pool, EstimatedTable::UserFeeds)
            .await
            .unwrap(),
        exact_user_feeds,
    );
}

#[tokio::test]
async fn admin_summary_estimates_only_the_article_total() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    sqlx::query("ANALYZE articles").execute(pool).await.unwrap();

    let summary = feeds::admin_summary(pool).await.unwrap();
    let exact_feeds: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM feeds")
        .fetch_one(pool)
        .await
        .unwrap();
    let exact_subscribers: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM user_feeds")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(summary.total, exact_feeds, "feed totals stay exact");
    assert_eq!(
        summary.subscribers, exact_subscribers,
        "subscription totals stay exact"
    );
    assert_close(summary.articles, articles::count(pool).await.unwrap());
}

#[tokio::test]
async fn exact_counts_still_drive_filters_and_pagination() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let pool = db.pool();
    let marker = Uuid::now_v7().simple().to_string();
    for index in 0..3 {
        feeds::upsert_by_url(
            pool,
            &format!("https://estimates.example/{marker}-{index}.xml"),
            &format!("Filtered {marker}"),
            None,
        )
        .await
        .unwrap();
    }
    let filtered = feeds::count_admin_filtered(pool, None, Some(&marker))
        .await
        .unwrap();
    assert_eq!(filtered, 3, "filtered counts must remain exact");
}
