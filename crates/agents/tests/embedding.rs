//! M9 integration tests: embedding agent + vector search. Require
//! `TEST_DATABASE_URL`; skipped otherwise.

use std::time::Duration;

use serde_json::json;

use rustss_agents::{Agent, AgentContext, EmbedArticleAgent, FetchFeedAgent};
use rustss_db::models::NewArticle;
use rustss_db::{articles, embeddings, feeds, jobs, subscriptions, users, Db};
use rustss_embed::{Embedder, HashEmbedder};
use rustss_feeds::SafeFetchClient;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

async fn make_user(db: &Db) -> Uuid {
    users::create(
        db.pool(),
        &format!("embed-{}@ex.com", Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap()
    .id
}

async fn add_article(db: &Db, feed_id: Uuid, title: &str, body: &str) -> Uuid {
    let (a, _) = articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id,
            url: format!("https://ex.com/{}", Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: title.into(),
            author: None,
            content: body.into(),
            excerpt: Some(body.into()),
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    a.id
}

#[tokio::test]
async fn embed_article_agent_stores_vector_once() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let url = format!("https://ex.com/{}.xml", Uuid::now_v7());
    let (feed, _) = feeds::upsert_by_url(db.pool(), &url, "F", None)
        .await
        .unwrap();
    let id = add_article(&db, feed.id, "Async Rust", "Tokio and async/await in Rust.").await;

    let out = EmbedArticleAgent::default()
        .run(&ctx(db.clone()), json!({ "article_id": id }))
        .await
        .unwrap();
    assert_eq!(out["status"], "embedded");
    let article = articles::find_by_id(db.pool(), id).await.unwrap().unwrap();
    assert!(embeddings::exists(
        db.pool(),
        id,
        HashEmbedder::default().model(),
        article.content_revision,
    )
    .await
    .unwrap());

    // Idempotent.
    let out2 = EmbedArticleAgent::default()
        .run(&ctx(db.clone()), json!({ "article_id": id }))
        .await
        .unwrap();
    assert_eq!(out2["skipped"], "already embedded");
}

#[tokio::test]
async fn semantic_search_orders_by_similarity_and_guards_model() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let embedder = HashEmbedder::default();
    let model = embedder.model().to_string();

    let user = make_user(&db).await;
    let url = format!("https://ex.com/{}.xml", Uuid::now_v7());
    let (feed, _) = feeds::upsert_by_url(db.pool(), &url, "F", None)
        .await
        .unwrap();
    subscriptions::subscribe(db.pool(), user, feed.id, None)
        .await
        .unwrap();

    let rust = add_article(
        &db,
        feed.id,
        "Async Rust",
        "concurrency with async await in the rust programming language",
    )
    .await;
    let pg = add_article(
        &db,
        feed.id,
        "Postgres",
        "database indexes and query planning in postgres",
    )
    .await;
    let pasta = add_article(
        &db,
        feed.id,
        "Pasta",
        "authentic italian pasta and tomato recipe",
    )
    .await;
    for id in [rust, pg, pasta] {
        let art = articles::find_by_id(db.pool(), id).await.unwrap().unwrap();
        let v = embedder.embed_one(&art.content).await.unwrap();
        embeddings::upsert(db.pool(), id, &v, &model, art.content_revision)
            .await
            .unwrap();
    }

    // A query about async rust should rank the Rust article first.
    let q = embedder.embed_one("async rust concurrency").await.unwrap();
    let hits = embeddings::search_for_user(db.pool(), user, &q, &model, 10)
        .await
        .unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].id, rust, "most similar article should rank first");

    // Model guard: a different model space returns nothing.
    let none = embeddings::search_for_user(db.pool(), user, &q, "other-model", 10)
        .await
        .unwrap();
    assert!(none.is_empty(), "results must not cross embedding spaces");
}

#[tokio::test]
async fn fetch_feed_enqueues_embed_when_enabled() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let run = Uuid::now_v7().to_string();
    let rss = format!(
        r#"<?xml version="1.0"?><rss version="2.0"><channel><title>F</title><link>https://m.ex</link>
        <item><title>Item</title><link>https://m.ex/{run}</link><guid>{run}</guid><description>hi</description></item>
        </channel></rss>"#
    );
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss))
        .mount(&server)
        .await;

    let feed_url = format!("{}/feed.xml?{run}", server.uri());
    let (feed, _) = feeds::upsert_by_url(db.pool(), &feed_url, "F", None)
        .await
        .unwrap();

    // embed_on_ingest = true; feed does NOT enable extraction → embed directly.
    let context = ctx(db.clone()).with_embed_on_ingest(true);
    let stats = FetchFeedAgent
        .run(&context, json!({ "feed_id": feed.id }))
        .await
        .unwrap();
    assert_eq!(stats["created"], 1);
    assert_eq!(stats["embed_jobs"], 1);

    let art = &articles::list_for_feed(db.pool(), feed.id, 5)
        .await
        .unwrap()[0];
    let dedup = format!("embed:{}:{}", art.id, art.content_revision);
    assert_eq!(
        jobs::count_active_by_dedup(db.pool(), &dedup)
            .await
            .unwrap(),
        1
    );
}
