//! Per-user vector search through the HNSW index (FND-232).
//! Tests skip when `TEST_DATABASE_URL` is unavailable.
//!
//! pgvector's HNSW scan collects only `hnsw.ef_search` (default 40) nearest
//! tuples across the whole shared corpus and the `user_feeds` join filters
//! them afterwards. A reader whose few embedded articles are further from the
//! query than other readers' articles therefore got fewer rows than the limit,
//! or none. These tests force the HNSW plan with session-level planner
//! settings (`enable_seqscan`/`enable_sort` off on every pooled connection),
//! because a small test table otherwise gets an exact sequential scan that
//! hides the bug. With that plan the semantic, saved-search and hybrid tests
//! fail without `hnsw.iterative_scan` (verified while developing FND-232:
//! zero rows instead of three). The related-articles query orders by a joined
//! column, which the HNSW index cannot serve, so it is an exact scan and its
//! test is a regression guard only.

use rusty_feed_core::config::SearchTextConfig;
use rusty_feed_db::models::NewArticle;
use rusty_feed_db::{articles, embeddings, feeds, search, subscriptions, users, Db};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor, PgPool};
use uuid::Uuid;

const MODEL: &str = "vector-scan-test-model";
const TARGET_ARTICLES: usize = 3;
const DECOY_ARTICLES: usize = 120;

async fn test_db() -> Option<(Db, PgPool)> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect to test database");
    db.migrate().await.expect("run migrations");
    let hnsw_only = PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                connection
                    .execute("SET enable_seqscan = off; SET enable_sort = off")
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .expect("connect HNSW-only pool");
    Some((db, hnsw_only))
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::now_v7())
}

async fn user(db: &Db, prefix: &str) -> Uuid {
    users::create(
        db.pool(),
        &format!("{}@example.test", unique(prefix)),
        None,
        None,
        "user",
        None,
    )
    .await
    .expect("create user")
    .id
}

async fn subscribed_feed(db: &Db, user_id: Uuid, prefix: &str) -> Uuid {
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://vector.example/{}/feed.xml", unique(prefix)),
        prefix,
        None,
    )
    .await
    .expect("create feed");
    subscriptions::subscribe(db.pool(), user_id, feed.id, None)
        .await
        .expect("subscribe");
    feed.id
}

async fn embedded_article(db: &Db, feed_id: Uuid, index: usize, vector: &[f32]) -> Uuid {
    let (article, _) = articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id,
            url: format!("https://vector.example/articles/{}", Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: format!("Vector article {index}"),
            author: None,
            content: format!("Vector body {index} {}", Uuid::now_v7()),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .expect("create article");
    embeddings::upsert(
        db.pool(),
        article.id,
        vector,
        MODEL,
        article.content_revision,
    )
    .await
    .expect("store vector");
    article.id
}

/// Deterministic per-run noise so every stored vector is distinct. Exact
/// duplicates accumulate across runs on a shared test database and leave
/// parts of the HNSW graph unreachable, which would test pgvector's graph
/// rather than the query.
struct Noise(u64);

impl Noise {
    fn new() -> Self {
        Self(Uuid::now_v7().as_u128() as u64 | 1)
    }

    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % 10_000) as f32 / 10_000.0 - 0.5
    }

    fn around(&mut self, centre: &[f32], scale: f32) -> Vec<f32> {
        centre
            .iter()
            .map(|value| value + self.next() * scale)
            .collect()
    }
}

/// A reader with three embedded articles pointing away from the query, and
/// many other readers' articles pointing almost exactly at it.
struct Fixture {
    reader: Uuid,
    targets: Vec<Uuid>,
    query: Vec<f32>,
}

async fn fixture(db: &Db) -> Fixture {
    let reader = user(db, "vector-reader").await;
    let outsider = user(db, "vector-outsider").await;
    let reader_feed = subscribed_feed(db, reader, "vector-reader-feed").await;
    let outsider_feed = subscribed_feed(db, outsider, "vector-outsider-feed").await;
    // Random directions per run: independent random vectors in 384 dimensions
    // are nearly orthogonal, so the reader's articles sit far from the query
    // and the decoys sit close to it, away from other tests' axis vectors.
    let mut noise = Noise::new();
    let zero = vec![0.0_f32; 384];
    let query = noise.around(&zero, 1.0);
    for index in 0..DECOY_ARTICLES {
        let vector = noise.around(&query, 0.2);
        embedded_article(db, outsider_feed, index, &vector).await;
    }
    let mut targets = Vec::new();
    let first_target = noise.around(&zero, 1.0);
    for index in 0..TARGET_ARTICLES {
        let vector = if index == 0 {
            first_target.clone()
        } else {
            noise.around(&zero, 1.0)
        };
        targets.push(embedded_article(db, reader_feed, index, &vector).await);
    }
    // Other readers' near-copies of the reader's first article, so "related
    // articles" for it face the same post-filtering problem.
    for index in 0..DECOY_ARTICLES {
        let vector = noise.around(&first_target, 0.02);
        embedded_article(db, outsider_feed, DECOY_ARTICLES + index, &vector).await;
    }
    Fixture {
        reader,
        targets,
        query,
    }
}

fn assert_descending(similarities: &[f64]) {
    assert!(
        similarities.windows(2).all(|pair| pair[0] >= pair[1]),
        "results must stay in exact similarity order: {similarities:?}"
    );
}

macro_rules! setup {
    () => {{
        let Some((db, hnsw_only)) = test_db().await else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let fixture = fixture(&db).await;
        (hnsw_only, fixture)
    }};
}

#[tokio::test]
async fn semantic_search_returns_every_subscribed_match_through_hnsw() {
    let (hnsw_only, fixture) = setup!();
    let mut expected = fixture.targets.clone();
    expected.sort();
    let hits = embeddings::search_for_user(&hnsw_only, fixture.reader, &fixture.query, MODEL, 10)
        .await
        .expect("semantic search");
    let mut ids: Vec<Uuid> = hits.iter().map(|hit| hit.id).collect();
    ids.sort();
    assert_eq!(ids, expected, "min(limit, embedded) rows for the reader");
    assert_descending(&hits.iter().map(|hit| hit.similarity).collect::<Vec<_>>());
    let limited = embeddings::search_for_user(&hnsw_only, fixture.reader, &fixture.query, MODEL, 2)
        .await
        .expect("limited semantic search");
    assert_eq!(limited.len(), 2);
}

#[tokio::test]
async fn saved_search_candidates_return_every_subscribed_match_through_hnsw() {
    let (hnsw_only, fixture) = setup!();
    let candidates =
        embeddings::candidates_for_query(&hnsw_only, fixture.reader, &fixture.query, MODEL, 7, 10)
            .await
            .expect("saved-search candidates");
    assert_eq!(candidates.len(), TARGET_ARTICLES);
    assert_descending(
        &candidates
            .iter()
            .map(|hit| hit.similarity)
            .collect::<Vec<_>>(),
    );
}

#[tokio::test]
async fn related_articles_return_every_subscribed_match_through_hnsw() {
    let (hnsw_only, fixture) = setup!();
    let related =
        embeddings::related_for_user(&hnsw_only, fixture.reader, fixture.targets[0], MODEL, 10)
            .await
            .expect("related articles");
    assert_eq!(related.len(), TARGET_ARTICLES - 1);
    assert_descending(&related.iter().map(|hit| hit.similarity).collect::<Vec<_>>());
}

#[tokio::test]
async fn hybrid_search_keeps_every_semantic_match_through_hnsw() {
    let (hnsw_only, fixture) = setup!();
    let hybrid = search::hybrid_for_user(
        &hnsw_only,
        &search::HybridSearch {
            user_id: fixture.reader,
            query: "zzznomatchingword",
            query_vector: &fixture.query,
            model: MODEL,
            text_config: SearchTextConfig::Simple,
            semantic_weight: 0.5,
            limit: 10,
        },
    )
    .await
    .expect("hybrid search");
    let semantic = hybrid
        .iter()
        .filter(|hit| hit.semantic_score.is_some())
        .count();
    assert_eq!(
        semantic, TARGET_ARTICLES,
        "hybrid keeps every semantic match"
    );
    assert_descending(&hybrid.iter().map(|hit| hit.final_score).collect::<Vec<_>>());
}

#[tokio::test]
async fn vector_scan_settings_apply_inside_the_transaction_only() {
    let Some((db, _)) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let single = PgPoolOptions::new()
        .max_connections(1)
        .connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .expect("single-connection pool");
    // Load pgvector so its settings report real defaults, not placeholders.
    sqlx::query("SELECT '[1]'::vector")
        .execute(&single)
        .await
        .expect("load pgvector");
    for _ in 0..2 {
        let mut transaction = embeddings::begin_vector_scan(&single)
            .await
            .expect("begin vector scan");
        let (iterative, ef): (String, String) = sqlx::query_as(
            "SELECT current_setting('hnsw.iterative_scan'), current_setting('hnsw.ef_search')",
        )
        .fetch_one(&mut *transaction)
        .await
        .expect("read settings");
        assert_eq!(iterative, "relaxed_order");
        assert_eq!(ef, "40", "ef_search keeps the instance default");
        transaction.commit().await.expect("commit");
    }
    // The same pooled connection is back to the instance defaults.
    let (iterative, ef): (String, String) = sqlx::query_as(
        "SELECT current_setting('hnsw.iterative_scan'), current_setting('hnsw.ef_search')",
    )
    .fetch_one(&single)
    .await
    .expect("read settings after commit");
    assert_eq!(iterative, "off");
    assert_eq!(ef, "40");
    drop(db);
}
