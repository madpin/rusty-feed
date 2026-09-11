//! M7 integration tests: durable queue semantics (dedup, SKIP LOCKED, backoff)
//! and the agents (O(1) enqueue, fetch+ingest via the queue). Require
//! `TEST_DATABASE_URL`; skipped otherwise.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use time::Duration as TimeDuration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rusty_feed_agents::{
    process, Agent, AgentContext, FeedRefreshAgent, FetchFeedAgent, Registry, SummaryBackfillAgent,
};
use rusty_feed_db::{agent_runs, articles, feeds, jobs, subscriptions, summary_policy, users, Db};
use rusty_feed_feeds::SafeFetchClient;

static SUMMARY_POLICY_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct FailingAgent;

#[async_trait::async_trait]
impl Agent for FailingAgent {
    fn name(&self) -> &'static str {
        "failing_test_agent"
    }

    async fn run(
        &self,
        _ctx: &AgentContext,
        _payload: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        anyhow::bail!(
            "summary provider returned 401 at https://user:password@example.test/v1/chat?api_key=secret"
        )
    }
}

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
    rusty_feed_db::outbox::dispatch(db.pool(), &["fetch_feed".to_string()], 1_000)
        .await
        .unwrap();
    assert_eq!(
        jobs::count_active_by_dedup(db.pool(), &dedup)
            .await
            .unwrap(),
        1,
        "N subscribers must enqueue exactly one fetch"
    );

    // Running again does not create a duplicate (dedup on active jobs).
    FeedRefreshAgent.run(&context, json!({})).await.unwrap();
    rusty_feed_db::outbox::dispatch(db.pool(), &["fetch_feed".to_string()], 1_000)
        .await
        .unwrap();
    assert_eq!(
        jobs::count_active_by_dedup(db.pool(), &dedup)
            .await
            .unwrap(),
        1
    );
    sqlx::query("DELETE FROM job_queue WHERE dedup_key = $1")
        .bind(dedup)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox_events WHERE kind = 'fetch_feed' AND payload->>'feed_id' = $1")
        .bind(feed.id.to_string())
        .execute(db.pool())
        .await
        .unwrap();
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
    sqlx::query("DELETE FROM job_queue WHERE id = $1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM agent_runs WHERE agent_name = $1")
        .bind(kind)
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn feed_ingestion_queues_eligible_changed_articles_when_enabled() {
    let _guard = SUMMARY_POLICY_TEST_LOCK.lock().await;
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let run = uuid::Uuid::now_v7();
    let server = MockServer::start().await;
    let body = format!(
        r#"<?xml version="1.0"?><rss version="2.0"><channel><title>T</title><link>https://summary.example</link>
        <item><title>Long</title><link>https://summary.example/{run}</link><guid>{run}</guid><description><![CDATA[<p>{}</p>]]></description></item>
        </channel></rss>"#,
        "eligible content ".repeat(20)
    );
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("{}/feed.xml", server.uri()),
        "Summary ingest",
        None,
    )
    .await
    .unwrap();
    let (existing, _) = articles::upsert(
        db.pool(),
        &rusty_feed_db::models::NewArticle {
            feed_id: feed.id,
            url: format!("https://summary.example/existing-{run}"),
            comments_url: None,
            guid: None,
            title: "Existing eligible article".to_string(),
            author: None,
            content: format!("<p>{}</p>", "old eligible content ".repeat(20)),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    summary_policy::update(db.pool(), true, 100).await.unwrap();

    let result = FetchFeedAgent
        .run(&ctx(db.clone()), json!({ "feed_id": feed.id }))
        .await
        .unwrap();

    assert_eq!(result["summarize_jobs"], 1);
    let article = articles::list_for_feed(db.pool(), feed.id, 10)
        .await
        .unwrap()
        .into_iter()
        .find(|article| article.title == "Long")
        .unwrap();
    let dedup = format!("summarize:{}:{}", article.id, article.content_revision);
    assert_eq!(
        jobs::count_active_by_dedup(db.pool(), &dedup)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        jobs::count_active_by_dedup(
            db.pool(),
            &format!("summarize:{}:{}", existing.id, existing.content_revision),
        )
        .await
        .unwrap(),
        0,
        "saving automatic policy must not backfill old articles"
    );
    summary_policy::update(db.pool(), false, summary_policy::DEFAULT_MIN_CONTENT_CHARS)
        .await
        .unwrap();
    sqlx::query("DELETE FROM job_queue WHERE dedup_key = $1")
        .bind(dedup)
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn automatic_summary_backfill_queues_only_articles_over_the_plain_text_minimum() {
    use rusty_feed_db::models::NewArticle;

    let _guard = SUMMARY_POLICY_TEST_LOCK.lock().await;
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://summary-{}.example/feed", uuid::Uuid::now_v7()),
        "Summary threshold",
        None,
    )
    .await
    .unwrap();
    let create = |title: &str, content: String| NewArticle {
        feed_id: feed.id,
        url: format!("https://summary.example/{}", uuid::Uuid::now_v7()),
        comments_url: None,
        guid: None,
        title: title.to_string(),
        author: None,
        content,
        excerpt: None,
        image_url: None,
        enclosures: Vec::new(),
        published_at: None,
    };
    let (short, _) = articles::upsert(
        db.pool(),
        &create("Short", "<p>nineteen chars here</p>".to_string()),
    )
    .await
    .unwrap();
    let (long, _) = articles::upsert(
        db.pool(),
        &create("Long", format!("<p>{}</p>", "long article ".repeat(20))),
    )
    .await
    .unwrap();
    summary_policy::update(db.pool(), true, 100).await.unwrap();

    let result = SummaryBackfillAgent
        .run(
            &ctx(db.clone()),
            json!({ "feed_id": feed.id, "ready_only": false }),
        )
        .await
        .unwrap();

    assert_eq!(result["enqueued"], 1);
    assert_eq!(
        jobs::count_active_by_dedup(
            db.pool(),
            &format!("summarize:{}:{}", long.id, long.content_revision),
        )
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        jobs::count_active_by_dedup(
            db.pool(),
            &format!("summarize:{}:{}", short.id, short.content_revision),
        )
        .await
        .unwrap(),
        0
    );

    summary_policy::update(db.pool(), false, summary_policy::DEFAULT_MIN_CONTENT_CHARS)
        .await
        .unwrap();
    sqlx::query("DELETE FROM job_queue WHERE payload->>'article_id' IN ($1, $2)")
        .bind(long.id.to_string())
        .bind(short.id.to_string())
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn process_persists_a_sanitized_actionable_error() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let kind = format!("failing_test_{}", uuid::Uuid::now_v7());
    let mut registry = Registry::with_defaults();
    registry.register(kind.clone(), Arc::new(FailingAgent));
    let id = jobs::enqueue_in_lane(
        db.pool(),
        jobs::QueueLane::Interactive,
        &kind,
        json!({}),
        None,
        None,
        1,
    )
    .await
    .unwrap()
    .unwrap();
    let job = jobs::claim_next_in_lane(
        db.pool(),
        "interactive#0",
        jobs::QueueLane::Interactive,
        std::slice::from_ref(&kind),
    )
    .await
    .unwrap()
    .unwrap();

    process(&ctx(db.clone()), &registry, job).await;

    assert_eq!(
        jobs::get_status(db.pool(), id).await.unwrap().as_deref(),
        Some("dead")
    );
    let queue_error: String = sqlx::query_scalar("SELECT last_error FROM job_queue WHERE id = $1")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(queue_error.contains("summary provider returned 401"));
    assert!(queue_error.contains("https://example.test/v1/chat"));
    assert!(!queue_error.contains("password"));
    assert!(!queue_error.contains("secret"));
    let run = agent_runs::list_recent(db.pool(), 100)
        .await
        .unwrap()
        .into_iter()
        .find(|run| run.agent_name == kind)
        .unwrap();
    assert_eq!(run.error_message.as_deref(), Some(queue_error.as_str()));

    sqlx::query("DELETE FROM job_queue WHERE id = $1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM agent_runs WHERE agent_name = $1")
        .bind(kind)
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn paused_feed_fetch_job_skips_network_access() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let server = MockServer::start().await;
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("{}/paused.xml", server.uri()),
        "Paused feed",
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE feeds SET health_status = 'disabled' WHERE id = $1")
        .bind(feed.id)
        .execute(db.pool())
        .await
        .unwrap();

    let result = FetchFeedAgent
        .run(&ctx(db.clone()), json!({ "feed_id": feed.id }))
        .await
        .unwrap();
    assert_eq!(result["skipped"], "feed is paused");
    assert!(server.received_requests().await.unwrap().is_empty());
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
    sqlx::query("DELETE FROM job_queue WHERE id = $1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
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
    sqlx::query("DELETE FROM job_queue WHERE kind = $1")
        .bind(kind)
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn worker_heartbeats_report_capacity_and_unregister_cleanly() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let worker_id = format!("test-worker-{}", uuid::Uuid::now_v7());
    jobs::register_worker(db.pool(), &worker_id, 7, 2, true)
        .await
        .unwrap();

    let workers = jobs::list_online_workers(db.pool()).await.unwrap();
    let worker = workers
        .iter()
        .find(|worker| worker.worker_id == worker_id)
        .expect("registered worker should be online");
    assert_eq!(worker.concurrency, 9);
    assert_eq!(worker.interactive_concurrency, 2);
    assert!(worker.scheduler_enabled);

    let kind = format!("heartbeat_queue_test_{}", uuid::Uuid::now_v7());
    jobs::enqueue(db.pool(), &kind, json!({}), None, None, 2)
        .await
        .unwrap();
    jobs::claim_next(
        db.pool(),
        &format!("{worker_id}#0"),
        std::slice::from_ref(&kind),
    )
    .await
    .unwrap()
    .expect("heartbeat test job should be claimed");
    let summary = jobs::queue_by_kind(db.pool())
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.kind == kind)
        .unwrap();
    assert_eq!(summary.running, 1);
    assert_eq!(summary.stalled, 0);

    jobs::heartbeat_worker(db.pool(), &worker_id).await.unwrap();
    jobs::unregister_worker(db.pool(), &worker_id)
        .await
        .unwrap();
    let summary = jobs::queue_by_kind(db.pool())
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.kind == kind)
        .unwrap();
    assert_eq!(summary.running, 0);
    assert_eq!(summary.stalled, 1);
    assert!(jobs::list_online_workers(db.pool())
        .await
        .unwrap()
        .iter()
        .all(|worker| worker.worker_id != worker_id));
    sqlx::query("UPDATE job_queue SET locked_at = now() - interval '20 minutes' WHERE kind = $1")
        .bind(&kind)
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        jobs::reclaim_stale(db.pool(), time::Duration::minutes(10))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        jobs::queue_by_kind(db.pool())
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.kind == kind)
            .unwrap()
            .pending_due,
        1
    );
    sqlx::query("DELETE FROM job_queue WHERE kind = $1")
        .bind(kind)
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn interactive_lane_has_reserved_claimers() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let background_kind = format!("test_background_{}", uuid::Uuid::now_v7());
    let interactive_kind = format!("test_interactive_{}", uuid::Uuid::now_v7());
    jobs::enqueue(db.pool(), &background_kind, json!({}), None, None, 2)
        .await
        .unwrap();
    jobs::enqueue_in_lane(
        db.pool(),
        jobs::QueueLane::Interactive,
        &interactive_kind,
        json!({}),
        None,
        None,
        2,
    )
    .await
    .unwrap();

    let background = jobs::claim_next_in_lane(
        db.pool(),
        "background#0",
        jobs::QueueLane::Background,
        std::slice::from_ref(&background_kind),
    )
    .await
    .unwrap()
    .unwrap();
    let interactive = jobs::claim_next_in_lane(
        db.pool(),
        "interactive#0",
        jobs::QueueLane::Interactive,
        std::slice::from_ref(&interactive_kind),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(background.kind, background_kind);
    assert_eq!(interactive.kind, interactive_kind);
    assert_eq!(interactive.queue_name, "interactive");

    sqlx::query("DELETE FROM job_queue WHERE kind = ANY($1)")
        .bind(vec![background_kind, interactive_kind])
        .execute(db.pool())
        .await
        .unwrap();
}

/// Helper: make a job runnable immediately again (bypass backoff for the test).
async fn sqlx_set_run_at_now(db: &Db, id: uuid::Uuid) {
    sqlx::query("UPDATE job_queue SET run_at = now() WHERE id = $1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
}
