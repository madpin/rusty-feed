//! Per-user resource quotas for public instances (FND-188).
//!
//! Counts of **per-user** rows only — quotas never touch shared content, so the
//! "content is shared; state is personal" invariant is preserved. A limit of `0`
//! means **unlimited** (the historical single-tenant default); public operators
//! set finite caps (`docs/PUBLIC_LAUNCH_PLAN.md` §4.2).

/// The per-user resources that can be capped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaKind {
    Subscriptions,
    SavedSearches,
    Categories,
    ApiTokens,
    Integrations,
    PublicShares,
}

impl QuotaKind {
    /// A short, user-facing noun used in the "limit reached" message.
    pub fn noun(self) -> &'static str {
        match self {
            QuotaKind::Subscriptions => "feed subscriptions",
            QuotaKind::SavedSearches => "saved searches",
            QuotaKind::Categories => "categories",
            QuotaKind::ApiTokens => "API tokens",
            QuotaKind::Integrations => "integrations",
            QuotaKind::PublicShares => "public shares",
        }
    }
}

/// Immutable per-user caps. `0` == unlimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quotas {
    pub max_subscriptions: u32,
    pub max_saved_searches: u32,
    pub max_categories: u32,
    pub max_api_tokens: u32,
    pub max_integrations: u32,
    pub max_public_shares: u32,
}

impl Default for Quotas {
    /// Unlimited — historical single-tenant behavior.
    fn default() -> Self {
        Self {
            max_subscriptions: 0,
            max_saved_searches: 0,
            max_categories: 0,
            max_api_tokens: 0,
            max_integrations: 0,
            max_public_shares: 0,
        }
    }
}

impl Quotas {
    /// The configured cap for `kind` (`0` == unlimited).
    pub fn limit(&self, kind: QuotaKind) -> u32 {
        match kind {
            QuotaKind::Subscriptions => self.max_subscriptions,
            QuotaKind::SavedSearches => self.max_saved_searches,
            QuotaKind::Categories => self.max_categories,
            QuotaKind::ApiTokens => self.max_api_tokens,
            QuotaKind::Integrations => self.max_integrations,
            QuotaKind::PublicShares => self.max_public_shares,
        }
    }

    /// Whether creating one more of `kind` is allowed given the current `count`.
    /// Unlimited (`0`) always allows.
    pub fn allows(&self, kind: QuotaKind, current_count: i64) -> bool {
        let limit = self.limit(kind);
        limit == 0 || current_count < i64::from(limit)
    }

    /// A client-safe message for a breached quota.
    pub fn exceeded_message(&self, kind: QuotaKind) -> String {
        format!(
            "You've reached this account's limit of {} {}. Remove some first, or ask an administrator to raise the limit.",
            self.limit(kind),
            kind.noun()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_by_default() {
        let q = Quotas::default();
        assert!(q.allows(QuotaKind::Subscriptions, 1_000_000));
        assert_eq!(q.limit(QuotaKind::Categories), 0);
    }

    #[test]
    fn finite_cap_blocks_at_limit() {
        let q = Quotas {
            max_subscriptions: 3,
            ..Quotas::default()
        };
        assert!(q.allows(QuotaKind::Subscriptions, 2));
        assert!(!q.allows(QuotaKind::Subscriptions, 3));
        assert!(!q.allows(QuotaKind::Subscriptions, 4));
        // Other kinds remain unlimited.
        assert!(q.allows(QuotaKind::Categories, 10_000));
        assert!(q.exceeded_message(QuotaKind::Subscriptions).contains("3"));
    }
}
