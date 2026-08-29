//! M7 integration tests: durable queue semantics (dedup, SKIP LOCKED, backoff)
//! and the agents (O(1) enqueue, fetch+ingest via the queue). Require
//! `TEST_DATABASE_URL`; skipped otherwise.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use time::Duration as TimeDuration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rustss_agents::{process, Agent, AgentContext, FeedRefreshAgent, FetchFeedAgent, Registry};
use rustss_db::{agent_runs, articles, feeds, jobs, subscriptions, users, Db};
use rustss_feeds::SafeFetchClient;

async fn connect() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn ctx(db: Db) -> AgentContext {
    AgentContext::new(
        db,
        SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap(),
    )
}

fn rss(run: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><rss version="2.0"><channel><title>T</title><link>https://m.ex</link>
        <item><title>One</title><link>https://m.ex/{run}/1</link><guid>{run}-1</guid></item>
        <item><title>Two</title><link>https://m.ex/{run}/2</link><guid>{run}-2</guid></item>
        </channel></rss>"#
    )
}

async fn make_user(db: &Db) -> uuid::Uuid {
    users::create(
        db.pool(),
        &format!("agent-{}@ex.com", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn feed_refresh_enqueues_one_fetch_regardless_of_subscribers() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    // A brand-new feed (never fetched → due) with THREE subscribers.
    let url = format!("https://ex.com/{}.xml", uuid::Uuid::now_v7());
    let (feed, _) = feeds::upsert_by_url(db.pool(), &url, "Feed", None)
        .await
        .unwrap();
    for _ in 0..3 {
        let uid = make_user(&db).await;
        subscriptions::subscribe(db.pool(), uid, feed.id, None)
            .await
            .unwrap();
    }

    let dedup = format!("fetch_feed:{}", feed.id);
    let context = ctx(db.clone());

    FeedRefreshAgent.run(&context, json!({})).await.unwrap();
    assert_eq!(
        jobs::count_active_by_dedup(db.pool(), &dedup)
            .await
            .unwrap(),
        1,
        "N subscribers must enqueue exactly one fetch"
    );

    // Running again does not create a duplicate (dedup on active jobs).
    FeedRefreshAgent.run(&context, json!({})).await.unwrap();
    assert_eq!(
        jobs::count_active_by_dedup(db.pool(), &dedup)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn fetch_feed_job_ingests_and_records_a_run() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let run = uuid::Uuid::now_v7().to_string();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&run)))
        .mount(&server)
        .await;

    let feed_url = format!("{}/feed.xml?r={run}", server.uri());
    let (feed, _) = feeds::upsert_by_url(db.pool(), &feed_url, "Feed", None)
        .await
        .unwrap();

    // Use a unique job kind so this test's consumer can't claim other tests'
    // jobs (the queue is shared across parallel tests).
    let kind = format!("fetch_feed_test_{run}");
    let mut registry = Registry::with_defaults();
    assert!(registry.get("catalog_content_probe").is_some());
    registry.register(kind.clone(), Arc::new(FetchFeedAgent));

    let id = jobs::enqueue(
        db.pool(),
        &kind,
        json!({ "feed_id": feed.id }),
        None,
        None,
        5,
    )
    .await
    .unwrap()
    .unwrap();

    let job = jobs::claim_next(db.pool(), "w1", std::slice::from_ref(&kind))
        .await
        .unwrap()
        .expect("should claim our job");
    assert_eq!(job.id, id);

    let context = ctx(db.clone());
    process(&context, &registry, job).await;

    assert_eq!(
        jobs::get_status(db.pool(), id).await.unwrap().as_deref(),
        Some("done")
    );
    let arts = articles::list_for_feed(db.pool(), feed.id, 10)
        .await
        .unwrap();
    assert_eq!(arts.len(), 2, "articles should be ingested by the job");
    assert!(
        agent_runs::count_by_status(db.pool(), &kind, "success")
            .await
            .unwrap()
            >= 1
    );
}

#[tokio::test]
async fn failing_job_backs_off_then_dies() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let kind = format!("test_fail_{}", uuid::Uuid::now_v7());

    // max_attempts = 2: first failure retries, second dies.
    let id = jobs::enqueue(db.pool(), &kind, json!({}), None, None, 2)
        .await
        .unwrap()
        .unwrap();

    let job = jobs::claim_next(db.pool(), "w", std::slice::from_ref(&kind))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.attempts, 1);
    let outcome = jobs::fail(db.pool(), &job, "boom", TimeDuration::seconds(1))
        .await
        .unwrap();
    assert_eq!(outcome, jobs::FailOutcome::Retried);
    assert_eq!(
        jobs::get_status(db.pool(), id).await.unwrap().as_deref(),
        Some("pending")
    );

    // Make it due again immediately, then fail once more → dead.
    sqlx_set_run_at_now(&db, id).await;
    let job = jobs::claim_next(db.pool(), "w", std::slice::from_ref(&kind))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.attempts, 2);
    let outcome = jobs::fail(db.pool(), &job, "boom", TimeDuration::seconds(1))
        .await
        .unwrap();
    assert_eq!(outcome, jobs::FailOutcome::Dead);
    assert_eq!(
        jobs::get_status(db.pool(), id).await.unwrap().as_deref(),
        Some("dead")
    );
}

#[tokio::test]
async fn skip_locked_prevents_double_claim() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    // Unique kind isolates this test from parallel ones sharing the queue.
    let kind = format!("test_claim_{}", uuid::Uuid::now_v7());
    for _ in 0..3 {
        jobs::enqueue(db.pool(), &kind, json!({}), None, None, 5)
            .await
            .unwrap();
    }

    // Many concurrent claimers; each claimed job id must be unique.
    let mut handles = Vec::new();
    for i in 0..8 {
        let pool = db.pool().clone();
        let kind = kind.clone();
        handles.push(tokio::spawn(async move {
            jobs::claim_next(&pool, &format!("w{i}"), &[kind])
                .await
                .unwrap()
        }));
    }
    let mut ids = Vec::new();
    for h in handles {
        if let Some(job) = h.await.unwrap() {
            ids.push(job.id);
        }
    }
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(ids.len(), unique.len(), "a job must never be claimed twice");
    assert_eq!(
        ids.len(),
        3,
        "all three jobs should be claimed exactly once"
    );
}

/// Helper: make a job runnable immediately again (bypass backoff for the test).
async fn sqlx_set_run_at_now(db: &Db, id: uuid::Uuid) {
    sqlx::query("UPDATE job_queue SET run_at = now() WHERE id = $1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
}
