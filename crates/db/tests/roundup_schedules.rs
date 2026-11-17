//! Personal Roundup schedule/history tenancy, cadence, retention, and cascades.

use time::{Duration, OffsetDateTime, Weekday};
use uuid::Uuid;

use rusty_feed_core::roundup::{RoundupKey, RoundupPeriod, RoundupTiming};
use rusty_feed_db::{
    articles, categories, feeds, roundup_runs, roundup_schedules, roundups, subscriptions, users,
    Db, NewArticle, User,
};

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

async fn subscribed_feed(db: &Db, user_id: Uuid, title: &str) -> rusty_feed_db::Feed {
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://roundup-schedules.example/{}.xml", Uuid::now_v7()),
        title,
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(db.pool(), user_id, feed.id, None)
        .await
        .unwrap();
    feed
}

fn timing(period: RoundupPeriod) -> RoundupTiming {
    RoundupTiming {
        period,
        local_hour: 7,
        weekday: (period == RoundupPeriod::Week).then_some(Weekday::Monday),
        timezone: "Europe/Dublin".into(),
    }
}

#[tokio::test]
async fn schedule_crud_cap_due_claims_and_cross_user_isolation() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let alice = user(&db, "roundup-schedule-alice").await;
    let bob = user(&db, "roundup-schedule-bob").await;
    let first = subscribed_feed(&db, alice.id, "First schedule feed").await;
    let second = subscribed_feed(&db, alice.id, "Second schedule feed").await;
    let folder = categories::create(db.pool(), alice.id, "Schedule folder", None)
        .await
        .unwrap();
    let day = timing(RoundupPeriod::Day);
    let week = timing(RoundupPeriod::Week);

    let daily = match roundup_schedules::create(
        db.pool(),
        roundup_schedules::NewRoundupSchedule {
            user_id: alice.id,
            scope: roundups::RoundupScope::Feed(first.id),
            timing: &day,
            email_delivery: true,
            enabled: true,
        },
        2,
    )
    .await
    .unwrap()
    {
        roundup_schedules::CreateScheduleOutcome::Created(schedule) => *schedule,
        _ => panic!("first schedule should be created"),
    };
    assert_eq!(daily.timezone, "Europe/Dublin");
    assert!(daily.email_delivery);
    assert!(daily.next_run().is_some());
    assert!(matches!(
        roundup_schedules::create(
            db.pool(),
            roundup_schedules::NewRoundupSchedule {
                user_id: alice.id,
                scope: roundups::RoundupScope::Feed(first.id),
                timing: &day,
                email_delivery: false,
                enabled: true,
            },
            2,
        )
        .await
        .unwrap(),
        roundup_schedules::CreateScheduleOutcome::Conflict
    ));
    let weekly = match roundup_schedules::create(
        db.pool(),
        roundup_schedules::NewRoundupSchedule {
            user_id: alice.id,
            scope: roundups::RoundupScope::Folder(folder.id),
            timing: &week,
            email_delivery: false,
            enabled: true,
        },
        2,
    )
    .await
    .unwrap()
    {
        roundup_schedules::CreateScheduleOutcome::Created(schedule) => *schedule,
        _ => panic!("folder schedule should be created"),
    };
    assert!(matches!(
        roundup_schedules::create(
            db.pool(),
            roundup_schedules::NewRoundupSchedule {
                user_id: alice.id,
                scope: roundups::RoundupScope::Feed(second.id),
                timing: &day,
                email_delivery: false,
                enabled: true,
            },
            2,
        )
        .await
        .unwrap(),
        roundup_schedules::CreateScheduleOutcome::CapReached
    ));

    assert!(
        roundup_schedules::find_for_user(db.pool(), bob.id, daily.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(roundup_schedules::list_for_user(db.pool(), bob.id)
        .await
        .unwrap()
        .is_empty());
    assert!(
        !roundup_schedules::set_enabled(db.pool(), bob.id, daily.id, false)
            .await
            .unwrap()
    );
    assert!(!roundup_schedules::delete(db.pool(), bob.id, daily.id)
        .await
        .unwrap());
    assert!(roundup_schedules::update(
        db.pool(),
        bob.id,
        daily.id,
        roundup_schedules::NewRoundupSchedule {
            user_id: bob.id,
            scope: roundups::RoundupScope::Feed(first.id),
            timing: &day,
            email_delivery: false,
            enabled: true,
        },
    )
    .await
    .is_err());

    let now = OffsetDateTime::now_utc();
    sqlx::query("UPDATE roundup_schedules SET created_at = $2 WHERE id = $1")
        .bind(daily.id)
        .bind(now - Duration::days(2))
        .execute(db.pool())
        .await
        .unwrap();
    let mut daily = roundup_schedules::find_for_user(db.pool(), alice.id, daily.id)
        .await
        .unwrap()
        .unwrap();
    let due = roundup_schedules::due_candidates(db.pool(), now, 20)
        .await
        .unwrap();
    assert!(due.iter().any(|schedule| schedule.id == daily.id));
    assert!(!due.iter().any(|schedule| schedule.id == weekly.id));
    let scheduled_for = daily.next_run().unwrap();
    assert!(scheduled_for <= now);
    let claimed = roundup_runs::claim_window(db.pool(), &daily, scheduled_for, now)
        .await
        .unwrap()
        .expect("first claim");
    assert!(
        roundup_runs::claim_window(db.pool(), &daily, scheduled_for, now)
            .await
            .unwrap()
            .is_none()
    );
    assert!(roundup_runs::finish(
        db.pool(),
        alice.id,
        claimed.id,
        "skipped",
        None,
        None,
        Some("Not enough articles."),
    )
    .await
    .unwrap());
    daily = roundup_schedules::find_for_user(db.pool(), alice.id, daily.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(daily.last_status.as_deref(), Some("skipped"));
    assert!(roundup_schedules::due_candidates(db.pool(), now, 20)
        .await
        .unwrap()
        .iter()
        .all(|schedule| schedule.id != daily.id));

    assert!(
        roundup_schedules::set_enabled(db.pool(), alice.id, weekly.id, false)
            .await
            .unwrap()
    );
    assert!(roundup_schedules::delete(db.pool(), alice.id, weekly.id)
        .await
        .unwrap());
    assert_eq!(
        roundup_schedules::list_for_user(db.pool(), alice.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn history_references_shared_rows_retains_ninety_days_and_cascades_with_scope() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let alice = user(&db, "roundup-history-alice").await;
    let bob = user(&db, "roundup-history-bob").await;
    let feed = subscribed_feed(&db, alice.id, "History feed").await;
    subscriptions::subscribe(db.pool(), bob.id, feed.id, None)
        .await
        .unwrap();
    let (article, _) = articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://roundup-history.example/{}", Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: "Shared history source".into(),
            author: None,
            content: "Full body".into(),
            excerpt: Some("Shared excerpt".into()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    let key = RoundupKey::new(&[article.id], RoundupPeriod::Day, "history-model");
    let shared = roundups::insert(
        db.pool(),
        roundups::NewRoundup {
            key: &key,
            provider: "test",
            model: "history-model",
            headline: "One shared headline",
            body_markdown: "Shared body",
            source_ids: &[article.id],
        },
    )
    .await
    .unwrap()
    .unwrap();
    let ready = roundup_runs::record(
        db.pool(),
        roundup_runs::NewRoundupRun {
            user_id: alice.id,
            schedule_id: None,
            scope: roundups::RoundupScope::Feed(feed.id),
            period: RoundupPeriod::Day,
            status: "ready",
            key: Some(&key),
            roundup_id: Some(shared.id),
            skip_reason: None,
            scheduled_for: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(ready.roundup_id, Some(shared.id));
    assert_eq!(
        ready.roundup_headline.as_deref(),
        Some("One shared headline")
    );
    let recent = roundup_runs::record(
        db.pool(),
        roundup_runs::NewRoundupRun {
            user_id: bob.id,
            schedule_id: None,
            scope: roundups::RoundupScope::Feed(feed.id),
            period: RoundupPeriod::Day,
            status: "skipped",
            key: None,
            roundup_id: None,
            skip_reason: Some("Bob's private reason"),
            scheduled_for: None,
        },
    )
    .await
    .unwrap();
    assert!(roundup_runs::list_for_user(db.pool(), bob.id, 30)
        .await
        .unwrap()
        .iter()
        .all(|run| run.user_id == bob.id && run.id != ready.id));

    sqlx::query("UPDATE roundup_runs SET created_at = now() - interval '100 days' WHERE id = $1")
        .bind(ready.id)
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        roundup_runs::purge_older_than(db.pool(), 90).await.unwrap(),
        1
    );
    assert!(roundup_runs::find_for_user(db.pool(), alice.id, ready.id)
        .await
        .unwrap()
        .is_none());
    assert!(roundup_runs::find_for_user(db.pool(), bob.id, recent.id)
        .await
        .unwrap()
        .is_some());
    let shared_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM roundups WHERE id = $1")
        .bind(shared.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        shared_rows, 1,
        "history retention never deletes shared content"
    );

    let day = timing(RoundupPeriod::Day);
    let schedule = match roundup_schedules::create(
        db.pool(),
        roundup_schedules::NewRoundupSchedule {
            user_id: alice.id,
            scope: roundups::RoundupScope::Feed(feed.id),
            timing: &day,
            email_delivery: true,
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
    let now = OffsetDateTime::now_utc();
    let scheduled_for = schedule.next_run().unwrap();
    let run = roundup_runs::claim_window(db.pool(), &schedule, scheduled_for, now)
        .await
        .unwrap()
        .unwrap();
    assert!(roundup_runs::finish(
        db.pool(),
        alice.id,
        run.id,
        "ready",
        Some(&key),
        Some(shared.id),
        None,
    )
    .await
    .unwrap());
    let mail = roundup_runs::claim_pending_email(db.pool())
        .await
        .unwrap()
        .expect("opted-in ready run should be claimable");
    assert_eq!(mail.id, run.id);
    assert_eq!(mail.email, alice.email);
    assert_eq!(mail.headline, "One shared headline");
    assert_eq!(mail.key().unwrap().model_version, "history-model");
    roundup_runs::release_email_claim(db.pool(), run.id)
        .await
        .unwrap();
    assert!(roundup_runs::claim_pending_email(db.pool())
        .await
        .unwrap()
        .is_some());
    assert!(roundup_schedules::delete(db.pool(), alice.id, schedule.id)
        .await
        .unwrap());
    assert!(roundup_runs::find_for_user(db.pool(), alice.id, run.id)
        .await
        .unwrap()
        .unwrap()
        .schedule_id
        .is_none());

    assert!(subscriptions::unsubscribe(db.pool(), alice.id, feed.id)
        .await
        .unwrap());
    assert!(roundup_runs::find_for_user(db.pool(), alice.id, run.id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        roundup_schedules::count_for_user(db.pool(), alice.id)
            .await
            .unwrap(),
        0
    );
}
