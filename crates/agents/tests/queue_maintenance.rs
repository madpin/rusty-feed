//! QueueMaintenanceAgent through the registry and the job processor (FND-238).
//! Requires `TEST_DATABASE_URL`; skipped otherwise.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use uuid::Uuid;

use rusty_feed_agents::{process, Agent, AgentContext, QueueMaintenanceAgent, Registry};
use rusty_feed_db::{agent_runs, jobs, Db};
use rusty_feed_feeds::SafeFetchClient;

static PURGE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

async fn insert_job(db: &Db, kind: &str, status: &str, age_days: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO job_queue (id, kind, status, created_at, updated_at) \
         VALUES ($1, $2, $3, now() - make_interval(days => $4), now() - make_interval(days => $4))",
    )
    .bind(id)
    .bind(kind)
    .bind(status)
    .bind(age_days)
    .execute(db.pool())
    .await
    .unwrap();
    id
}

async fn insert_outbox(db: &Db, kind: &str, age_days: i32, processed: bool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO outbox_events (id, kind, created_at, processed_at) \
         VALUES ($1, $2, now() - make_interval(days => $3), \
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

async fn surviving(db: &Db, table: &str, ids: &[Uuid]) -> HashSet<Uuid> {
    let sql = match table {
        "job_queue" => "SELECT id FROM job_queue WHERE id = ANY($1)",
        "outbox_events" => "SELECT id FROM outbox_events WHERE id = ANY($1)",
        _ => "SELECT id FROM agent_runs WHERE id = ANY($1)",
    };
    sqlx::query_scalar::<_, Uuid>(sql)
        .bind(ids)
        .fetch_all(db.pool())
        .await
        .unwrap()
        .into_iter()
        .collect()
}

#[test]
fn registry_defaults_include_queue_maintenance() {
    let registry = Registry::with_defaults();
    assert_eq!(registry.kinds().len(), 14);
    let agent = registry
        .get("queue_maintenance")
        .expect("queue_maintenance registered");
    assert_eq!(agent.name(), "queue_maintenance");
}

#[tokio::test]
async fn queue_maintenance_job_purges_finished_rows_and_records_a_run() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = PURGE_LOCK.lock().await;
    let kind = format!("queue-maintenance-test-{}", Uuid::now_v7());

    let old_done = insert_job(&db, &kind, "done", 10).await;
    let old_dead = insert_job(&db, &kind, "dead", 400).await;
    let recent_done = insert_job(&db, &kind, "done", 1).await;
    let old_event = insert_outbox(&db, &kind, 10, true).await;
    let pending_event = insert_outbox(&db, &kind, 400, false).await;
    let old_run = insert_run(&db, &kind, "success", 45).await;
    let running_run = insert_run(&db, &kind, "running", 400).await;

    let registry = Registry::with_defaults();
    let context = ctx(db.clone());
    let job_id = jobs::enqueue(
        db.pool(),
        "queue_maintenance",
        json!({}),
        Some(&format!("queue_maintenance-test-{kind}")),
        None,
        3,
    )
    .await
    .unwrap()
    .expect("enqueued");
    let job = jobs::claim_next(db.pool(), "test-worker", &["queue_maintenance".to_string()])
        .await
        .unwrap()
        .expect("claimed");
    assert_eq!(job.id, job_id);
    process(&context, &registry, job).await;

    assert_eq!(
        jobs::get_status(db.pool(), job_id)
            .await
            .unwrap()
            .as_deref(),
        Some("done")
    );
    assert_eq!(
        surviving(&db, "job_queue", &[old_done, old_dead, recent_done]).await,
        HashSet::from([old_dead, recent_done])
    );
    assert_eq!(
        surviving(&db, "outbox_events", &[old_event, pending_event]).await,
        HashSet::from([pending_event])
    );
    assert_eq!(
        surviving(&db, "agent_runs", &[old_run, running_run]).await,
        HashSet::from([running_run])
    );
    assert!(
        agent_runs::count_by_status(db.pool(), "queue_maintenance", "success")
            .await
            .unwrap()
            >= 1
    );
    let lease: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agent_locks WHERE agent_name = 'queue_maintenance'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(lease, 0, "the singleton lease is released after the run");

    for table in ["job_queue", "outbox_events"] {
        sqlx::query(&format!("DELETE FROM {table} WHERE kind = $1"))
            .bind(&kind)
            .execute(db.pool())
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM agent_runs WHERE agent_name = $1")
        .bind(&kind)
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn queue_maintenance_with_zero_days_deletes_nothing_and_respects_the_lease() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = PURGE_LOCK.lock().await;
    let kind = format!("queue-maintenance-off-{}", Uuid::now_v7());
    let job = insert_job(&db, &kind, "done", 4_000).await;
    let event = insert_outbox(&db, &kind, 4_000, true).await;
    let run = insert_run(&db, &kind, "failed", 4_000).await;
    let context = ctx(db.clone());

    let disabled = QueueMaintenanceAgent::new(0, 0);
    let stats = disabled.run(&context, json!({})).await.unwrap();
    assert_eq!(stats["purged_done_jobs"], 0);
    assert_eq!(stats["purged_processed_outbox_events"], 0);
    assert_eq!(stats["purged_finished_agent_runs"], 0);
    assert_eq!(surviving(&db, "job_queue", &[job]).await.len(), 1);
    assert_eq!(surviving(&db, "outbox_events", &[event]).await.len(), 1);
    assert_eq!(surviving(&db, "agent_runs", &[run]).await.len(), 1);

    let held = rusty_feed_db::agent_locks::acquire(
        db.pool(),
        "queue_maintenance",
        "other-instance",
        time::Duration::minutes(5),
    )
    .await
    .unwrap();
    assert!(held);
    let stats = QueueMaintenanceAgent::default()
        .run(&context, json!({}))
        .await
        .unwrap();
    assert!(stats.get("skipped").is_some(), "{stats}");
    assert_eq!(surviving(&db, "job_queue", &[job]).await.len(), 1);
    rusty_feed_db::agent_locks::release(db.pool(), "queue_maintenance", "other-instance")
        .await
        .unwrap();

    let stats = QueueMaintenanceAgent::default()
        .run(&context, json!({}))
        .await
        .unwrap();
    assert!(stats["purged_done_jobs"].as_u64().unwrap() >= 1);
    assert!(surviving(&db, "job_queue", &[job]).await.is_empty());
    assert!(surviving(&db, "outbox_events", &[event]).await.is_empty());
    assert!(surviving(&db, "agent_runs", &[run]).await.is_empty());

    let registry = {
        let mut registry = Registry::with_defaults();
        registry.register(
            "queue_maintenance",
            Arc::new(QueueMaintenanceAgent::new(0, 0)),
        );
        registry
    };
    assert!(registry.get("queue_maintenance").is_some());
}
