//! PostgreSQL full-text and hybrid search integration coverage.
//! Tests skip when `TEST_DATABASE_URL` is unavailable.

use rusty_feed_core::config::SearchTextConfig;
use rusty_feed_db::models::NewArticle;
use rusty_feed_db::{articles, embeddings, feeds, search, subscriptions, users, Db};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect to test database");
    db.migrate().await.expect("run migrations");
    Some(db)
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
        &format!("https://search.example/{}/feed.xml", unique(prefix)),
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

async fn article(db: &Db, feed_id: Uuid, title: &str, content: &str) -> rusty_feed_db::Article {
    article_at(db, feed_id, title, content, None).await
}

async fn article_at(
    db: &Db,
    feed_id: Uuid,
    title: &str,
    content: &str,
    published_at: Option<OffsetDateTime>,
) -> rusty_feed_db::Article {
    articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id,
            url: format!("https://search.example/articles/{}", Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: title.to_string(),
            author: None,
            content: content.to_string(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at,
        },
    )
    .await
    .expect("create article")
    .0
}

#[tokio::test]
async fn keyword_search_supports_phrases_exclusions_language_and_tenant_scope() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let reader = user(&db, "search-reader").await;
    let outsider = user(&db, "search-outsider").await;
    let reader_feed = subscribed_feed(&db, reader, "reader-feed").await;
    let outsider_feed = subscribed_feed(&db, outsider, "outsider-feed").await;

    let expected = article(
        &db,
        reader_feed,
        "Async Rust in Production",
        "A Tokio runtime field guide.",
    )
    .await;
    article(
        &db,
        reader_feed,
        "Async Rust for Beginners",
        "An introductory course.",
    )
    .await;
    article(
        &db,
        outsider_feed,
        "Async Rust in Production",
        "This private subscription must never leak.",
    )
    .await;

    let hits = search::keyword_for_user(
        db.pool(),
        reader,
        "\"async rust\" -beginners",
        SearchTextConfig::Simple,
        20,
    )
    .await
    .expect("search phrase");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, expected.id);
    assert!(hits[0].keyword_score.is_some());
    assert!(hits[0].semantic_score.is_none());

    let stemmed = article(
        &db,
        reader_feed,
        "Systems running smoothly",
        "Operational notes.",
    )
    .await;
    let simple = search::keyword_for_user(db.pool(), reader, "run", SearchTextConfig::Simple, 20)
        .await
        .expect("simple search");
    assert!(!simple.iter().any(|hit| hit.id == stemmed.id));
    let english = search::keyword_for_user(db.pool(), reader, "run", SearchTextConfig::English, 20)
        .await
        .expect("english search");
    assert!(english.iter().any(|hit| hit.id == stemmed.id));

    let alternatives = search::keyword_for_user(
        db.pool(),
        reader,
        "\"async rust\" OR \"systems running\"",
        SearchTextConfig::Simple,
        20,
    )
    .await
    .expect("OR search");
    assert!(alternatives.iter().any(|hit| hit.id == expected.id));
    assert!(alternatives.iter().any(|hit| hit.id == stemmed.id));
}

#[tokio::test]
async fn generated_vectors_follow_updates_and_title_matches_rank_higher() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let reader = user(&db, "search-update").await;
    let feed_id = subscribed_feed(&db, reader, "update-feed").await;
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT indexname FROM pg_indexes \
         WHERE schemaname = current_schema() \
           AND indexname = ANY($1) ORDER BY indexname",
    )
    .bind(vec!["articles_search_idx", "articles_search_simple_idx"])
    .fetch_all(db.pool())
    .await
    .expect("inspect full-text indexes");
    assert_eq!(
        indexes,
        vec![
            "articles_search_idx".to_string(),
            "articles_search_simple_idx".to_string()
        ]
    );
    let title_hit = article(
        &db,
        feed_id,
        "Postgres indexing",
        "A practical database guide.",
    )
    .await;
    let body_hit = article(
        &db,
        feed_id,
        "Database notes",
        "A guide to Postgres indexing.",
    )
    .await;

    let ranked = search::keyword_for_user(
        db.pool(),
        reader,
        "postgres indexing",
        SearchTextConfig::Simple,
        20,
    )
    .await
    .expect("rank keyword hits");
    let title_position = ranked
        .iter()
        .position(|hit| hit.id == title_hit.id)
        .unwrap();
    let body_position = ranked.iter().position(|hit| hit.id == body_hit.id).unwrap();
    assert!(title_position < body_position, "title weighting should win");

    let mut changed = NewArticle {
        feed_id,
        url: title_hit.url.clone(),
        comments_url: None,
        guid: None,
        title: "Cobalt release".to_string(),
        author: None,
        content: "Initial notes.".to_string(),
        excerpt: None,
        image_url: None,
        enclosures: Vec::new(),
        published_at: None,
    };
    articles::upsert(db.pool(), &changed)
        .await
        .expect("update searchable article");
    changed.title = "Amber release".to_string();
    articles::upsert(db.pool(), &changed)
        .await
        .expect("update generated vector");

    let amber = search::keyword_for_user(db.pool(), reader, "amber", SearchTextConfig::Simple, 20)
        .await
        .expect("search updated vector");
    assert!(amber.iter().any(|hit| hit.id == title_hit.id));
    let cobalt =
        search::keyword_for_user(db.pool(), reader, "cobalt", SearchTextConfig::Simple, 20)
            .await
            .expect("search previous vector");
    assert!(!cobalt.iter().any(|hit| hit.id == title_hit.id));
}

#[tokio::test]
async fn hybrid_search_keeps_keyword_only_articles_and_normalizes_cosine_scores() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let reader = user(&db, "search-hybrid").await;
    let feed_id = subscribed_feed(&db, reader, "hybrid-feed").await;
    let keyword_only = article(
        &db,
        feed_id,
        "Quasar observatory",
        "No embedding is available yet.",
    )
    .await;
    let semantic = article(
        &db,
        feed_id,
        "Astronomy report",
        "A meaning-based candidate.",
    )
    .await;
    let opposite = article(
        &db,
        feed_id,
        "Opposite vector",
        "A cosine normalization boundary.",
    )
    .await;
    let mut query_vector = vec![0.0_f32; 384];
    query_vector[0] = 1.0;
    let mut opposite_vector = vec![0.0_f32; 384];
    opposite_vector[0] = -1.0;
    embeddings::upsert(
        db.pool(),
        semantic.id,
        &query_vector,
        "search-test-model",
        semantic.content_revision,
    )
    .await
    .expect("store semantic vector");
    embeddings::upsert(
        db.pool(),
        opposite.id,
        &opposite_vector,
        "search-test-model",
        opposite.content_revision,
    )
    .await
    .expect("store opposite vector");

    let hits = search::hybrid_for_user(
        db.pool(),
        &search::HybridSearch {
            user_id: reader,
            query: "quasar",
            query_vector: &query_vector,
            model: "search-test-model",
            text_config: SearchTextConfig::Simple,
            semantic_weight: 0.6,
            limit: 20,
        },
    )
    .await
    .expect("hybrid search");
    assert!(hits.iter().any(|hit| {
        hit.id == keyword_only.id && hit.keyword_score.is_some() && hit.semantic_score.is_none()
    }));
    assert!(hits.iter().any(|hit| {
        hit.id == semantic.id && hit.semantic_score == Some(1.0) && hit.keyword_score.is_none()
    }));
    let opposite_hit = hits.iter().find(|hit| hit.id == opposite.id).unwrap();
    assert_eq!(opposite_hit.semantic_score, Some(0.0));
}

// Pre-FND-231 keyword SQL, kept verbatim as an oracle: it ranks every match.
const OLD_KEYWORD_SQL: &str =
    "WITH q AS (SELECT websearch_to_tsquery('{dictionary}', $2) AS value), \
     ranked AS ( \
       SELECT a.id, ts_rank_cd(a.{column}, q.value, 32)::float8 AS keyword_score \
       FROM articles a \
       JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
       CROSS JOIN q \
       WHERE a.{column} @@ q.value \
       ORDER BY keyword_score DESC, a.published_at DESC NULLS LAST, a.id \
       LIMIT $3 \
     ) \
     SELECT a.id, a.title, a.url, a.excerpt, a.image_url, a.published_at, \
            COALESCE(uf.custom_name, f.name) AS feed_name, \
            NULL::float8 AS semantic_score, ranked.keyword_score, \
            ranked.keyword_score AS final_score \
     FROM ranked \
     JOIN articles a ON a.id = ranked.id \
     JOIN feeds f ON f.id = a.feed_id \
     JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
     ORDER BY final_score DESC, a.published_at DESC NULLS LAST, a.id";

// Pre-FND-231 hybrid SQL, kept verbatim as an oracle.
const OLD_HYBRID_SQL: &str =
    "WITH q AS (SELECT websearch_to_tsquery('{dictionary}', $2) AS value), \
     keyword AS ( \
       SELECT a.id, ts_rank_cd(a.{column}, q.value, 32)::float8 AS keyword_score \
       FROM articles a \
       JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
       CROSS JOIN q \
       WHERE a.{column} @@ q.value \
       ORDER BY keyword_score DESC, a.published_at DESC NULLS LAST, a.id \
       LIMIT $5 \
     ), \
     semantic AS ( \
       SELECT e.article_id AS id, \
              GREATEST(0::float8, LEAST(1::float8, \
                1 - ((e.embedding <=> $3::vector) / 2))) AS semantic_score \
       FROM article_embeddings e \
       JOIN articles a ON a.id = e.article_id \
       JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
       WHERE e.model = $4 AND e.content_revision = a.content_revision \
       ORDER BY e.embedding <=> $3::vector \
       LIMIT $5 \
     ), \
     candidates AS (SELECT id FROM keyword UNION SELECT id FROM semantic) \
     SELECT a.id, a.title, a.url, a.excerpt, a.image_url, a.published_at, \
            COALESCE(uf.custom_name, f.name) AS feed_name, \
            semantic.semantic_score, keyword.keyword_score, \
            ($6::float8 * COALESCE(semantic.semantic_score, 0) + \
             (1 - $6::float8) * COALESCE(keyword.keyword_score, 0)) AS final_score \
     FROM candidates \
     JOIN articles a ON a.id = candidates.id \
     JOIN feeds f ON f.id = a.feed_id \
     JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
     LEFT JOIN keyword ON keyword.id = candidates.id \
     LEFT JOIN semantic ON semantic.id = candidates.id \
     ORDER BY final_score DESC, a.published_at DESC NULLS LAST, a.id \
     LIMIT $7";

fn oracle_sql(template: &str, config: SearchTextConfig) -> String {
    let (column, dictionary) = match config {
        SearchTextConfig::Simple => ("search_vector_simple", "simple"),
        SearchTextConfig::English => ("search_vector", "english"),
    };
    template
        .replace("{column}", column)
        .replace("{dictionary}", dictionary)
}

async fn old_keyword(
    db: &Db,
    user_id: Uuid,
    query: &str,
    config: SearchTextConfig,
    limit: i64,
) -> Vec<search::SearchHit> {
    sqlx::query_as::<_, search::SearchHit>(&oracle_sql(OLD_KEYWORD_SQL, config))
        .bind(user_id)
        .bind(query)
        .bind(limit)
        .fetch_all(db.pool())
        .await
        .expect("old keyword oracle")
}

async fn old_hybrid(db: &Db, request: &search::HybridSearch<'_>) -> Vec<search::SearchHit> {
    let vector = format!(
        "[{}]",
        request
            .query_vector
            .iter()
            .map(f32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    );
    let result_limit = request.limit.clamp(1, 100);
    sqlx::query_as::<_, search::SearchHit>(&oracle_sql(OLD_HYBRID_SQL, request.text_config))
        .bind(request.user_id)
        .bind(request.query)
        .bind(vector)
        .bind(request.model)
        .bind(result_limit.saturating_mul(4).clamp(result_limit, 400))
        .bind(request.semantic_weight)
        .bind(result_limit)
        .fetch_all(db.pool())
        .await
        .expect("old hybrid oracle")
}

/// Everything a reader can observe about a hit, with scores compared bit-for-bit.
type Observed = (
    Uuid,
    String,
    Option<OffsetDateTime>,
    Option<u64>,
    Option<u64>,
    u64,
);

fn observed(hits: &[search::SearchHit]) -> Vec<Observed> {
    hits.iter()
        .map(|hit| {
            (
                hit.id,
                hit.feed_name.clone(),
                hit.published_at,
                hit.keyword_score.map(f64::to_bits),
                hit.semantic_score.map(f64::to_bits),
                hit.final_score.to_bits(),
            )
        })
        .collect()
}

/// A reader with a varied corpus: repeated terms (rank ties broken by date and
/// id), shared timestamps, missing dates, and an outsider's matching rows.
async fn ranked_corpus(db: &Db, prefix: &str) -> (Uuid, Vec<rusty_feed_db::Article>) {
    let reader = user(db, prefix).await;
    let outsider = user(db, &format!("{prefix}-outsider")).await;
    let first = subscribed_feed(db, reader, "oracle-a").await;
    let second = subscribed_feed(db, reader, "oracle-b").await;
    let outsider_feed = subscribed_feed(db, outsider, "oracle-outsider").await;
    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("base time");
    let rows = [
        (
            first,
            "Async Rust in production",
            "The Tokio runtime field guide for the team.",
            Some(0),
        ),
        (
            first,
            "Async Rust for beginners",
            "The introductory course on async Rust.",
            Some(1),
        ),
        (
            second,
            "Postgres running smoothly",
            "The database runs the indexing jobs.",
            Some(1),
        ),
        (
            second,
            "Quasarine telescope",
            "A rare observation of the quasarine.",
            Some(2),
        ),
        (
            first,
            "Tokio and Postgres",
            "Connection pools for the async runtime.",
            None,
        ),
        (
            second,
            "Rust release notes",
            "The Rust release ships async closures.",
            Some(3),
        ),
        (
            first,
            "Weekend reading",
            "The best of the week in systems.",
            Some(4),
        ),
        (
            second,
            "Async Rust in production",
            "The same title as the first, a later copy.",
            Some(5),
        ),
        (
            first,
            "Golang news",
            "Running Go services next to Rust.",
            None,
        ),
    ];
    let mut created = Vec::new();
    for (feed, title, content, day) in rows {
        let published = day.map(|day| base + Duration::days(day));
        created.push(article_at(db, feed, title, content, published).await);
    }
    article_at(
        db,
        outsider_feed,
        "Async Rust in production",
        "The outsider's private copy.",
        Some(base),
    )
    .await;
    (reader, created)
}

const ORACLE_QUERIES: &[&str] = &[
    "quasarine",
    "\"async rust\"",
    "\"async rust\" OR postgres",
    "rust -beginners",
    "running",
    "the",
    "async rust tokio",
];

#[tokio::test]
async fn bounded_keyword_ranking_matches_the_unbounded_oracle_at_or_below_the_cap() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let (reader, _) = ranked_corpus(&db, "search-oracle").await;
    let mut compared = 0;
    for config in [SearchTextConfig::Simple, SearchTextConfig::English] {
        for query in ORACLE_QUERIES {
            for limit in [20, 3] {
                let old = old_keyword(&db, reader, query, config, limit).await;
                let page = search::keyword_page_for_user(db.pool(), reader, query, config, limit)
                    .await
                    .expect("bounded keyword search");
                assert_eq!(
                    observed(&page.hits),
                    observed(&old),
                    "{config:?} {query} limit {limit}"
                );
                assert!(!page.truncated, "{config:?} {query}");
                let legacy = search::keyword_for_user(db.pool(), reader, query, config, limit)
                    .await
                    .expect("keyword search");
                assert_eq!(observed(&legacy), observed(&old));

                // A cap equal to the full match count is still exact.
                let all = old_keyword(&db, reader, query, config, 100).await;
                if !all.is_empty() {
                    let exact = search::keyword_page_with_cap(
                        db.pool(),
                        reader,
                        query,
                        config,
                        limit,
                        all.len() as i64,
                    )
                    .await
                    .expect("cap at match count");
                    assert_eq!(observed(&exact.hits), observed(&old), "{config:?} {query}");
                    assert!(!exact.truncated, "{config:?} {query}");
                }
                compared += old.len();
            }
        }
    }
    assert!(compared > 40, "the oracle corpus must produce real matches");
}

#[tokio::test]
async fn bounded_hybrid_ranking_matches_the_unbounded_oracle_at_or_below_the_cap() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let (reader, created) = ranked_corpus(&db, "search-hybrid-oracle").await;
    let model = "search-oracle-model";
    for (index, article) in created.iter().enumerate().step_by(2) {
        let mut vector = vec![0.0_f32; 384];
        vector[index % 384] = 1.0;
        vector[(index + 1) % 384] = 0.5;
        embeddings::upsert(
            db.pool(),
            article.id,
            &vector,
            model,
            article.content_revision,
        )
        .await
        .expect("store vector");
    }
    let mut query_vector = vec![0.0_f32; 384];
    query_vector[0] = 1.0;
    for config in [SearchTextConfig::Simple, SearchTextConfig::English] {
        for query in ORACLE_QUERIES {
            for limit in [20, 2] {
                let request = search::HybridSearch {
                    user_id: reader,
                    query,
                    query_vector: &query_vector,
                    model,
                    text_config: config,
                    semantic_weight: 0.4,
                    limit,
                };
                let old = old_hybrid(&db, &request).await;
                let page = search::hybrid_page_for_user(db.pool(), &request)
                    .await
                    .expect("bounded hybrid search");
                assert_eq!(
                    observed(&page.hits),
                    observed(&old),
                    "{config:?} {query} limit {limit}"
                );
                assert!(!page.truncated);
                let legacy = search::hybrid_for_user(db.pool(), &request)
                    .await
                    .expect("hybrid search");
                assert_eq!(observed(&legacy), observed(&old));
            }
        }
    }
}

#[tokio::test]
async fn keyword_cap_ranks_only_the_newest_matches_and_reports_truncation() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let reader = user(&db, "search-cap").await;
    let feed_id = subscribed_feed(&db, reader, "cap-feed").await;
    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("base time");
    // Older articles say "nebula" more often, so they would outrank the newer
    // ones if they were ranked at all. Two share a timestamp (id breaks the
    // tie) and one has no date (sorts last, so it is the first to drop).
    let mut matches = Vec::new();
    for (day, repeats) in [
        (Some(0), 6),
        (Some(1), 5),
        (Some(2), 4),
        (Some(3), 1),
        (Some(3), 1),
        (None, 6),
    ] {
        let content = format!(
            "{} entry{}",
            vec!["nebula"; repeats].join(" "),
            matches.len()
        );
        let published = day.map(|day| base + Duration::days(day));
        matches.push(article_at(&db, feed_id, "Nebula notes", &content, published).await);
    }
    article_at(&db, feed_id, "Unrelated", "Comet tails.", Some(base)).await;

    let mut newest = matches.clone();
    newest.sort_by(|left, right| {
        right
            .published_at
            .is_some()
            .cmp(&left.published_at.is_some())
            .then(right.published_at.cmp(&left.published_at))
            .then(left.id.cmp(&right.id))
    });
    let kept: Vec<Uuid> = newest.iter().take(3).map(|article| article.id).collect();

    let all = old_keyword(&db, reader, "nebula", SearchTextConfig::Simple, 20).await;
    assert_eq!(all.len(), 6);
    let expected: Vec<Observed> = observed(&all)
        .into_iter()
        .filter(|hit| kept.contains(&hit.0))
        .collect();

    for _ in 0..2 {
        let capped = search::keyword_page_with_cap(
            db.pool(),
            reader,
            "nebula",
            SearchTextConfig::Simple,
            20,
            3,
        )
        .await
        .expect("capped search");
        assert!(capped.truncated);
        assert_eq!(
            observed(&capped.hits),
            expected,
            "newest three, ranked as before"
        );
    }
    let at_cap =
        search::keyword_page_with_cap(db.pool(), reader, "nebula", SearchTextConfig::Simple, 20, 6)
            .await
            .expect("cap equals matches");
    assert!(!at_cap.truncated);
    assert_eq!(observed(&at_cap.hits), observed(&all));
    let below =
        search::keyword_page_with_cap(db.pool(), reader, "nebula", SearchTextConfig::Simple, 20, 5)
            .await
            .expect("cap below matches");
    assert!(below.truncated);
    assert_eq!(below.hits.len(), 5);
    assert!(!below.hits.iter().any(|hit| hit.published_at.is_none()));
    let no_match = search::keyword_page_with_cap(
        db.pool(),
        reader,
        "zzzunmatched",
        SearchTextConfig::Simple,
        20,
        1,
    )
    .await
    .expect("no matches");
    assert!(no_match.hits.is_empty() && !no_match.truncated);

    let query_vector = vec![0.0_f32; 384];
    let hybrid = search::hybrid_page_with_cap(
        db.pool(),
        &search::HybridSearch {
            user_id: reader,
            query: "nebula",
            query_vector: &query_vector,
            model: "search-cap-model-without-vectors",
            text_config: SearchTextConfig::Simple,
            semantic_weight: 0.5,
            limit: 20,
        },
        3,
    )
    .await
    .expect("capped hybrid");
    assert!(hybrid.truncated);
    let mut hybrid_ids: Vec<Uuid> = hybrid.hits.iter().map(|hit| hit.id).collect();
    let mut kept_sorted = kept.clone();
    hybrid_ids.sort();
    kept_sorted.sort();
    assert_eq!(hybrid_ids, kept_sorted);
}
