//! AI cost & compute budget policy for public instances (FND-187).
//!
//! Local embeddings are free of API cost but not of CPU, and LLM summaries cost
//! real money. On an open-registration instance these must be bounded *before*
//! every spend. This module holds the immutable, validated budget values; the
//! enforcement (consulting the durable `cost_ledger`) happens at the call sites
//! that own a database pool (`docs/PUBLIC_LAUNCH_PLAN.md` §4.1).
//!
//! A limit of `0` means **unlimited** — the default, which preserves the
//! historical single-tenant behavior. Public operators set finite values.

use time::{OffsetDateTime, Time};

/// Immutable daily AI budget, shared by the web, agents, and llm layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiBudget {
    /// Instance-wide kill-switch for instance-funded LLM spend (micro-USD/day).
    pub llm_global_daily_micro_usd: i64,
    /// Per-user instance-funded LLM spend cap (micro-USD/day).
    pub llm_per_user_daily_micro_usd: i64,
    /// Per-user LLM call cap/day (funding-agnostic; bounds job/DB abuse).
    pub llm_per_user_daily_calls: u32,
    /// Per-user interactive embedding (search) call cap/day.
    pub embed_per_user_daily_calls: u32,
}

impl Default for AiBudget {
    /// Unlimited — historical single-tenant behavior.
    fn default() -> Self {
        Self {
            llm_global_daily_micro_usd: 0,
            llm_per_user_daily_micro_usd: 0,
            llm_per_user_daily_calls: 0,
            embed_per_user_daily_calls: 0,
        }
    }
}

impl AiBudget {
    /// Build from operator-facing USD/call values (`0`/`0.0` == unlimited).
    pub fn from_config(
        global_daily_usd: f64,
        per_user_daily_usd: f64,
        per_user_daily_calls: u32,
        embed_per_user_daily_calls: u32,
    ) -> Self {
        Self {
            llm_global_daily_micro_usd: usd_to_micro(global_daily_usd),
            llm_per_user_daily_micro_usd: usd_to_micro(per_user_daily_usd),
            llm_per_user_daily_calls: per_user_daily_calls,
            embed_per_user_daily_calls,
        }
    }

    /// The start of the current UTC day — the window boundary for daily budgets.
    pub fn today_start() -> OffsetDateTime {
        OffsetDateTime::now_utc().replace_time(Time::MIDNIGHT)
    }

    /// Whether instance-funded global LLM spend is capped at all.
    pub fn has_global_llm_cap(&self) -> bool {
        self.llm_global_daily_micro_usd > 0
    }
    pub fn has_per_user_llm_usd_cap(&self) -> bool {
        self.llm_per_user_daily_micro_usd > 0
    }
    pub fn has_per_user_llm_call_cap(&self) -> bool {
        self.llm_per_user_daily_calls > 0
    }
    pub fn has_embed_call_cap(&self) -> bool {
        self.embed_per_user_daily_calls > 0
    }
}

/// Convert a non-negative USD amount to micro-USD, saturating; `<= 0` → `0`.
pub fn usd_to_micro(usd: f64) -> i64 {
    if !usd.is_finite() || usd <= 0.0 {
        return 0;
    }
    (usd * 1_000_000.0).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usd_conversion_and_unlimited_semantics() {
        assert_eq!(usd_to_micro(5.0), 5_000_000);
        assert_eq!(usd_to_micro(0.10), 100_000);
        assert_eq!(usd_to_micro(0.0), 0);
        assert_eq!(usd_to_micro(-1.0), 0);
        let b = AiBudget::default();
        assert!(!b.has_global_llm_cap());
        assert!(!b.has_per_user_llm_call_cap());
    }

    #[test]
    fn from_config_maps_fields() {
        let b = AiBudget::from_config(5.0, 0.10, 25, 500);
        assert_eq!(b.llm_global_daily_micro_usd, 5_000_000);
        assert_eq!(b.llm_per_user_daily_micro_usd, 100_000);
        assert_eq!(b.llm_per_user_daily_calls, 25);
        assert_eq!(b.embed_per_user_daily_calls, 500);
        assert!(b.has_global_llm_cap());
        assert!(b.has_embed_call_cap());
    }

    #[test]
    fn today_start_is_midnight_utc() {
        let start = AiBudget::today_start();
        assert_eq!(start.time(), Time::MIDNIGHT);
        assert_eq!(start.offset(), time::UtcOffset::UTC);
    }
}
