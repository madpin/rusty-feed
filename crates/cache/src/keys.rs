//! Cache key builders and TTLs (docs/ARCHITECTURE.md §10).
//!
//! Keys are split into **shared** (keyed by feed/article — reused across all
//! users) and **per-user** (keyed by user id). Invalidation is prefix-based, so
//! keys are designed so related entries share a prefix.

use std::time::Duration;
use uuid::Uuid;

// ── Shared keys ──────────────────────────────────────────────────────────────

/// Metadata for a shared feed.
pub fn feed(feed_id: Uuid) -> String {
    format!("feed:{feed_id}")
}

/// A page of a feed's article list.
pub fn feed_articles(feed_id: Uuid, page: u32) -> String {
    format!("feed:articles:{feed_id}:{page}")
}

/// Prefix covering all cached article pages for a feed.
pub fn feed_articles_prefix(feed_id: Uuid) -> String {
    format!("feed:articles:{feed_id}:")
}

/// A shared article's embedding (single space per instance).
pub fn article_embedding(article_id: Uuid) -> String {
    format!("embedding:{article_id}")
}

/// A shared article's generated summary.
pub fn summary(article_id: Uuid) -> String {
    format!("summary:{article_id}")
}

/// Related-articles result for a source article.
pub fn related(article_id: Uuid) -> String {
    format!("related:{article_id}")
}

// ── Per-user keys ────────────────────────────────────────────────────────────

/// A user's relevance score for a specific article.
pub fn score(user_id: Uuid, article_id: Uuid) -> String {
    format!("score:{user_id}:{article_id}")
}

/// Prefix covering all of a user's cached article scores (invalidated on
/// feedback / pattern change).
pub fn user_scores_prefix(user_id: Uuid) -> String {
    format!("score:{user_id}:")
}

/// A user's learned keyword patterns.
pub fn user_patterns(user_id: Uuid) -> String {
    format!("patterns:{user_id}")
}

/// A user's preferences.
pub fn user_preferences(user_id: Uuid) -> String {
    format!("prefs:{user_id}")
}

/// A user's feed subscriptions.
pub fn user_subscriptions(user_id: Uuid) -> String {
    format!("subs:{user_id}")
}

/// A user's categories.
pub fn user_categories(user_id: Uuid) -> String {
    format!("categories:{user_id}")
}

/// Default TTLs per namespace (docs/ARCHITECTURE.md §10).
pub mod ttl {
    use std::time::Duration;

    pub const FEED_METADATA: Duration = Duration::from_secs(3_600);
    pub const FEED_ARTICLES: Duration = Duration::from_secs(300);
    pub const ARTICLE_EMBEDDING: Duration = Duration::from_secs(86_400);
    pub const SUMMARY: Duration = Duration::from_secs(7 * 24 * 3_600);
    pub const RELATED: Duration = Duration::from_secs(86_400);
    pub const SEARCH: Duration = Duration::from_secs(1_800);
    pub const SCORE: Duration = Duration::from_secs(3_600);
    pub const USER_PATTERNS: Duration = Duration::from_secs(900);
    pub const USER_PREFERENCES: Duration = Duration::from_secs(3_600);
    pub const USER_SUBSCRIPTIONS: Duration = Duration::from_secs(900);
    pub const USER_CATEGORIES: Duration = Duration::from_secs(1_800);
}

/// The coarse safety cap used by the L1 cache (longest namespace TTL).
pub(crate) const L1_MAX_TTL: Duration = ttl::SUMMARY;
