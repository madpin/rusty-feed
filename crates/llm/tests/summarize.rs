//! M12 tests: OpenAI-compatible client + summarize orchestration. The DB tests
//! require `TEST_DATABASE_URL`; the client/summarize tests need only wiremock.

use std::sync::Arc;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rusty_feed_core::ai_budget::AiBudget;
use rusty_feed_db::models::NewArticle;
use rusty_feed_db::{articles, cost_ledger, feeds, llm_profiles, users, Db};
use rusty_feed_llm::{
    summarize, summarize_article, summarize_article_with_cascade, LlmClient, OpenAiCompatClient,
    ProviderCascade,
};

async fn mock_llm(content: &str) -> (MockServer, OpenAiCompatClient) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "role": "assistant", "content": content } }],
            "usage": { "prompt_tokens": 100, "completion_tokens": 50 },
        })))
        .mount(&server)
        .await;
    let client = OpenAiCompatClient::new(
        "openai",
        server.uri(),
        Some("k".into()),
        "gpt-4o-mini".into(),
    );
    (server, client)
}

#[tokio::test]
async fn completes_and_reports_usage() {
    let (_s, client) = mock_llm("hello").await;
    let c = client.complete("sys", "user", 100).await.unwrap();
    assert_eq!(c.text, "hello");
    assert_eq!(c.input_tokens, 100);
    assert_eq!(c.output_tokens, 50);
}

#[tokio::test]
async fn summarize_parses_json_response() {
    let (_s, client) = mock_llm(r#"{"summary":"S","key_points":["a","b"],"topics":["t"]}"#).await;
    let (summary, _) = summarize(&client, "Title", "Body").await.unwrap();
    assert_eq!(summary.summary, "S");
    assert_eq!(summary.key_points, vec!["a", "b"]);
    assert_eq!(summary.topics, vec!["t"]);
}

async fn seed_article(db: &Db) -> rusty_feed_db::Article {
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://ex.com/{}.xml", uuid::Uuid::now_v7()),
        "F",
        None,
    )
    .await
    .unwrap();
    let (a, _) = articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://ex.com/{}", uuid::Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: "Title".into(),
            author: None,
            content: "Some article content to summarize.".into(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();
    a
}

#[tokio::test]
async fn summarize_article_stores_once_and_records_cost() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url).await.unwrap();
    db.migrate().await.unwrap();
    let (_s, client) = mock_llm(r#"{"summary":"Done","key_points":["k"],"topics":["x"]}"#).await;

    let user = users::create(
        db.pool(),
        &format!("llm-{}@ex.com", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap()
    .id;
    let article = seed_article(&db).await;

    let written = summarize_article(&db, &client, Some(user), &article, false)
        .await
        .unwrap();
    assert!(written);
    assert!(articles::is_summarized(db.pool(), article.id)
        .await
        .unwrap());
    let stored = articles::get_summary(db.pool(), article.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.summary, "Done");

    // gpt-4o-mini: 100 input + 50 output → 100*0.15 + 50*0.60 = 45 micro-USD.
    assert!(cost_ledger::total_micro_usd(db.pool()).await.unwrap() >= 45);

    // Second call is a no-op (already summarized).
    assert!(
        !summarize_article(&db, &client, Some(user), &article, false)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn summarize_article_is_first_writer_wins_under_concurrency() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url).await.unwrap();
    db.migrate().await.unwrap();
    let (_s, client) = mock_llm(r#"{"summary":"C","key_points":[],"topics":[]}"#).await;
    let client: Arc<dyn LlmClient> = Arc::new(client);
    let article = seed_article(&db).await;

    let mut handles = Vec::new();
    for _ in 0..4 {
        let db = db.clone();
        let client = client.clone();
        let article = article.clone();
        handles.push(tokio::spawn(async move {
            summarize_article(&db, client.as_ref(), None, &article, false)
                .await
                .unwrap()
        }));
    }
    let mut wrote = 0;
    for h in handles {
        if h.await.unwrap() {
            wrote += 1;
        }
    }
    assert_eq!(wrote, 1, "exactly one concurrent writer should win");
}

#[tokio::test]
async fn managed_provider_cascade_prefers_user_then_falls_back_to_admin() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url).await.unwrap();
    db.migrate().await.unwrap();
    let user = users::create(
        db.pool(),
        &format!("cascade-{}@ex.com", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let failing = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&failing)
        .await;
    let working = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "{\"summary\":\"Fallback worked\",\"key_points\":[],\"topics\":[]}" } }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 10 }
        })))
        .expect(1)
        .mount(&working)
        .await;
    let secret = b"cascade-test-encryption-secret";
    let cryptor = rusty_feed_core::crypto::Cryptor::new_with_context(
        secret,
        b"rusty-feed/llm-provider-credentials/v1",
    );
    let encrypted = cryptor.encrypt_str("test-key").unwrap();
    llm_profiles::create(
        db.pool(),
        Some(user.id),
        "user",
        "Unavailable user provider",
        "openai",
        "user-model",
        &failing.uri(),
        Some(&encrypted),
        true,
        10,
    )
    .await
    .unwrap();
    llm_profiles::create(
        db.pool(),
        None,
        "admin",
        "Working instance provider",
        "openai",
        "admin-model",
        &working.uri(),
        Some(&encrypted),
        true,
        10,
    )
    .await
    .unwrap();
    let article = seed_article(&db).await;
    let cascade = ProviderCascade::new(db.clone(), secret, None);
    let (written, model) =
        summarize_article_with_cascade(&cascade, &AiBudget::default(), Some(user.id), &article)
            .await
            .unwrap();
    assert!(written);
    assert_eq!(model, "admin-model");
}

#[tokio::test]
async fn per_user_daily_call_cap_blocks_further_summaries() {
    // A stranger must not be able to summarize thousands of distinct articles:
    // the per-user daily call cap blocks the second call (FND-187 §4.1).
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url).await.unwrap();
    db.migrate().await.unwrap();
    let user = users::create(
        db.pool(),
        &format!("cap-{}@ex.com", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let working = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "{\"summary\":\"ok\",\"key_points\":[],\"topics\":[]}" } }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 5 }
        })))
        .mount(&working)
        .await;
    let secret = b"cap-test-encryption-secret";
    let cryptor = rusty_feed_core::crypto::Cryptor::new_with_context(
        secret,
        b"rusty-feed/llm-provider-credentials/v1",
    );
    let encrypted = cryptor.encrypt_str("test-key").unwrap();
    llm_profiles::create(
        db.pool(),
        None,
        "admin",
        "Instance provider",
        "openai",
        "admin-model",
        &working.uri(),
        Some(&encrypted),
        true,
        10,
    )
    .await
    .unwrap();
    let cascade = ProviderCascade::new(db.clone(), secret, None);
    let budget = AiBudget::from_config(0.0, 0.0, 1, 0); // 1 LLM call/user/day.

    let article1 = seed_article(&db).await;
    let (written, _) = summarize_article_with_cascade(&cascade, &budget, Some(user.id), &article1)
        .await
        .unwrap();
    assert!(written);

    let article2 = seed_article(&db).await;
    let result = summarize_article_with_cascade(&cascade, &budget, Some(user.id), &article2).await;
    assert!(
        matches!(result, Err(rusty_feed_llm::SummarizeError::Budget(_))),
        "the second summary must be blocked by the per-user daily cap"
    );
    assert!(!articles::is_summarized(db.pool(), article2.id)
        .await
        .unwrap());
}
