use std::time::Duration;

use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rustss_agents::{Agent, AgentContext, IntegrationDeliveryAgent};
use rustss_core::crypto::Cryptor;
use rustss_db::{integrations, users, Db};
use rustss_feeds::SafeFetchClient;

async fn connect() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn profile(
    db: &Db,
    endpoint: &str,
    secret: &str,
    max_attempts: i32,
) -> integrations::IntegrationProfile {
    let user = users::create(
        db.pool(),
        &format!("delivery-{}@example.test", uuid::Uuid::now_v7()),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let cryptor = Cryptor::new(b"rustss-agent-test-integration-key");
    integrations::create_profile(
        db.pool(),
        user.id,
        "Signed workflow",
        "webhook",
        endpoint,
        Some(&cryptor.encrypt_str(secret).unwrap()),
        "hmac",
        "{{event}} {{title}} {{tags}}",
        &["research".into()],
        &["article.saved".into()],
        max_attempts,
        30,
        true,
    )
    .await
    .unwrap()
}

fn context(db: Db) -> AgentContext {
    AgentContext::new(
        db,
        SafeFetchClient::new_allow_private(Duration::from_secs(5)).unwrap(),
    )
}

#[tokio::test]
async fn signed_delivery_records_success_and_uses_versioned_payload() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/events"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let profile = profile(&db, &format!("{}/events", server.uri()), "top-secret", 3).await;
    let delivery = integrations::queue_delivery(db.pool(), &profile, None, "article.saved")
        .await
        .unwrap();

    let output = IntegrationDeliveryAgent
        .run(&context(db.clone()), json!({"delivery_id": delivery.id}))
        .await
        .unwrap();
    assert_eq!(output["status"], "delivered");

    let stored = integrations::find_delivery(db.pool(), delivery.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, "delivered");
    assert_eq!(stored.response_status, Some(204));

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        request
            .headers
            .get("x-rustss-payload-version")
            .unwrap()
            .to_str()
            .unwrap(),
        "1"
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(b"top-secret").unwrap();
    mac.update(&request.body);
    let expected: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(
        request
            .headers
            .get("x-rustss-signature")
            .unwrap()
            .to_str()
            .unwrap(),
        format!("sha256={expected}")
    );
}

#[tokio::test]
async fn exhausted_delivery_becomes_dead_without_breaking_the_agent() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/unavailable"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let profile = profile(&db, &format!("{}/unavailable", server.uri()), "secret", 1).await;
    let delivery = integrations::queue_delivery(db.pool(), &profile, None, "test")
        .await
        .unwrap();

    let output = IntegrationDeliveryAgent
        .run(&context(db.clone()), json!({"delivery_id": delivery.id}))
        .await
        .unwrap();
    assert_eq!(output["status"], "dead");
    let stored = integrations::find_delivery(db.pool(), delivery.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, "dead");
    assert_eq!(stored.response_status, Some(503));
    assert!(stored.error_summary.is_some());
}
