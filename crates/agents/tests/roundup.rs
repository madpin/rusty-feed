//! RoundupAgent execution through the real agent trait and shared repository.

use std::sync::Arc;

use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rusty_feed_agents::{Agent, AgentContext, Registry, RoundupAgent};
use rusty_feed_core::roundup::{RoundupKey, RoundupPeriod};
use rusty_feed_db::{articles, feeds, roundups, users, Db, NewArticle};
use rusty_feed_feeds::SafeFetchClient;
use rusty_feed_llm::OpenAiCompatClient;

#[tokio::test]
async fn agent_generates_once_from_a_queued_payload() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url).await.unwrap();
    db.migrate().await.unwrap();
    let user = users::create(
        db.pool(),
        &format!("roundup-agent-{}@example.test", Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://roundup-agent.example/{}.xml", Uuid::now_v7()),
        "Agent feed",
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
                url: format!("https://roundup-agent.example/{index}/{}", Uuid::now_v7()),
                comments_url: None,
                guid: None,
                title: format!("Agent source {index}"),
                author: None,
                content: format!("Full agent source body {index}"),
                excerpt: Some(format!("Agent shared excerpt {index}")),
                image_url: None,
                enclosures: Vec::new(),
                published_at: None,
            },
        )
        .await
        .unwrap();
        ids.push(article.id);
    }
    let response = json!({
        "headline": "Agent-generated cohesive news",
        "intro": "The short shared introduction.",
        "sections": [
            {
                "heading": "First theme",
                "body": "The first supported theme.",
                "article_ids": &ids[..2],
            },
            {
                "heading": "Second theme",
                "body": "The second supported theme.",
                "article_ids": [ids[2]],
            },
        ],
    })
    .to_string();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"content": response}}],
            "usage": {"prompt_tokens": 20, "completion_tokens": 20},
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
    let agent = RoundupAgent::new(Some(Arc::new(client)));
    let context = AgentContext::new(db.clone(), SafeFetchClient::default());
    let key = RoundupKey::new(&ids, RoundupPeriod::Day, "gpt-4o-mini");
    let payload = json!({
        "input_hash": key.input_hash,
        "period": "day",
        "model_version": "gpt-4o-mini",
        "article_ids": ids,
        "user_id": user.id,
    });

    let outcome = agent.run(&context, payload.clone()).await.unwrap();
    assert_eq!(outcome["written"], true);
    assert!(roundups::find_by_key(db.pool(), &key)
        .await
        .unwrap()
        .is_some());
    let replay = agent.run(&context, payload).await.unwrap();
    assert_eq!(replay["shared"], true, "replay reads the shared winner");
    assert!(Registry::with_defaults().get("roundup").is_some());
}
