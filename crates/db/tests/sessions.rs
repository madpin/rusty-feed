//! Session lookup integration coverage (FND-241). Tests skip when
//! `TEST_DATABASE_URL` is unavailable.

use rusty_feed_db::{sessions, users, Db};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect to test database");
    db.migrate().await.expect("run migrations");
    Some(db)
}

async fn user_with_session(db: &Db, ttl: Duration) -> (rusty_feed_db::User, String) {
    let user = users::create(
        db.pool(),
        &format!("sess-{}@example.test", Uuid::now_v7()),
        Some("Session Tester"),
        None,
        "user",
        Some("hash"),
    )
    .await
    .expect("create user");
    let token_hash = format!("hash-{}", Uuid::now_v7());
    sessions::create(
        db.pool(),
        user.id,
        &token_hash,
        OffsetDateTime::now_utc() + ttl,
    )
    .await
    .expect("create session");
    (user, token_hash)
}

#[tokio::test]
async fn joined_lookup_matches_the_two_query_oracle() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let (_, token_hash) = user_with_session(&db, Duration::hours(1)).await;

    let oracle_session = sessions::find_valid(db.pool(), &token_hash)
        .await
        .unwrap()
        .expect("session");
    let oracle_user = users::find_by_id(db.pool(), oracle_session.user_id)
        .await
        .unwrap()
        .expect("user");
    let joined = sessions::find_valid_with_user(db.pool(), &token_hash)
        .await
        .unwrap()
        .expect("joined");

    assert_eq!(
        format!("{:?}", joined.session),
        format!("{oracle_session:?}")
    );
    assert_eq!(format!("{:?}", joined.user), format!("{oracle_user:?}"));
}

#[tokio::test]
async fn joined_lookup_rejects_unknown_expired_and_deleted_user() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    assert!(sessions::find_valid_with_user(db.pool(), "no-such-hash")
        .await
        .unwrap()
        .is_none());

    let (_, expired) = user_with_session(&db, Duration::hours(1)).await;
    sqlx::query(
        "UPDATE sessions SET expires_at = now() - interval '1 second' WHERE token_hash = $1",
    )
    .bind(&expired)
    .execute(db.pool())
    .await
    .unwrap();
    assert!(sessions::find_valid_with_user(db.pool(), &expired)
        .await
        .unwrap()
        .is_none());

    let (user, token_hash) = user_with_session(&db, Duration::hours(1)).await;
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user.id)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(sessions::find_valid_with_user(db.pool(), &token_hash)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn touch_is_throttled_to_once_a_minute() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let (_, token_hash) = user_with_session(&db, Duration::hours(1)).await;
    let fresh = sessions::find_valid(db.pool(), &token_hash)
        .await
        .unwrap()
        .unwrap();

    sessions::touch(db.pool(), fresh.id).await.unwrap();
    let after_fresh_touch = sessions::find_valid(db.pool(), &token_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_fresh_touch.last_used_at, fresh.last_used_at);

    sqlx::query("UPDATE sessions SET last_used_at = now() - interval '2 minutes' WHERE id = $1")
        .bind(fresh.id)
        .execute(db.pool())
        .await
        .unwrap();
    let stale = sessions::find_valid(db.pool(), &token_hash)
        .await
        .unwrap()
        .unwrap();
    sessions::touch(db.pool(), fresh.id).await.unwrap();
    let refreshed = sessions::find_valid(db.pool(), &token_hash)
        .await
        .unwrap()
        .unwrap();
    assert!(refreshed.last_used_at > stale.last_used_at + Duration::seconds(90));
}
