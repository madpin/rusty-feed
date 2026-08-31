//! # rustss-cache
//!
//! A two-tier cache behind one small API (docs/ARCHITECTURE.md §10):
//!
//! - **L1** — in-process [`moka`], always on, microsecond hits.
//! - **L2** — Redis (optional), shared across instances, survives restarts.
//!
//! The cache is **best-effort**: if Redis is unavailable, L1 + the source of
//! truth still serve requests — cache errors are logged, never propagated.
//! Keys separate *shared* from *per-user* entries so shared work is reused
//! across users (the multi-user dedup principle applies to caching too).
//!
//! Redis-backed instances coordinate L1 invalidation through a versioned,
//! namespace-scoped Pub/Sub channel. If that subscriber disconnects, L1 is
//! cleared and bypassed until a clean resubscription, while L2/source reads
//! continue. Invalidation is prefix-based (`delete_prefix`); the key scheme in
//!   [`keys`] is designed so related entries share a prefix.

mod invalidation;
pub mod keys;

use std::sync::Arc;
use std::time::{Duration, Instant};

use moka::future::Cache as MokaCache;
use serde::{de::DeserializeOwned, Serialize};

use crate::invalidation::{InvalidationCoordinator, InvalidationKind};
use crate::keys::L1_MAX_TTL;

const DEFAULT_NAMESPACE: &str = "rustss";

/// An L1 entry: serialized bytes plus the per-key expiry deadline.
#[derive(Clone)]
pub(crate) struct L1Value {
    data: Arc<[u8]>,
    deadline: Instant,
}

/// Configuration for [`Cache::new`].
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Master switch; when false, no Redis is used (L1 only).
    pub enabled: bool,
    /// Redis connection URL (e.g. `redis://localhost:6379`).
    pub redis_url: Option<String>,
    /// Optional Redis password (used if the URL doesn't already carry one).
    pub redis_password: Option<String>,
    /// Max number of entries kept in the in-process L1 cache.
    pub l1_max_capacity: u64,
    /// Isolates L2 keys and invalidation messages when Redis is shared by
    /// multiple RustSS installations.
    pub namespace: String,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            redis_url: None,
            redis_password: None,
            l1_max_capacity: 0,
            namespace: DEFAULT_NAMESPACE.to_string(),
        }
    }
}

/// A cloneable, two-tier cache handle.
#[derive(Clone)]
pub struct Cache {
    l1: MokaCache<String, L1Value>,
    l2: Option<redis::aio::ConnectionManager>,
    key_prefix: Arc<str>,
    invalidation: Option<InvalidationCoordinator>,
}

impl Cache {
    /// Build a cache, connecting to Redis if configured. Never fails: a Redis
    /// connection error degrades to L1-only (logged as a warning).
    pub async fn new(config: CacheConfig) -> Self {
        let capacity = if config.l1_max_capacity == 0 {
            10_000
        } else {
            config.l1_max_capacity
        };
        let l1 = MokaCache::builder()
            .max_capacity(capacity)
            .time_to_live(L1_MAX_TTL) // coarse safety cap; real TTL is per-entry
            .support_invalidation_closures() // enables delete_prefix's predicate sweep
            .build();

        let namespace = normalize_namespace(&config.namespace);
        let key_prefix: Arc<str> = format!("{namespace}:").into();
        let (l2, invalidation) = if config.enabled {
            match connect_redis(
                config.redis_url.as_deref(),
                config.redis_password.as_deref(),
            )
            .await
            {
                Some((manager, client)) => {
                    match tokio::time::timeout(
                        Duration::from_secs(5),
                        InvalidationCoordinator::start(client, l1.clone(), &namespace),
                    )
                    .await
                    {
                        Ok(Ok(coordinator)) => (Some(manager), Some(coordinator)),
                        Ok(Err(error)) => {
                            tracing::warn!(
                                %error,
                                "cache: invalidation subscriber unavailable; running L1-only"
                            );
                            (None, None)
                        }
                        Err(_) => {
                            tracing::warn!(
                                "cache: invalidation subscriber timed out; running L1-only"
                            );
                            (None, None)
                        }
                    }
                }
                None => (None, None),
            }
        } else {
            (None, None)
        };

        Self {
            l1,
            l2,
            key_prefix,
            invalidation,
        }
    }

    /// An L1-only cache (no Redis). Handy for tests and Redis-less deployments.
    pub async fn in_memory() -> Self {
        Self::new(CacheConfig {
            enabled: false,
            l1_max_capacity: 10_000,
            namespace: DEFAULT_NAMESPACE.to_string(),
            ..Default::default()
        })
        .await
    }

    /// Whether an L2 (Redis) connection is active.
    pub fn l2_connected(&self) -> bool {
        self.l2.is_some()
    }

    /// Get and deserialize a value, checking L1 then L2. Returns `None` on miss
    /// or any (swallowed) error.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        // L1
        if self.l1_available() {
            if let Some(v) = self.l1.get(key).await {
                if Instant::now() < v.deadline {
                    if let Ok(val) = serde_json::from_slice::<T>(&v.data) {
                        return Some(val);
                    }
                } else {
                    self.l1.invalidate(key).await;
                }
            }
        }

        // L2
        if let Some(mgr) = &self.l2 {
            let mut conn = mgr.clone();
            let redis_key = self.redis_key(key);
            let bytes: Option<Vec<u8>> = redis::cmd("GET")
                .arg(&redis_key)
                .query_async(&mut conn)
                .await
                .ok()
                .flatten();
            if let Some(bytes) = bytes {
                // Backfill L1 using the remaining Redis TTL when available.
                let ttl_ms: i64 = redis::cmd("PTTL")
                    .arg(&redis_key)
                    .query_async(&mut conn)
                    .await
                    .unwrap_or(-1);
                let remaining = if ttl_ms > 0 {
                    Duration::from_millis(ttl_ms as u64)
                } else {
                    Duration::from_secs(60)
                };
                self.insert_l1(key, &bytes, remaining).await;
                if let Ok(val) = serde_json::from_slice::<T>(&bytes) {
                    return Some(val);
                }
            }
        }

        None
    }

    /// Serialize and store a value in both tiers with the given TTL.
    pub async fn set<T: Serialize>(&self, key: &str, value: &T, ttl: Duration) {
        let bytes = match serde_json::to_vec(value) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, key, "cache: serialize failed");
                return;
            }
        };

        self.insert_l1(key, &bytes, ttl).await;

        if let Some(mgr) = &self.l2 {
            let mut conn = mgr.clone();
            let secs = ttl.as_secs().max(1);
            let res: redis::RedisResult<()> = redis::cmd("SET")
                .arg(self.redis_key(key))
                .arg(bytes.as_slice())
                .arg("EX")
                .arg(secs)
                .query_async(&mut conn)
                .await;
            if let Err(e) = res {
                tracing::debug!(error = %e, key, "cache: L2 set failed (degraded)");
            } else {
                self.publish_invalidation(&mut conn, InvalidationKind::Key, key)
                    .await;
            }
        }
    }

    /// Cache-aside: return the cached value, or run `fetch`, cache it, return it.
    /// Fetch errors are propagated; only successful values are cached.
    pub async fn get_or_set<T, E, F, Fut>(&self, key: &str, ttl: Duration, fetch: F) -> Result<T, E>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
    {
        if let Some(v) = self.get::<T>(key).await {
            return Ok(v);
        }
        let value = fetch().await?;
        self.set(key, &value, ttl).await;
        Ok(value)
    }

    /// Delete a single key from both tiers.
    pub async fn delete(&self, key: &str) {
        self.l1.invalidate(key).await;
        if let Some(mgr) = &self.l2 {
            let mut conn = mgr.clone();
            let deleted: redis::RedisResult<()> = redis::cmd("DEL")
                .arg(self.redis_key(key))
                .query_async(&mut conn)
                .await;
            match deleted {
                Ok(()) => {
                    self.publish_invalidation(&mut conn, InvalidationKind::Key, key)
                        .await;
                }
                Err(error) => {
                    tracing::debug!(%error, key, "cache: L2 delete failed (degraded)");
                }
            }
        }
    }

    /// Delete every key beginning with `prefix`. L2 uses a non-blocking `SCAN`
    /// (never `KEYS`); L1 uses a predicate sweep.
    pub async fn delete_prefix(&self, prefix: &str) {
        let owned = prefix.to_string();
        if let Err(e) = self
            .l1
            .invalidate_entries_if(move |k, _v| k.starts_with(&owned))
        {
            tracing::warn!(error = %e, "cache: L1 prefix invalidation unavailable");
        }
        // Force the predicate to be applied now (deterministic for callers/tests).
        self.l1.run_pending_tasks().await;

        if let Some(mgr) = &self.l2 {
            let mut conn = mgr.clone();
            let pattern = format!("{}{prefix}*", self.key_prefix);
            let mut cursor: u64 = 0;
            let mut completed = true;
            loop {
                let scan: redis::RedisResult<(u64, Vec<String>)> = redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .arg("COUNT")
                    .arg(200)
                    .query_async(&mut conn)
                    .await;
                let (next, found) = match scan {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::debug!(error = %e, "cache: L2 SCAN failed (degraded)");
                        completed = false;
                        break;
                    }
                };
                if !found.is_empty() {
                    let deleted: redis::RedisResult<()> =
                        redis::cmd("DEL").arg(&found).query_async(&mut conn).await;
                    if let Err(error) = deleted {
                        tracing::debug!(
                            %error,
                            "cache: L2 prefix delete failed (degraded)"
                        );
                        completed = false;
                        break;
                    }
                }
                if next == 0 {
                    break;
                }
                cursor = next;
            }
            if completed {
                self.publish_invalidation(&mut conn, InvalidationKind::Prefix, prefix)
                    .await;
            }
        }
    }

    async fn insert_l1(&self, key: &str, bytes: &[u8], ttl: Duration) {
        if !self.l1_available() {
            return;
        }
        let deadline = Instant::now().checked_add(ttl).unwrap_or_else(Instant::now);
        self.l1
            .insert(
                key.to_string(),
                L1Value {
                    data: Arc::from(bytes.to_vec().into_boxed_slice()),
                    deadline,
                },
            )
            .await;
    }

    fn l1_available(&self) -> bool {
        self.invalidation
            .as_ref()
            .is_none_or(InvalidationCoordinator::is_available)
    }

    fn redis_key(&self, key: &str) -> String {
        format!("{}{key}", self.key_prefix)
    }

    async fn publish_invalidation(
        &self,
        connection: &mut redis::aio::ConnectionManager,
        kind: InvalidationKind,
        target: &str,
    ) {
        let Some(coordinator) = &self.invalidation else {
            return;
        };
        let Some(payload) = coordinator.encode(kind, target) else {
            return;
        };
        let result: redis::RedisResult<usize> = redis::cmd("PUBLISH")
            .arg(coordinator.channel())
            .arg(payload)
            .query_async(connection)
            .await;
        if let Err(error) = result {
            tracing::debug!(%error, "cache: invalidation publish failed");
        }
    }
}

/// Connect a Redis `ConnectionManager`, injecting a password if provided and not
/// already present in the URL. Returns `None` (degraded) on any failure.
async fn connect_redis(
    url: Option<&str>,
    password: Option<&str>,
) -> Option<(redis::aio::ConnectionManager, redis::Client)> {
    let url = url?;
    let mut info = match redis::IntoConnectionInfo::into_connection_info(url) {
        Ok(info) => info,
        Err(e) => {
            tracing::warn!(error = %e, "cache: invalid REDIS_URL; running L1-only");
            return None;
        }
    };
    if info.redis.password.is_none() {
        if let Some(pw) = password {
            info.redis.password = Some(pw.to_string());
        }
    }

    let client = match redis::Client::open(info) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "cache: failed to open Redis client; running L1-only");
            return None;
        }
    };
    // Bound the initial connect so a down/unreachable Redis degrades quickly
    // instead of blocking startup on reconnection backoff.
    let connect = redis::aio::ConnectionManager::new(client.clone());
    match tokio::time::timeout(Duration::from_secs(5), connect).await {
        Ok(Ok(mgr)) => {
            tracing::info!("cache: connected to Redis (L2 enabled)");
            Some((mgr, client))
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "cache: Redis unavailable; running L1-only");
            None
        }
        Err(_) => {
            tracing::warn!("cache: Redis connect timed out; running L1-only");
            None
        }
    }
}

fn normalize_namespace(namespace: &str) -> String {
    let namespace = namespace.trim();
    if (1..=64).contains(&namespace.len())
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        namespace.to_string()
    } else {
        tracing::warn!(
            namespace_bytes = namespace.len(),
            "cache: invalid namespace; using the safe default"
        );
        DEFAULT_NAMESPACE.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn default_config_uses_safe_installation_namespace() {
        assert_eq!(CacheConfig::default().namespace, DEFAULT_NAMESPACE);
        assert_eq!(normalize_namespace("bad:namespace"), DEFAULT_NAMESPACE);
    }

    #[tokio::test]
    async fn set_get_roundtrip_l1() {
        let cache = Cache::in_memory().await;
        assert!(!cache.l2_connected());
        cache
            .set("k1", &"value".to_string(), Duration::from_secs(60))
            .await;
        let got: Option<String> = cache.get("k1").await;
        assert_eq!(got.as_deref(), Some("value"));
    }

    #[tokio::test]
    async fn missing_key_is_none() {
        let cache = Cache::in_memory().await;
        let got: Option<String> = cache.get("nope").await;
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn entry_expires_after_ttl() {
        let cache = Cache::in_memory().await;
        cache.set("temp", &42u32, Duration::from_millis(40)).await;
        assert_eq!(cache.get::<u32>("temp").await, Some(42));
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(cache.get::<u32>("temp").await, None);
    }

    #[tokio::test]
    async fn delete_removes_key() {
        let cache = Cache::in_memory().await;
        cache.set("d", &1u8, Duration::from_secs(60)).await;
        cache.delete("d").await;
        assert_eq!(cache.get::<u8>("d").await, None);
    }

    #[tokio::test]
    async fn delete_prefix_only_removes_matching() {
        let cache = Cache::in_memory().await;
        cache
            .set("score:u1:a1", &1u8, Duration::from_secs(60))
            .await;
        cache
            .set("score:u1:a2", &2u8, Duration::from_secs(60))
            .await;
        cache
            .set("score:u2:a1", &3u8, Duration::from_secs(60))
            .await;
        cache.set("prefs:u1", &9u8, Duration::from_secs(60)).await;

        cache.delete_prefix("score:u1:").await;

        assert_eq!(cache.get::<u8>("score:u1:a1").await, None);
        assert_eq!(cache.get::<u8>("score:u1:a2").await, None);
        assert_eq!(cache.get::<u8>("score:u2:a1").await, Some(3));
        assert_eq!(cache.get::<u8>("prefs:u1").await, Some(9));
    }

    #[tokio::test]
    async fn get_or_set_fetches_once() {
        let cache = Cache::in_memory().await;
        let calls = AtomicUsize::new(0);

        let fetch = || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ()>("computed".to_string())
        };

        let v1 = cache
            .get_or_set("k", Duration::from_secs(60), fetch)
            .await
            .unwrap();
        let v2: String = cache
            .get_or_set("k", Duration::from_secs(60), || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ()>("computed".to_string())
            })
            .await
            .unwrap();

        assert_eq!(v1, "computed");
        assert_eq!(v2, "computed");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "fetch should run only once"
        );
    }

    #[tokio::test]
    async fn unavailable_redis_degrades_to_l1() {
        let cache = Cache::new(CacheConfig {
            enabled: true,
            redis_url: Some("not-a-redis-url".to_string()),
            redis_password: None,
            l1_max_capacity: 100,
            namespace: "degraded-test".to_string(),
        })
        .await;

        assert!(!cache.l2_connected());
        cache.set("available", &true, Duration::from_secs(30)).await;
        assert_eq!(cache.get::<bool>("available").await, Some(true));
    }

    // Redis-backed round trip. Runs only when TEST_REDIS_URL is set.
    #[tokio::test]
    async fn redis_l2_roundtrip() {
        let Ok(url) = std::env::var("TEST_REDIS_URL") else {
            eprintln!("skipping: TEST_REDIS_URL not set");
            return;
        };
        let cache = Cache::new(CacheConfig {
            enabled: true,
            redis_url: Some(url),
            redis_password: None,
            l1_max_capacity: 1_000,
            namespace: format!("test-{}", uuid::Uuid::now_v7()),
        })
        .await;
        assert!(cache.l2_connected(), "should connect to Redis");

        let key = format!("test:{}", uuid::Uuid::now_v7());
        cache
            .set(&key, &"hello".to_string(), Duration::from_secs(30))
            .await;

        // A fresh instance (empty L1) must read the value from Redis (L2).
        let fresh = || async {
            Cache::new(CacheConfig {
                enabled: true,
                redis_url: std::env::var("TEST_REDIS_URL").ok(),
                redis_password: None,
                l1_max_capacity: 1_000,
                namespace: cache.key_prefix.trim_end_matches(':').to_string(),
            })
            .await
        };
        assert_eq!(
            fresh().await.get::<String>(&key).await.as_deref(),
            Some("hello")
        );

        // After delete, another fresh instance must miss (proves L2 removal).
        cache.delete(&key).await;
        assert_eq!(fresh().await.get::<String>(&key).await, None);
    }

    #[tokio::test]
    async fn redis_invalidation_updates_other_instances_l1() {
        let Ok(url) = std::env::var("TEST_REDIS_URL") else {
            eprintln!("skipping: TEST_REDIS_URL not set");
            return;
        };
        let namespace = format!("test-{}", uuid::Uuid::now_v7());
        let config = || CacheConfig {
            enabled: true,
            redis_url: Some(url.clone()),
            redis_password: None,
            l1_max_capacity: 1_000,
            namespace: namespace.clone(),
        };
        let writer = Cache::new(config()).await;
        let reader = Cache::new(config()).await;
        assert!(writer.l2_connected() && reader.l2_connected());

        let key = format!("shared:{}", uuid::Uuid::now_v7());
        writer.set(&key, &1u8, Duration::from_secs(30)).await;
        assert_eq!(reader.get::<u8>(&key).await, Some(1));

        writer.set(&key, &2u8, Duration::from_secs(30)).await;
        wait_for_value(&reader, &key, Some(2)).await;

        writer.delete(&key).await;
        wait_for_value(&reader, &key, None).await;
    }

    #[tokio::test]
    async fn redis_prefix_invalidation_is_remote_and_namespace_scoped() {
        let Ok(url) = std::env::var("TEST_REDIS_URL") else {
            eprintln!("skipping: TEST_REDIS_URL not set");
            return;
        };
        let namespace = format!("test-{}", uuid::Uuid::now_v7());
        let config = |namespace: String| CacheConfig {
            enabled: true,
            redis_url: Some(url.clone()),
            redis_password: None,
            l1_max_capacity: 1_000,
            namespace,
        };
        let writer = Cache::new(config(namespace.clone())).await;
        let reader = Cache::new(config(namespace.clone())).await;
        let isolated = Cache::new(config(format!("{namespace}-isolated"))).await;

        writer
            .set("score:u1:a1", &1u8, Duration::from_secs(30))
            .await;
        writer
            .set("score:u2:a1", &2u8, Duration::from_secs(30))
            .await;
        isolated
            .set("score:u1:a1", &9u8, Duration::from_secs(30))
            .await;
        assert_eq!(reader.get::<u8>("score:u1:a1").await, Some(1));
        assert_eq!(reader.get::<u8>("score:u2:a1").await, Some(2));

        writer.delete_prefix("score:u1:").await;
        wait_for_value(&reader, "score:u1:a1", None).await;
        assert_eq!(reader.get::<u8>("score:u2:a1").await, Some(2));
        assert_eq!(isolated.get::<u8>("score:u1:a1").await, Some(9));
    }

    async fn wait_for_value(cache: &Cache, key: &str, expected: Option<u8>) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if cache.get::<u8>(key).await == expected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("remote cache invalidation should arrive promptly");
    }
}
