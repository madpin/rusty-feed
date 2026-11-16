//! Media cache write-race coverage (FND-242). Tests skip when
//! `TEST_DATABASE_URL` is unavailable.

use std::time::Duration as StdDuration;

use rusty_feed_db::media_cache::FetchLock;
use rusty_feed_db::{media_cache, Db, DbPoolSettings};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect to test database");
    db.migrate().await.expect("run migrations");
    Some(db)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_stores_leave_one_consistent_row() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let key = format!("race-{}", Uuid::now_v7());
    let url = format!("https://media.example/{key}");
    let expires_at = OffsetDateTime::now_utc() + Duration::hours(1);

    let writers = (0..8).map(|index| {
        let pool = db.pool().clone();
        let (key, url) = (key.clone(), url.clone());
        tokio::spawn(async move {
            if index == 7 {
                media_cache::store_failure(&pool, &key, &url, "unavailable", expires_at).await
            } else {
                let body = vec![index as u8; 64];
                let content_type = format!("audio/x-{index}");
                media_cache::store_success(
                    &pool,
                    &key,
                    &url,
                    Some(&content_type),
                    &body,
                    expires_at,
                )
                .await
            }
        })
    });
    for result in join_all(writers).await {
        result.unwrap().unwrap();
    }

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM media_cache WHERE cache_key = $1")
        .bind(&key)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(rows, 1);
    let entry = media_cache::find_fresh(db.pool(), &key)
        .await
        .unwrap()
        .expect("entry");
    assert_eq!(entry.source_url, url);
    match entry.status.as_str() {
        "failed" => {
            assert!(entry.body.is_none() && entry.content_type.is_none());
            assert_eq!(entry.failure_kind.as_deref(), Some("unavailable"));
        }
        "ready" => {
            let body = entry.body.expect("body");
            let index = body[0];
            assert!(
                body.iter().all(|byte| *byte == index),
                "body from one writer"
            );
            assert_eq!(
                entry.content_type.as_deref(),
                Some(format!("audio/x-{index}").as_str()),
                "content type from the same writer as the body"
            );
            assert!(entry.failure_kind.is_none());
        }
        other => panic!("unexpected status {other}"),
    }
}

async fn join_all<T>(
    handles: impl IntoIterator<Item = tokio::task::JoinHandle<T>>,
) -> Vec<Result<T, tokio::task::JoinError>> {
    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await);
    }
    results
}

/// A fetch lock must never go back to a pool still locked: a released lock
/// frees the key, and so does a guard dropped by a cancelled request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_lock_is_freed_by_release_and_by_drop() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let single = Db::connect_with(
        &std::env::var("TEST_DATABASE_URL").unwrap(),
        &DbPoolSettings::new(1, 5),
    )
    .await
    .unwrap();
    let key = format!("lock-{}", Uuid::now_v7());
    let wait = StdDuration::from_secs(2);

    let first = FetchLock::acquire(single.pool(), &key).await.unwrap();
    // A different session blocks on the held lock.
    assert!(
        tokio::time::timeout(
            StdDuration::from_millis(300),
            FetchLock::acquire(db.media_lock_pool(), &key)
        )
        .await
        .is_err(),
        "second holder must wait while the first holds the lock"
    );
    first.release().await.unwrap();
    let second = tokio::time::timeout(wait, FetchLock::acquire(db.media_lock_pool(), &key))
        .await
        .expect("lock is free after release")
        .unwrap();

    // Cancellation: dropping without release must not leak the lock.
    drop(second);
    let third = tokio::time::timeout(wait, FetchLock::acquire(single.pool(), &key))
        .await
        .expect("lock is free after the guard is dropped")
        .unwrap();
    third.release().await.unwrap();
}
