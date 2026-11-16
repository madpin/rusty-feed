//! Manual benchmark: re-upsert unchanged stored articles (the common feed
//! refresh). Run against a data copy, never a shared test database:
//! `BENCH_DATABASE_URL=... cargo test -p rusty-feed-db --test ingest_bench --release -- --ignored --nocapture`

use rusty_feed_db::models::{NewArticle, NewArticleEnclosure};
use rusty_feed_db::{articles, Db, UpsertAction};

#[tokio::test]
#[ignore = "manual benchmark; needs BENCH_DATABASE_URL pointing at a data copy"]
async fn reupsert_unchanged_articles() {
    let Ok(url) = std::env::var("BENCH_DATABASE_URL") else {
        eprintln!("skipping: BENCH_DATABASE_URL not set");
        return;
    };
    let limit: i64 = std::env::var("BENCH_ARTICLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(500);
    let db = Db::connect(&url).await.expect("connect");
    let ids: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM articles WHERE feed_content IS NOT NULL ORDER BY id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db.pool())
    .await
    .unwrap();
    let mut inputs = Vec::new();
    for id in ids {
        let stored = articles::find_by_id(db.pool(), id).await.unwrap().unwrap();
        let enclosures = articles::list_enclosures(db.pool(), id)
            .await
            .unwrap()
            .into_iter()
            .map(|e| NewArticleEnclosure {
                url: e.url,
                media_kind: e.media_kind,
                mime_type: e.mime_type,
                title: e.title,
                size_bytes: e.size_bytes,
                duration_seconds: e.duration_seconds,
                width: e.width,
                height: e.height,
            })
            .collect();
        inputs.push((
            NewArticle {
                feed_id: stored.feed_id,
                url: stored.url.clone(),
                comments_url: stored.comments_url.clone(),
                guid: stored.guid.clone(),
                title: stored.title.clone(),
                author: stored.author.clone(),
                content: stored.feed_content.clone().unwrap_or_default(),
                excerpt: stored.feed_excerpt.clone(),
                image_url: stored.image_url.clone(),
                enclosures,
                published_at: stored.published_at,
            },
            stored.original_url.clone(),
            stored.feed_blocked_tracking_pixels,
        ));
    }

    // Warm-up pass so both builds are measured against a hot cache.
    for (new, original, blocked) in &inputs {
        articles::upsert_with_original_url_and_privacy(
            db.pool(),
            new,
            Some(original),
            u32::try_from(*blocked).unwrap_or(0),
        )
        .await
        .unwrap();
    }
    let mut timings = Vec::new();
    let mut skipped = 0;
    for _ in 0..3 {
        skipped = 0;
        let started = std::time::Instant::now();
        for (new, original, blocked) in &inputs {
            let (_, action) = articles::upsert_with_original_url_and_privacy(
                db.pool(),
                new,
                Some(original),
                u32::try_from(*blocked).unwrap_or(0),
            )
            .await
            .unwrap();
            skipped += usize::from(action == UpsertAction::Skipped);
        }
        timings.push(started.elapsed());
    }
    println!(
        "re-upserted {} articles ({skipped} skipped) x3 passes: {timings:?}",
        inputs.len()
    );
}
