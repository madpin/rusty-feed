//! Database-backed shared Roundup generation and call-unit accounting.

use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rusty_feed_core::ai_budget::AiBudget;
use rusty_feed_core::roundup::{RoundupKey, RoundupPeriod, ROUNDUP_LLM_CALL_WEIGHT};
use rusty_feed_db::{articles, cost_ledger, feeds, roundups, users, Db, NewArticle};
use rusty_feed_llm::{generate_roundup, OpenAiCompatClient};

#[tokio::test]
async fn identical_inputs_share_one_row_and_roundups_count_as_weighted_calls() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url).await.unwrap();
    db.migrate().await.unwrap();
    let first_user = users::create(
        db.pool(),
        &format!("roundup-first-{}@example.test", Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let second_user = users::create(
        db.pool(),
        &format!("roundup-second-{}@example.test", Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://roundup-llm.example/{}.xml", Uuid::now_v7()),
        "Shared feed",
        None,
    )
    .await
    .unwrap();
    let mut ids = Vec::new();
    for index in 0..3 {
        let (article, _) = articles::upsert(
            db.pool(),
            &NewArticle {
                feed_id: feed.id,
                url: format!("https://roundup-llm.example/{index}/{}", Uuid::now_v7()),
                comments_url: None,
                guid: None,
                title: format!("Shared story {index}"),
                author: None,
                content: format!("Full body {index}, never selected"),
                excerpt: Some(format!("Shared excerpt {index}")),
                image_url: None,
                enclosures: Vec::new(),
                published_at: None,
            },
        )
        .await
        .unwrap();
        ids.push(article.id);
    }
    let inputs = roundups::candidates_by_ids(db.pool(), &ids).await.unwrap();
    let key = RoundupKey::new(&ids, RoundupPeriod::Day, "gpt-4o-mini");
    let response = json!({
        "headline": "One cohesive headline",
        "intro": "A short introduction to the important news.",
        "sections": [
            {
                "heading": "The main theme",
                "body": "A cohesive account drawn from the supplied articles.",
                "article_ids": &ids[..2],
            },
            {
                "heading": "Another important development",
                "body": "A second themed account with its own support.",
                "article_ids": [ids[2]],
            },
        ],
    })
    .to_string();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": response}}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 50},
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OpenAiCompatClient::new(
        "openai",
        server.uri(),
        Some("test-key".into()),
        "gpt-4o-mini".into(),
    );

    assert!(
        generate_roundup(&db, &client, Some(first_user.id), &key, &inputs, true)
            .await
            .unwrap()
    );
    assert!(
        !generate_roundup(&db, &client, Some(second_user.id), &key, &inputs, false)
            .await
            .unwrap()
    );

    let stored = roundups::find_by_key(db.pool(), &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.headline, "One cohesive headline");
    assert_eq!(stored.source_ids.len(), 3);
    let shared_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM roundups WHERE input_hash = $1")
            .bind(&key.input_hash)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(shared_rows, 1);
    let ledger_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cost_ledger WHERE kind = 'roundup' AND user_id = ANY($1)",
    )
    .bind(vec![first_user.id, second_user.id])
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(ledger_rows, 1, "the cache hit must not fund a second call");
    assert_eq!(
        cost_ledger::user_llm_call_units_since(db.pool(), first_user.id, AiBudget::today_start(),)
            .await
            .unwrap(),
        ROUNDUP_LLM_CALL_WEIGHT,
        "one Roundup joins the existing summary cap at weighted cost"
    );
    assert_eq!(
        cost_ledger::user_llm_call_units_since(db.pool(), second_user.id, AiBudget::today_start(),)
            .await
            .unwrap(),
        0
    );

    cost_ledger::record(
        db.pool(),
        Some(first_user.id),
        "openai",
        "gpt-4o-mini",
        "summary",
        1,
        1,
        0,
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        cost_ledger::user_llm_call_units_since(db.pool(), first_user.id, AiBudget::today_start(),)
            .await
            .unwrap(),
        ROUNDUP_LLM_CALL_WEIGHT + 1,
        "summary and Roundup usage share one daily total"
    );
}
