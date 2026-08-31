//! A small in-process, fixed-window rate limiter keyed by client IP.
//!
//! Used to throttle authentication endpoints (login/register/OAuth start). It is
//! intentionally simple and best-effort — good enough to blunt brute force and
//! abuse on a self-hosted instance. Behind a reverse proxy, ensure the proxy
//! sets `X-Forwarded-For` (see [`crate::extract::ClientIp`]).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;

/// Fixed-window per-key rate limiter.
#[derive(Clone)]
pub struct RateLimiter {
    buckets: Cache<String, Arc<AtomicU32>>,
    max: u32,
}

impl RateLimiter {
    /// Allow at most `max` requests per `window` per key. A `max` of `0` means
    /// **unlimited** — [`check`](Self::check) always allows.
    pub fn new(max: u32, window: Duration) -> Self {
        Self {
            buckets: Cache::builder()
                .time_to_live(window)
                .max_capacity(100_000)
                .build(),
            max,
        }
    }

    /// Convenience constructor for a per-minute limit (`0` == unlimited).
    pub fn per_minute(max: u32) -> Self {
        Self::new(max, Duration::from_secs(60))
    }

    /// Record an attempt for `key`; returns `true` if it is within the limit.
    /// An unlimited limiter (`max == 0`) never records and always allows.
    pub async fn check(&self, key: &str) -> bool {
        if self.max == 0 {
            return true;
        }
        let counter = self
            .buckets
            .get_with(key.to_string(), async { Arc::new(AtomicU32::new(0)) })
            .await;
        let count = counter.fetch_add(1, Ordering::Relaxed) + 1;
        count <= self.max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn allows_up_to_limit_then_blocks() {
        let rl = RateLimiter::new(3, Duration::from_secs(60));
        assert!(rl.check("1.2.3.4").await); // 1
        assert!(rl.check("1.2.3.4").await); // 2
        assert!(rl.check("1.2.3.4").await); // 3
        assert!(!rl.check("1.2.3.4").await); // 4 -> blocked
                                             // A different key is independent.
        assert!(rl.check("5.6.7.8").await);
    }

    #[tokio::test]
    async fn zero_max_is_unlimited() {
        let rl = RateLimiter::per_minute(0);
        for _ in 0..1000 {
            assert!(rl.check("1.2.3.4").await);
        }
    }
}
