//! Retention purge of finished queue jobs, dispatched outbox events and
//! finished agent runs (FND-238). Requires `TEST_DATABASE_URL`; skipped otherwise.
//!
//! The purges are table-wide, so the tests serialise on one lock and assert on
//! the exact rows each test inserted rather than on global counts.

use std::collections::HashSet;

use rusty_feed_db::jobs::PurgeLimits;
use rusty_feed_db::models::NewArticle;
use rusty_feed_db::{agent_runs, articles, feeds, jobs, outbox, Db};
use serde_json::json;
use uuid::Uuid;

static PURGE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn test_db() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn insert_job(
    db: &Db,
    kind: &str,
    status: &str,
    age_days: i32,
    payload: serde_json::Value,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO job_queue (id, kind, payload, status, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, now() - make_interval(days => $5), now() - make_interval(days => $5))",
    )
    .bind(id)
    .bind(kind)
    .bind(payload)
    .bind(status)
    .bind(age_days)
    .execute(db.pool())
    .await
    .unwrap();
    id
}

async fn surviving_jobs(db: &Db, ids: &[Uuid]) -> HashSet<Uuid> {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM job_queue WHERE id = ANY($1)")
        .bind(ids)
        .fetch_all(db.pool())
        .await
        .unwrap()
        .into_iter()
        .collect()
}

async fn insert_outbox(db: &Db, kind: &str, age_days: i32, processed: bool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO outbox_events (id, kind, payload, created_at, processed_at) \
         VALUES ($1, $2, '{}', now() - make_interval(days => $3), \
                 CASE WHEN $4 THEN now() - make_interval(days => $3) END)",
    )
    .bind(id)
    .bind(kind)
    .bind(age_days)
    .bind(processed)
    .execute(db.pool())
    .await
    .unwrap();
    id
}

async fn surviving_events(db: &Db, ids: &[Uuid]) -> HashSet<Uuid> {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM outbox_events WHERE id = ANY($1)")
        .bind(ids)
        .fetch_all(db.pool())
        .await
        .unwrap()
        .into_iter()
        .collect()
}

async fn insert_run(db: &Db, agent: &str, status: &str, age_days: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agent_runs (id, agent_name, status, trigger, started_at) \
         VALUES ($1, $2, $3, 'scheduler', now() - make_interval(days => $4))",
    )
    .bind(id)
    .bind(agent)
    .bind(status)
    .bind(age_days)
    .execute(db.pool())
    .await
    .unwrap();
    id
}

async fn surviving_runs(db: &Db, ids: &[Uuid]) -> HashSet<Uuid> {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM agent_runs WHERE id = ANY($1)")
        .bind(ids)
        .fetch_all(db.pool())
        .await
        .unwrap()
        .into_iter()
        .collect()
}

fn set(ids: &[Uuid]) -> HashSet<Uuid> {
    ids.iter().copied().collect()
}

async fn make_article(db: &Db, summarized: bool) -> Uuid {
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://maintenance.example/{}.xml", Uuid::now_v7()),
        "Maintenance",
        None,
    )
    .await
    .unwrap();
    let (article, _) = articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://maintenance.example/{}", Uuid::now_v7()),
            comments_url: None,
            guid: Some(Uuid::now_v7().to_string()),
            title: "Maintenance".into(),
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
    if summarized {
        sqlx::query("UPDATE articles SET summarized_at = now() WHERE id = $1")
            .bind(article.id)
            .execute(db.pool())
            .await
            .unwrap();
    }
    article.id
}

#[tokio::test]
async fn purge_done_jobs_deletes_only_old_done_rows() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = PURGE_LOCK.lock().await;
    let kind = format!("maintenance-test-{}", Uuid::now_v7());

    let old_done = insert_job(&db, &kind, "done", 10, json!({})).await;
    let older_done = insert_job(&db, &kind, "done", 400, json!({})).await;
    let recent_done = insert_job(&db, &kind, "done", 1, json!({})).await;
    let boundary_done = insert_job(&db, &kind, "done", 6, json!({})).await;
    let old_pending = insert_job(&db, &kind, "pending", 400, json!({})).await;
    let old_running = insert_job(&db, &kind, "running", 400, json!({})).await;
    let old_dead = insert_job(&db, &kind, "dead", 400, json!({})).await;
    let all = [
        old_done,
        older_done,
        recent_done,
        boundary_done,
        old_pending,
        old_running,
        old_dead,
    ];

    let deleted = jobs::purge_done(db.pool(), 7).await.unwrap();
    assert!(deleted >= 2, "deleted {deleted}");
    assert_eq!(
        surviving_jobs(&db, &all).await,
        set(&[
            recent_done,
            boundary_done,
            old_pending,
            old_running,
            old_dead
        ]),
        "only done rows older than the retention window may be deleted"
    );

    sqlx::query("DELETE FROM job_queue WHERE kind = $1")
        .bind(&kind)
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn purge_done_keeps_summary_jobs_whose_article_has_no_summary() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = PURGE_LOCK.lock().await;
    let unsummarized = make_article(&db, false).await;
    let summarized = make_article(&db, true).await;

    let keep = insert_job(
        &db,
        "summarize_article",
        "done",
        30,
        json!({ "article_id": unsummarized, "content_revision": 1 }),
    )
    .await;
    let purge_summarized = insert_job(
        &db,
        "summarize_article",
        "done",
        30,
        json!({ "article_id": summarized, "content_revision": 1 }),
    )
    .await;
    let purge_missing = insert_job(
        &db,
        "summarize_article",
        "done",
        30,
        json!({ "article_id": Uuid::now_v7(), "content_revision": 1 }),
    )
    .await;
    let purge_malformed = insert_job(
        &db,
        "summarize_article",
        "done",
        30,
        json!({ "article_id": "not-a-uuid" }),
    )
    .await;
    let purge_no_payload = insert_job(&db, "summarize_article", "done", 30, json!({})).await;
    let all = [
        keep,
        purge_summarized,
        purge_missing,
        purge_malformed,
        purge_no_payload,
    ];

    jobs::purge_done(db.pool(), 7).await.unwrap();
    assert_eq!(surviving_jobs(&db, &all).await, set(&[keep]));

    // Once the article is summarized the finished job is no longer evidence.
    sqlx::query("UPDATE articles SET summarized_at = now() WHERE id = $1")
        .bind(unsummarized)
        .execute(db.pool())
        .await
        .unwrap();
    jobs::purge_done(db.pool(), 7).await.unwrap();
    assert!(surviving_jobs(&db, &all).await.is_empty());
}

#[tokio::test]
async fn purge_processed_outbox_events_keeps_unprocessed_and_recent_rows() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = PURGE_LOCK.lock().await;
    let kind = format!("maintenance-outbox-{}", Uuid::now_v7());

    let old_processed = insert_outbox(&db, &kind, 10, true).await;
    let recent_processed = insert_outbox(&db, &kind, 1, true).await;
    let old_unprocessed = insert_outbox(&db, &kind, 400, false).await;
    let recent_unprocessed = insert_outbox(&db, &kind, 0, false).await;
    let all = [
        old_processed,
        recent_processed,
        old_unprocessed,
        recent_unprocessed,
    ];

    let deleted = outbox::purge_processed(db.pool(), 7).await.unwrap();
    assert!(deleted >= 1, "deleted {deleted}");
    assert_eq!(
        surviving_events(&db, &all).await,
        set(&[recent_processed, old_unprocessed, recent_unprocessed])
    );

    sqlx::query("DELETE FROM outbox_events WHERE kind = $1")
        .bind(&kind)
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn purged_outbox_event_is_never_dispatched_again() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = PURGE_LOCK.lock().await;
    let kind = format!("maintenance-redispatch-{}", Uuid::now_v7());
    let kinds = std::slice::from_ref(&kind);

    let mut tx = db.pool().begin().await.unwrap();
    let event = outbox::publish(&mut tx, &kind, json!({})).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(outbox::dispatch(db.pool(), kinds, 10).await.unwrap(), 1);
    let dedup = format!("outbox:{event}");
    assert_eq!(
        jobs::count_active_by_dedup(db.pool(), &dedup)
            .await
            .unwrap(),
        1
    );

    sqlx::query("UPDATE outbox_events SET processed_at = now() - interval '10 days' WHERE id = $1")
        .bind(event)
        .execute(db.pool())
        .await
        .unwrap();
    outbox::purge_processed(db.pool(), 7).await.unwrap();
    assert!(surviving_events(&db, &[event]).await.is_empty());
    assert_eq!(
        outbox::dispatch(db.pool(), kinds, 10).await.unwrap(),
        0,
        "a purged event has nothing left to dispatch"
    );

    sqlx::query("DELETE FROM job_queue WHERE kind = $1")
        .bind(&kind)
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn purge_finished_agent_runs_keeps_running_and_recent_rows() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = PURGE_LOCK.lock().await;
    let agent = format!("maintenance-agent-{}", Uuid::now_v7());

    let old_success = insert_run(&db, &agent, "success", 45).await;
    let old_failed = insert_run(&db, &agent, "failed", 45).await;
    let recent_success = insert_run(&db, &agent, "success", 5).await;
    let old_running = insert_run(&db, &agent, "running", 400).await;
    let old_pending = insert_run(&db, &agent, "pending", 400).await;
    let all = [
        old_success,
        old_failed,
        recent_success,
        old_running,
        old_pending,
    ];

    let deleted = agent_runs::purge_finished(db.pool(), 30).await.unwrap();
    assert!(deleted >= 2, "deleted {deleted}");
    assert_eq!(
        surviving_runs(&db, &all).await,
        set(&[recent_success, old_running, old_pending])
    );
    assert_eq!(
        agent_runs::count_by_status(db.pool(), &agent, "success")
            .await
            .unwrap(),
        1
    );

    sqlx::query("DELETE FROM agent_runs WHERE agent_name = $1")
        .bind(&agent)
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn purge_loops_over_batches_and_honours_the_per_run_cap() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = PURGE_LOCK.lock().await;
    let kind = format!("maintenance-batch-{}", Uuid::now_v7());
    let mut ids = Vec::new();
    for _ in 0..12 {
        ids.push(insert_job(&db, &kind, "done", 10, json!({})).await);
    }

    let capped = PurgeLimits {
        batch_size: 5,
        max_rows: 7,
    };
    assert_eq!(
        jobs::purge_done_with(db.pool(), 7, capped).await.unwrap(),
        7
    );
    assert_eq!(surviving_jobs(&db, &ids).await.len(), 5);

    let batched = PurgeLimits {
        batch_size: 2,
        max_rows: 1_000,
    };
    let deleted = jobs::purge_done_with(db.pool(), 7, batched).await.unwrap();
    assert!(
        deleted >= 5,
        "a batch size of 2 must have looped, deleted {deleted}"
    );
    assert!(surviving_jobs(&db, &ids).await.is_empty());

    let mut events = Vec::new();
    for _ in 0..7 {
        events.push(insert_outbox(&db, &kind, 10, true).await);
    }
    let batched = PurgeLimits {
        batch_size: 3,
        max_rows: 1_000,
    };
    let deleted = outbox::purge_processed_with(db.pool(), 7, batched)
        .await
        .unwrap();
    assert!(deleted >= 7, "deleted {deleted}");
    assert!(surviving_events(&db, &events).await.is_empty());

    let agent = format!("maintenance-batch-agent-{}", Uuid::now_v7());
    let mut runs = Vec::new();
    for _ in 0..7 {
        runs.push(insert_run(&db, &agent, "success", 60).await);
    }
    let deleted = agent_runs::purge_finished_with(db.pool(), 30, batched)
        .await
        .unwrap();
    assert!(deleted >= 7, "deleted {deleted}");
    assert!(surviving_runs(&db, &runs).await.is_empty());
}

#[tokio::test]
async fn zero_retention_disables_every_purge() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = PURGE_LOCK.lock().await;
    let tag = format!("maintenance-disabled-{}", Uuid::now_v7());

    let job = insert_job(&db, &tag, "done", 4_000, json!({})).await;
    let event = insert_outbox(&db, &tag, 4_000, true).await;
    let run = insert_run(&db, &tag, "success", 4_000).await;

    assert_eq!(jobs::purge_done(db.pool(), 0).await.unwrap(), 0);
    assert_eq!(outbox::purge_processed(db.pool(), 0).await.unwrap(), 0);
    assert_eq!(agent_runs::purge_finished(db.pool(), 0).await.unwrap(), 0);
    assert_eq!(surviving_jobs(&db, &[job]).await.len(), 1);
    assert_eq!(surviving_events(&db, &[event]).await.len(), 1);
    assert_eq!(surviving_runs(&db, &[run]).await.len(), 1);

    jobs::purge_done(db.pool(), 7).await.unwrap();
    outbox::purge_processed(db.pool(), 7).await.unwrap();
    agent_runs::purge_finished(db.pool(), 30).await.unwrap();
    assert!(surviving_jobs(&db, &[job]).await.is_empty());
    assert!(surviving_events(&db, &[event]).await.is_empty());
    assert!(surviving_runs(&db, &[run]).await.is_empty());
}
