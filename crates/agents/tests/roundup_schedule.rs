//! Scheduled Roundup fan-out, budget skips, and shared-generation proof.

use std::sync::Arc;

use serde_json::json;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rusty_feed_agents::{Agent, AgentContext, Registry, RoundupAgent, RoundupScheduleAgent};
use rusty_feed_core::ai_budget::AiBudget;
use rusty_feed_core::roundup::{RoundupKey, RoundupPeriod, RoundupTiming};
use rusty_feed_db::{
    articles, cost_ledger, feeds, jobs, roundup_runs, roundup_schedules, roundups, subscriptions,
    users, Db, NewArticle, User,
};
use rusty_feed_feeds::SafeFetchClient;
use rusty_feed_llm::{OpenAiCompatClient, ProviderCascade};

static SCHEDULE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn connect() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn user(db: &Db, label: &str) -> User {
    users::create(
        db.pool(),
        &format!("{label}-{}@example.test", Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap()
}

async fn shared_feed(
    db: &Db,
    users: &[User],
    article_count: usize,
) -> (rusty_feed_db::Feed, Vec<Uuid>) {
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!(
            "https://roundup-scheduled-agent.example/{}.xml",
            Uuid::now_v7()
        ),
        "Scheduled shared feed",
        None,
    )
    .await
    .unwrap();
    for user in users {
        subscriptions::subscribe(db.pool(), user.id, feed.id, None)
            .await
            .unwrap();
    }
    let mut ids = Vec::new();
    for index in 0..article_count {
        let (article, _) = articles::upsert(
            db.pool(),
            &NewArticle {
                feed_id: feed.id,
                url: format!(
                    "https://roundup-scheduled-agent.example/{index}/{}",
                    Uuid::now_v7()
                ),
                comments_url: None,
                guid: None,
                title: format!("Scheduled source {index}"),
                author: None,
                content: format!("Full body {index}"),
                excerpt: Some(format!("Shared excerpt {index}")),
                image_url: None,
                enclosures: Vec::new(),
                published_at: None,
            },
        )
        .await
        .unwrap();
        ids.push(article.id);
    }
    (feed, ids)
}

async fn due_schedule(db: &Db, user_id: Uuid, feed_id: Uuid) -> roundup_schedules::RoundupSchedule {
    let timing = RoundupTiming {
        period: RoundupPeriod::Day,
        local_hour: 7,
        weekday: None,
        timezone: "Europe/Dublin".into(),
    };
    let schedule = match roundup_schedules::create(
        db.pool(),
        roundup_schedules::NewRoundupSchedule {
            user_id,
            scope: roundups::RoundupScope::Feed(feed_id),
            timing: &timing,
            email_delivery: false,
            enabled: true,
        },
        10,
    )
    .await
    .unwrap()
    {
        roundup_schedules::CreateScheduleOutcome::Created(schedule) => *schedule,
        _ => panic!("schedule should be created"),
    };
    sqlx::query("UPDATE roundup_schedules SET created_at = $2 WHERE id = $1")
        .bind(schedule.id)
        .bind(OffsetDateTime::now_utc() - Duration::days(2))
        .execute(db.pool())
        .await
        .unwrap();
    roundup_schedules::find_for_user(db.pool(), user_id, schedule.id)
        .await
        .unwrap()
        .unwrap()
}

fn scheduler(db: &Db, client: Arc<OpenAiCompatClient>) -> RoundupScheduleAgent {
    RoundupScheduleAgent::new(
        Some(Arc::new(ProviderCascade::new(
            db.clone(),
            b"roundup-schedule-test-secret",
            Some(client),
        ))),
        None,
        None,
        "https://feeds.example".into(),
    )
}

#[tokio::test]
async fn due_schedules_enqueue_once_and_two_users_share_one_generation() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = SCHEDULE_TEST_LOCK.lock().await;
    let alice = user(&db, "roundup-scheduled-alice").await;
    let bob = user(&db, "roundup-scheduled-bob").await;
    let users = vec![alice.clone(), bob.clone()];
    let (feed, ids) = shared_feed(&db, &users, 3).await;
    due_schedule(&db, alice.id, feed.id).await;
    due_schedule(&db, bob.id, feed.id).await;
    let model = format!("scheduled-model-{}", Uuid::now_v7());
    let response = json!({
        "headline": "Scheduled cohesive news",
        "intro": "One shared introduction.",
        "sections": [
            {"heading": "First theme", "body": "First supported account.", "article_ids": &ids[..2]},
            {"heading": "Second theme", "body": "Second supported account.", "article_ids": [ids[2]]},
        ],
    })
    .to_string();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"content": response}}],
            "usage": {"prompt_tokens": 20, "completion_tokens": 20},
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = Arc::new(OpenAiCompatClient::new(
        "openai",
        server.uri(),
        Some("test-key".into()),
        model.clone(),
    ));
    let scheduler = scheduler(&db, client.clone());
    let context = AgentContext::new(db.clone(), SafeFetchClient::default());

    let first = scheduler.run(&context, json!({})).await.unwrap();
    assert_eq!(first["enqueued"], 2, "both personal waits are queued");
    let key = RoundupKey::new(&ids, RoundupPeriod::Day, &model);
    let jobs_for_key: i64 =
        sqlx::query_scalar("SELECT count(*) FROM job_queue WHERE dedup_key = $1")
            .bind(key.dedup_key())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        jobs_for_key, 1,
        "identical inputs have one active shared job"
    );
    for user in &users {
        let history = roundup_runs::list_for_user(db.pool(), user.id, 10)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, "queued");
    }

    // Another scheduler pass in the same window neither claims nor enqueues again.
    scheduler.run(&context, json!({})).await.unwrap();
    let runs_for_users: i64 =
        sqlx::query_scalar("SELECT count(*) FROM roundup_runs WHERE user_id = ANY($1)")
            .bind(vec![alice.id, bob.id])
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(runs_for_users, 2);

    let (job_id, payload): (Uuid, serde_json::Value) =
        sqlx::query_as("SELECT id, payload FROM job_queue WHERE dedup_key = $1")
            .bind(key.dedup_key())
            .fetch_one(db.pool())
            .await
            .unwrap();
    let generation = RoundupAgent::new(Some(client));
    assert_eq!(
        generation.run(&context, payload).await.unwrap()["written"],
        true
    );
    jobs::complete(db.pool(), job_id).await.unwrap();

    let reconciled = scheduler.run(&context, json!({})).await.unwrap();
    assert_eq!(reconciled["ready"], 2);
    let shared = roundups::find_by_key(db.pool(), &key)
        .await
        .unwrap()
        .unwrap();
    for user in &users {
        let history = roundup_runs::list_for_user(db.pool(), user.id, 10)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, "ready");
        assert_eq!(history[0].roundup_id, Some(shared.id));
    }
    let ledger_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cost_ledger WHERE kind = 'roundup' AND model = $1",
    )
    .bind(&model)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        ledger_rows, 1,
        "two subscribers still fund one provider call"
    );
    assert!(Registry::with_defaults().get("roundup_schedule").is_some());
}

#[tokio::test]
async fn over_budget_schedule_records_a_skip_without_a_job_or_new_spend() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let _guard = SCHEDULE_TEST_LOCK.lock().await;
    let user = user(&db, "roundup-scheduled-budget").await;
    let users = vec![user.clone()];
    let (feed, _) = shared_feed(&db, &users, 3).await;
    due_schedule(&db, user.id, feed.id).await;
    let model = format!("scheduled-budget-model-{}", Uuid::now_v7());
    cost_ledger::record(
        db.pool(),
        Some(user.id),
        "openai",
        &model,
        "roundup",
        10,
        10,
        0,
        true,
    )
    .await
    .unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let client = Arc::new(OpenAiCompatClient::new(
        "openai",
        server.uri(),
        Some("test-key".into()),
        model.clone(),
    ));
    let scheduler = scheduler(&db, client);
    let context = AgentContext::new(db.clone(), SafeFetchClient::default())
        .with_ai_budget(AiBudget::from_config(0.0, 0.0, 5, 0));

    let outcome = scheduler.run(&context, json!({})).await.unwrap();
    assert_eq!(outcome["enqueued"], 0);
    assert_eq!(outcome["skipped"], 1);
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM job_queue WHERE kind = 'roundup' AND payload ->> 'user_id' = $1",
    )
    .bind(user.id.to_string())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(queued, 0);
    let history = roundup_runs::list_for_user(db.pool(), user.id, 10)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, "skipped");
    assert!(history[0]
        .skip_reason
        .as_deref()
        .unwrap()
        .contains("budget"));
    let ledger_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cost_ledger WHERE kind = 'roundup' AND model = $1",
    )
    .bind(&model)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(ledger_rows, 1, "only the pre-existing budget row remains");
}
