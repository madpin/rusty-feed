use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use moka::future::Cache as MokaCache;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::L1Value;

const PROTOCOL_VERSION: u8 = 1;
const MAX_TARGET_BYTES: usize = 512;
const MAX_MESSAGE_BYTES: usize = 1_024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InvalidationKind {
    Key,
    Prefix,
}

#[derive(Debug, Serialize, Deserialize)]
struct InvalidationMessage {
    version: u8,
    sender: Uuid,
    kind: InvalidationKind,
    target: String,
}

struct SubscriberTask {
    abort: tokio::task::AbortHandle,
}

impl Drop for SubscriberTask {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

/// Coordinates one process's L1 cache with sibling processes through a
/// namespace-scoped Redis Pub/Sub channel.
#[derive(Clone)]
pub(crate) struct InvalidationCoordinator {
    node_id: Uuid,
    channel: Arc<str>,
    available: Arc<AtomicBool>,
    _subscriber: Arc<SubscriberTask>,
}

impl InvalidationCoordinator {
    pub(crate) async fn start(
        client: redis::Client,
        l1: MokaCache<String, L1Value>,
        namespace: &str,
    ) -> redis::RedisResult<Self> {
        let channel: Arc<str> = format!("rustss:cache-invalidation:v1:{namespace}").into();
        let mut pubsub = client.get_async_pubsub().await?;
        pubsub.subscribe(channel.as_ref()).await?;

        let node_id = Uuid::now_v7();
        let available = Arc::new(AtomicBool::new(true));
        let handle = tokio::spawn(run_subscriber(
            pubsub,
            client,
            l1,
            channel.clone(),
            node_id,
            available.clone(),
        ));

        Ok(Self {
            node_id,
            channel,
            available,
            _subscriber: Arc::new(SubscriberTask {
                abort: handle.abort_handle(),
            }),
        })
    }

    pub(crate) fn is_available(&self) -> bool {
        self.available.load(Ordering::Acquire)
    }

    pub(crate) fn channel(&self) -> &str {
        &self.channel
    }

    pub(crate) fn encode(&self, kind: InvalidationKind, target: &str) -> Option<String> {
        if target.is_empty() || target.len() > MAX_TARGET_BYTES {
            tracing::warn!(
                target_bytes = target.len(),
                "cache: invalidation target is empty or too large"
            );
            return None;
        }
        serde_json::to_string(&InvalidationMessage {
            version: PROTOCOL_VERSION,
            sender: self.node_id,
            kind,
            target: target.to_string(),
        })
        .map_err(|error| {
            tracing::warn!(%error, "cache: could not serialize invalidation message");
        })
        .ok()
    }
}

async fn run_subscriber(
    mut pubsub: redis::aio::PubSub,
    client: redis::Client,
    l1: MokaCache<String, L1Value>,
    channel: Arc<str>,
    node_id: Uuid,
    available: Arc<AtomicBool>,
) {
    loop {
        {
            let mut messages = pubsub.on_message();
            while let Some(message) = messages.next().await {
                apply_message(&l1, node_id, &message).await;
            }
        }

        available.store(false, Ordering::Release);
        clear_l1(&l1).await;
        tracing::warn!("cache: invalidation subscriber disconnected; bypassing L1");

        let mut retry = Duration::from_millis(250);
        loop {
            tokio::time::sleep(retry).await;
            match tokio::time::timeout(Duration::from_secs(5), reconnect(&client, &channel)).await {
                Ok(Ok(next)) => {
                    pubsub = next;
                    clear_l1(&l1).await;
                    available.store(true, Ordering::Release);
                    tracing::info!("cache: invalidation subscriber reconnected");
                    break;
                }
                Ok(Err(error)) => {
                    tracing::debug!(%error, "cache: invalidation reconnect failed");
                    retry = (retry * 2).min(Duration::from_secs(30));
                }
                Err(_) => {
                    tracing::debug!("cache: invalidation reconnect timed out");
                    retry = (retry * 2).min(Duration::from_secs(30));
                }
            }
        }
    }
}

async fn reconnect(
    client: &redis::Client,
    channel: &str,
) -> redis::RedisResult<redis::aio::PubSub> {
    let mut pubsub = client.get_async_pubsub().await?;
    pubsub.subscribe(channel).await?;
    Ok(pubsub)
}

async fn apply_message(l1: &MokaCache<String, L1Value>, node_id: Uuid, message: &redis::Msg) {
    let Ok(payload) = message.get_payload::<String>() else {
        tracing::debug!("cache: ignored non-text invalidation message");
        return;
    };
    if payload.len() > MAX_MESSAGE_BYTES {
        tracing::debug!(
            message_bytes = payload.len(),
            "cache: ignored oversized invalidation"
        );
        return;
    }
    let Ok(message) = serde_json::from_str::<InvalidationMessage>(&payload) else {
        tracing::debug!("cache: ignored malformed invalidation message");
        return;
    };
    if message.version != PROTOCOL_VERSION
        || message.sender == node_id
        || message.target.is_empty()
        || message.target.len() > MAX_TARGET_BYTES
    {
        return;
    }

    match message.kind {
        InvalidationKind::Key => l1.invalidate(&message.target).await,
        InvalidationKind::Prefix => {
            let prefix = message.target;
            if let Err(error) = l1.invalidate_entries_if(move |key, _| key.starts_with(&prefix)) {
                tracing::warn!(%error, "cache: remote prefix invalidation unavailable");
            }
            l1.run_pending_tasks().await;
        }
    }
}

async fn clear_l1(l1: &MokaCache<String, L1Value>) {
    l1.invalidate_all();
    l1.run_pending_tasks().await;
}
