//! M10 integration test: PatternDecayAgent decays old weights and prunes tiny
//! ones. Requires `TEST_DATABASE_URL`; skipped otherwise.

use std::time::Duration;

use serde_json::json;
use uuid::Uuid;

use rustss_agents::{Agent, AgentContext, PatternDecayAgent};
use rustss_db::{patterns, users, Db};
use rustss_feeds::SafeFetchClient;

async fn connect() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

#[tokio::test]
async fn decays_old_patterns_and_prunes_tiny() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let user = users::create(
        db.pool(),
        &format!("decay-{}@ex.com", Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap()
    .id;

    // A strong-but-stale pattern, inserted directly so the BEFORE-UPDATE
    // `updated_at` trigger doesn't reset our backdated timestamp (65 days → two
    // 30-day decay periods), plus a tiny recent one.
    sqlx::query(
        "INSERT INTO user_patterns (id, user_id, keyword, weight, updated_at) \
         VALUES ($1, $2, 'decayme', 1.0, now() - interval '65 days')",
    )
    .bind(Uuid::now_v7())
    .bind(user)
    .execute(db.pool())
    .await
    .unwrap();
    patterns::upsert_delta(db.pool(), user, "tiny", 0.05)
        .await
        .unwrap();

    let ctx = AgentContext::new(
        db.clone(),
        SafeFetchClient::new(Duration::from_secs(5)).unwrap(),
    );
    PatternDecayAgent.run(&ctx, json!({})).await.unwrap();

    // 1.0 * 0.9^2 = 0.81.
    let w = patterns::weights_for(db.pool(), user, &["decayme".to_string()])
        .await
        .unwrap();
    assert_eq!(w.len(), 1);
    assert!(
        (w[0].1 - 0.81).abs() < 1e-6,
        "expected 0.81, got {}",
        w[0].1
    );

    // The tiny pattern (< 0.1) was pruned.
    let tiny = patterns::weights_for(db.pool(), user, &["tiny".to_string()])
        .await
        .unwrap();
    assert!(tiny.is_empty(), "sub-threshold pattern should be pruned");
}
