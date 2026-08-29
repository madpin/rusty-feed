use rustss_db::{feeds, jobs, subscriptions, users, Db};

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7())
}

async fn is_due(db: &Db, feed_id: uuid::Uuid) -> bool {
    feeds::list_due(db.pool(), 10_000)
        .await
        .unwrap()
        .iter()
        .any(|feed| feed.id == feed_id)
}

#[tokio::test]
async fn repeated_failures_move_from_normal_to_daily_then_weekly_checks() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let user = users::create(
        db.pool(),
        &format!("{}@example.test", unique("feed-health")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://feed-health.example/{}.xml", uuid::Uuid::now_v7()),
        "Failure cadence",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(db.pool(), user.id, feed.id, None)
        .await
        .unwrap();
    feeds::record_fetch_success(db.pool(), feed.id, None, None)
        .await
        .unwrap();
    for _ in 0..3 {
        feeds::record_fetch_failure(db.pool(), feed.id, feeds::FetchFailure::Network)
            .await
            .unwrap();
    }
    let failed = feeds::find_by_id(db.pool(), feed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.health_status, "error");
    assert_eq!(failed.consecutive_failures, 3);
    assert!(failed.failure_started_at.is_some());
    assert!(!is_due(&db, feed.id).await);

    sqlx::query(
        "UPDATE feeds SET failure_started_at = now() - interval '13 days 23 hours', \
         last_fetched_at = now() - interval '23 hours' WHERE id = $1",
    )
    .bind(feed.id)
    .execute(db.pool())
    .await
    .unwrap();
    assert!(!is_due(&db, feed.id).await);

    sqlx::query(
        "UPDATE feeds SET failure_started_at = now() - interval '14 days', \
         last_fetched_at = now() - interval '6 days 23 hours' WHERE id = $1",
    )
    .bind(feed.id)
    .execute(db.pool())
    .await
    .unwrap();
    assert!(!is_due(&db, feed.id).await);

    sqlx::query("UPDATE feeds SET last_fetched_at = now() - interval '7 days' WHERE id = $1")
        .bind(feed.id)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(is_due(&db, feed.id).await);

    feeds::record_fetch_success(db.pool(), feed.id, None, None)
        .await
        .unwrap();
    let recovered = feeds::find_by_id(db.pool(), feed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.health_status, "healthy");
    assert_eq!(recovered.consecutive_failures, 0);
    assert!(recovered.failure_started_at.is_none());
    let errors = feeds::list_recent_errors(db.pool(), 100).await.unwrap();
    assert!(errors
        .iter()
        .any(|error| error.feed_id == feed.id && error.resolved));
}

#[tokio::test]
async fn disabled_feeds_and_missing_icons_do_not_bypass_failure_backoff() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let user = users::create(
        db.pool(),
        &format!("{}@example.test", unique("feed-health-disabled")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://feed-health.example/{}.xml", uuid::Uuid::now_v7()),
        "Disabled feed",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(db.pool(), user.id, feed.id, None)
        .await
        .unwrap();
    feeds::record_fetch_success(db.pool(), feed.id, None, None)
        .await
        .unwrap();
    for _ in 0..3 {
        feeds::record_fetch_failure(db.pool(), feed.id, feeds::FetchFailure::Network)
            .await
            .unwrap();
    }
    assert!(!is_due(&db, feed.id).await);
    sqlx::query("UPDATE feeds SET health_status = 'disabled', last_fetched_at = now() - interval '30 days' WHERE id = $1")
        .bind(feed.id)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(!is_due(&db, feed.id).await);
}

#[tokio::test]
async fn forced_refresh_schedule_preserves_paused_sources() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let user = users::create(
        db.pool(),
        &format!("{}@example.test", unique("forced-refresh")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (active, _) = feeds::upsert_by_url(
        db.pool(),
        &format!(
            "https://feed-health.example/{}/active.xml",
            uuid::Uuid::now_v7()
        ),
        "Active feed",
        None,
    )
    .await
    .unwrap();
    let (paused, _) = feeds::upsert_by_url(
        db.pool(),
        &format!(
            "https://feed-health.example/{}/paused.xml",
            uuid::Uuid::now_v7()
        ),
        "Paused feed",
        None,
    )
    .await
    .unwrap();
    for feed in [&active, &paused] {
        subscriptions::subscribe(db.pool(), user.id, feed.id, None)
            .await
            .unwrap();
        feeds::record_fetch_success(db.pool(), feed.id, None, None)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE feeds SET health_status = 'disabled' WHERE id = $1")
        .bind(paused.id)
        .execute(db.pool())
        .await
        .unwrap();
    let run_at = time::OffsetDateTime::now_utc() + time::Duration::hours(1);
    let active_job = jobs::enqueue(
        db.pool(),
        "fetch_feed",
        serde_json::json!({ "feed_id": active.id }),
        Some(&format!("test-active:{}", active.id)),
        Some(run_at),
        1,
    )
    .await
    .unwrap()
    .unwrap();
    let paused_job = jobs::enqueue(
        db.pool(),
        "fetch_feed",
        serde_json::json!({ "feed_id": paused.id }),
        Some(&format!("test-paused:{}", paused.id)),
        Some(run_at),
        1,
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(
        feeds::reset_refresh_schedule(db.pool(), &[active.id, paused.id])
            .await
            .unwrap(),
        vec![active.id]
    );
    assert_eq!(
        jobs::run_pending_feed_fetches_now(db.pool(), &[active.id, paused.id])
            .await
            .unwrap(),
        1
    );
    let active_after = feeds::find_by_id(db.pool(), active.id)
        .await
        .unwrap()
        .unwrap();
    let paused_after = feeds::find_by_id(db.pool(), paused.id)
        .await
        .unwrap()
        .unwrap();
    assert!(active_after.last_fetched_at.is_none());
    assert!(paused_after.last_fetched_at.is_some());
    let active_ready: bool =
        sqlx::query_scalar("SELECT run_at <= now() FROM job_queue WHERE id = $1")
            .bind(active_job)
            .fetch_one(db.pool())
            .await
            .unwrap();
    let paused_ready: bool =
        sqlx::query_scalar("SELECT run_at <= now() FROM job_queue WHERE id = $1")
            .bind(paused_job)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(active_ready);
    assert!(!paused_ready);
}
