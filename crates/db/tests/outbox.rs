use rustss_db::{jobs, outbox, Db};

#[tokio::test]
async fn transactional_outbox_commits_dispatches_and_deduplicates() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();
    let suffix = uuid::Uuid::now_v7();
    let kind = format!("outbox-test-{suffix}");
    let dedup = format!("outbox-test-dedup-{suffix}");

    let mut rolled_back = db.pool().begin().await.unwrap();
    let rolled_back_id = outbox::publish(
        &mut rolled_back,
        &kind,
        serde_json::json!({ "_dedup_key": dedup }),
    )
    .await
    .unwrap();
    rolled_back.rollback().await.unwrap();
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM outbox_events WHERE id = $1)")
            .bind(rolled_back_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(!exists);

    let mut committed = db.pool().begin().await.unwrap();
    outbox::publish(
        &mut committed,
        &kind,
        serde_json::json!({ "value": 1, "_dedup_key": dedup }),
    )
    .await
    .unwrap();
    committed.commit().await.unwrap();
    assert_eq!(
        outbox::dispatch(db.pool(), std::slice::from_ref(&kind), 10)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        jobs::count_active_by_dedup(db.pool(), &dedup)
            .await
            .unwrap(),
        1
    );

    let mut repeated = db.pool().begin().await.unwrap();
    outbox::publish(
        &mut repeated,
        &kind,
        serde_json::json!({ "value": 2, "_dedup_key": dedup }),
    )
    .await
    .unwrap();
    repeated.commit().await.unwrap();
    assert_eq!(outbox::dispatch(db.pool(), &[kind], 10).await.unwrap(), 1);
    assert_eq!(
        jobs::count_active_by_dedup(db.pool(), &dedup)
            .await
            .unwrap(),
        1
    );
}
