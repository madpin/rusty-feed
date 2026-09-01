use async_trait::async_trait;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use uuid::Uuid;

use rusty_feed_db::integrations;

use crate::{Agent, AgentContext};

#[derive(Deserialize)]
struct Payload {
    delivery_id: Uuid,
}

pub struct IntegrationDeliveryAgent;

fn outbound_body(profile: &integrations::IntegrationProfile, payload: &Value) -> Value {
    let message = rusty_feed_core::integrations::render_template(
        &profile.payload_template,
        payload,
        &profile.tags,
    );
    match rusty_feed_core::integrations::provider(&profile.provider)
        .map(|provider| provider.payload_kind)
    {
        Some("discord") => json!({"content": message}),
        Some("slack") => json!({"text": message}),
        Some("telegram") => json!({"text": message, "disable_web_page_preview": false}),
        Some("ntfy") => {
            json!({"topic": "rusty-feed", "title": payload.pointer("/article/title"), "message": message, "tags": profile.tags})
        }
        _ => json!({"rusty-feed": payload, "message": message, "tags": profile.tags}),
    }
}

#[async_trait]
impl Agent for IntegrationDeliveryAgent {
    fn name(&self) -> &'static str {
        "integration_delivery"
    }

    async fn run(&self, ctx: &AgentContext, payload: Value) -> anyhow::Result<Value> {
        let Payload { delivery_id } = serde_json::from_value(payload)?;
        let delivery = integrations::find_delivery(ctx.db.pool(), delivery_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("delivery not found"))?;
        let profile = integrations::find_enabled(ctx.db.pool(), delivery.profile_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("integration profile unavailable"))?;
        let recent = integrations::deliveries_in_last_minute(ctx.db.pool(), profile.id).await?;
        if recent >= i64::from(profile.rate_limit_per_minute) {
            anyhow::bail!("integration profile rate limit reached; retrying later");
        }
        if !integrations::mark_delivering(ctx.db.pool(), delivery_id).await? {
            return Ok(json!({"skipped": "delivery no longer pending"}));
        }
        ctx.fetch.validate_url(&profile.endpoint_url)?;
        let canonical = integrations::payload_for_delivery(ctx.db.pool(), &delivery).await?;
        let body = serde_json::to_vec(&outbound_body(&profile, &canonical))?;
        let credential = profile
            .credential_ciphertext
            .as_deref()
            .map(|ciphertext| ctx.integration_credentials.decrypt_str(ciphertext))
            .transpose()
            .map_err(|_| anyhow::anyhow!("integration credential could not be decrypted"))?;
        let mut request = ctx
            .fetch
            .client_without_redirects()
            .post(&profile.endpoint_url)
            .header("content-type", "application/json")
            .header("x-rusty-feed-event-type", &delivery.event_type)
            .header(
                "x-rusty-feed-payload-version",
                delivery.payload_version.to_string(),
            )
            .body(body.clone());
        match (profile.credential_kind.as_str(), credential.as_deref()) {
            ("bearer", Some(secret)) => request = request.bearer_auth(secret),
            ("basic", Some(secret)) => {
                let (username, password) = secret.split_once(':').unwrap_or((secret, ""));
                request = request.basic_auth(username, Some(password));
            }
            ("hmac", Some(secret)) => {
                let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                    .map_err(|_| anyhow::anyhow!("invalid HMAC credential"))?;
                mac.update(&body);
                let signature: String = mac
                    .finalize()
                    .into_bytes()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                request = request.header("x-rusty-feed-signature", format!("sha256={signature}"));
            }
            _ => {}
        }
        match request.send().await {
            Ok(response) if response.status().is_success() => {
                integrations::mark_delivered(
                    ctx.db.pool(),
                    delivery_id,
                    i32::from(response.status().as_u16()),
                )
                .await?;
                Ok(json!({"status": "delivered", "http_status": response.status().as_u16()}))
            }
            Ok(response) => {
                let status = i32::from(response.status().as_u16());
                let retrying = integrations::mark_failed(
                    ctx.db.pool(),
                    delivery_id,
                    profile.max_attempts,
                    Some(status),
                    "destination returned a non-success status",
                )
                .await?;
                if retrying {
                    anyhow::bail!("destination returned HTTP {status}");
                }
                Ok(json!({"status": "dead", "http_status": status}))
            }
            Err(error) => {
                let retrying = integrations::mark_failed(
                    ctx.db.pool(),
                    delivery_id,
                    profile.max_attempts,
                    None,
                    "destination request failed",
                )
                .await?;
                if retrying {
                    return Err(anyhow::anyhow!("integration request failed: {error}"));
                }
                Ok(json!({"status": "dead"}))
            }
        }
    }
}
