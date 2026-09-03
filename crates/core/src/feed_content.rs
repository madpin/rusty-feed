use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::content_rules::{rewrite_rules_from_json, RewriteRule};

/// Default retention period for a shared public-page copy.
pub const DEFAULT_CONTENT_LIFETIME_DAYS: u16 = 30;
/// Longest finite public-page lifetime accepted from a subscription.
pub const MAX_CONTENT_LIFETIME_DAYS: u16 = 3650;
/// Built-in catalog sources whose public pages provide the recommended reading body.
pub const CATALOG_ARTICLE_PAGE_RECOMMENDATIONS: &[&str] = &[
    "https://openai.com/news/rss.xml",
    "https://blog.python.org/feeds/posts/default?alt=rss",
    "https://www.propublica.org/feeds/propublica/main",
    "https://api.quantamagazine.org/feed/",
    "https://news.ycombinator.com/rss",
    "https://feeds.arstechnica.com/arstechnica/index",
    "https://www.aljazeera.com/xml/rss/all.xml",
    "http://feeds.bbci.co.uk/news/world/rss.xml",
    "https://rss.dw.com/rdf/rss-en-top",
    "https://news.un.org/feed/subscribe/en/news/all/rss.xml",
    "https://www.theguardian.com/world/rss",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FeedContentMode {
    #[default]
    Rss,
    ArticlePage,
}

impl FeedContentMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "rss" => Some(Self::Rss),
            "article_page" => Some(Self::ArticlePage),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rss => "rss",
            Self::ArticlePage => "article_page",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Rss => "RSS content",
            Self::ArticlePage => "Public article pages",
        }
    }

    pub fn fetches_article_pages(self) -> bool {
        matches!(self, Self::ArticlePage)
    }
}

/// Return the built-in content recommendation for a catalog feed URL.
pub fn recommended_catalog_content_mode(feed_url: &str) -> FeedContentMode {
    if CATALOG_ARTICLE_PAGE_RECOMMENDATIONS.contains(&feed_url) {
        FeedContentMode::ArticlePage
    } else {
        FeedContentMode::Rss
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedContentSettings {
    pub mode: FeedContentMode,
    pub selector: Option<String>,
    pub rewrite_rules: Vec<RewriteRule>,
}

impl Default for FeedContentSettings {
    fn default() -> Self {
        Self {
            mode: FeedContentMode::Rss,
            selector: None,
            rewrite_rules: Vec::new(),
        }
    }
}

impl FeedContentSettings {
    pub fn from_json(value: Option<&Value>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        let mode = match value.get("mode").and_then(Value::as_str) {
            Some("paywalled") => FeedContentMode::Rss,
            Some(mode) => FeedContentMode::parse(mode).unwrap_or_default(),
            None => value
                .get("enabled")
                .and_then(Value::as_bool)
                .filter(|enabled| *enabled)
                .map(|_| FeedContentMode::ArticlePage)
                .unwrap_or_default(),
        };
        let selector = value
            .get("selector")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|selector| !selector.is_empty())
            .map(str::to_string);
        let rewrite_rules = rewrite_rules_from_json(Some(value));
        Self {
            mode,
            selector,
            rewrite_rules,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "mode": self.mode.as_str(),
            "selector": self.selector,
            "rewrite_rules": self.rewrite_rules,
        })
    }
}

/// How long a subscriber asks Rusty Feed to retain a fetched public article page.
/// RSS content is always stored with the shared article and is not governed by
/// this lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentLifetime {
    Days(u16),
    Infinite,
}

impl Default for ContentLifetime {
    fn default() -> Self {
        Self::Days(DEFAULT_CONTENT_LIFETIME_DAYS)
    }
}

impl ContentLifetime {
    /// Parse a settings-form value (`infinite` or a bounded day count).
    pub fn parse(value: &str) -> Option<Self> {
        if value == "infinite" {
            return Some(Self::Infinite);
        }
        value
            .parse::<u16>()
            .ok()
            .filter(|days| (1..=MAX_CONTENT_LIFETIME_DAYS).contains(days))
            .map(Self::Days)
    }

    /// Return the finite day count, or `None` for an infinite lifetime.
    pub fn days(self) -> Option<u16> {
        match self {
            Self::Days(days) => Some(days),
            Self::Infinite => None,
        }
    }

    /// Serialize the lifetime to the value used by the HTML settings form.
    pub fn form_value(self) -> String {
        match self {
            Self::Days(days) => days.to_string(),
            Self::Infinite => "infinite".to_string(),
        }
    }

    /// Merge subscriber requests for one shared copy. Infinite dominates;
    /// otherwise the longest requested lifetime wins.
    pub fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Infinite, _) | (_, Self::Infinite) => Self::Infinite,
            (Self::Days(left), Self::Days(right)) => Self::Days(left.max(right)),
        }
    }
}

/// The content choice owned by one user's subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscriptionContentSettings {
    /// Which stored body this subscriber wants to read.
    pub mode: FeedContentMode,
    /// Requested lifetime for the shared public-page copy.
    pub lifetime: ContentLifetime,
}

/// Per-subscription rendering override for enclosure delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubscriptionEnclosureDelivery {
    #[default]
    Inherit,
    Proxy,
    MixedContent,
    Direct,
}

impl SubscriptionEnclosureDelivery {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "inherit" => Some(Self::Inherit),
            "proxy" => Some(Self::Proxy),
            "mixed-content" => Some(Self::MixedContent),
            "direct" => Some(Self::Direct),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Proxy => "proxy",
            Self::MixedContent => "mixed-content",
            Self::Direct => "direct",
        }
    }
}

/// Per-subscription rendering override for attachment player loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubscriptionEnclosureLoad {
    #[default]
    Inherit,
    OnDemand,
    Metadata,
    LinksOnly,
}

impl SubscriptionEnclosureLoad {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "inherit" => Some(Self::Inherit),
            "on-demand" => Some(Self::OnDemand),
            "metadata" => Some(Self::Metadata),
            "links-only" => Some(Self::LinksOnly),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::OnDemand => "on-demand",
            Self::Metadata => "metadata",
            Self::LinksOnly => "links-only",
        }
    }
}

/// Personal attachment overrides stored on one subscription. These affect
/// rendering only; enclosure metadata and proxied bytes remain shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SubscriptionEnclosureSettings {
    pub delivery: SubscriptionEnclosureDelivery,
    pub load: SubscriptionEnclosureLoad,
}

impl SubscriptionEnclosureSettings {
    pub fn from_json(value: Option<&Value>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        Self {
            delivery: value
                .get("enclosure_delivery_mode")
                .and_then(Value::as_str)
                .and_then(SubscriptionEnclosureDelivery::parse)
                .unwrap_or_default(),
            load: value
                .get("enclosure_load_mode")
                .and_then(Value::as_str)
                .and_then(SubscriptionEnclosureLoad::parse)
                .unwrap_or_default(),
        }
    }

    pub fn to_json(self) -> Value {
        json!({
            "enclosure_delivery_mode": self.delivery.as_str(),
            "enclosure_load_mode": self.load.as_str(),
        })
    }
}

impl Default for SubscriptionContentSettings {
    fn default() -> Self {
        Self {
            mode: FeedContentMode::Rss,
            lifetime: ContentLifetime::default(),
        }
    }
}

impl SubscriptionContentSettings {
    /// Parse content fields from `user_feeds.settings`, using safe defaults for
    /// missing or malformed values.
    pub fn from_json(value: Option<&Value>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        let mode = value
            .get("content_mode")
            .and_then(Value::as_str)
            .and_then(FeedContentMode::parse)
            .unwrap_or_default();
        let lifetime = match value.get("content_lifetime_days") {
            Some(Value::Null) => ContentLifetime::Infinite,
            Some(Value::Number(days)) => days
                .as_u64()
                .and_then(|days| u16::try_from(days).ok())
                .filter(|days| (1..=MAX_CONTENT_LIFETIME_DAYS).contains(days))
                .map(ContentLifetime::Days)
                .unwrap_or_default(),
            Some(Value::String(value)) => ContentLifetime::parse(value).unwrap_or_default(),
            _ => ContentLifetime::default(),
        };
        Self { mode, lifetime }
    }

    /// JSON fields to merge into `user_feeds.settings` without disturbing
    /// unrelated per-subscription preferences.
    pub fn to_json(&self) -> Value {
        json!({
            "content_mode": self.mode.as_str(),
            "content_lifetime_days": self.lifetime.days(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_rss() {
        assert_eq!(
            FeedContentSettings::from_json(None),
            FeedContentSettings::default()
        );
    }

    #[test]
    fn recommends_public_pages_only_for_selected_catalog_sources() {
        assert_eq!(
            recommended_catalog_content_mode("https://openai.com/news/rss.xml"),
            FeedContentMode::ArticlePage
        );
        assert_eq!(
            recommended_catalog_content_mode("https://blog.rust-lang.org/feed.xml"),
            FeedContentMode::Rss
        );
    }

    #[test]
    fn parses_current_settings() {
        let settings = FeedContentSettings::from_json(Some(&json!({
            "mode": "article_page",
            "selector": " article .body ",
        })));
        assert_eq!(settings.mode, FeedContentMode::ArticlePage);
        assert_eq!(settings.selector.as_deref(), Some("article .body"));
        assert!(settings.mode.fetches_article_pages());
    }

    #[test]
    fn keeps_legacy_enabled_feeds_extracting() {
        let settings = FeedContentSettings::from_json(Some(&json!({ "enabled": true })));
        assert_eq!(settings.mode, FeedContentMode::ArticlePage);
    }

    #[test]
    fn treats_legacy_paywalled_mode_as_rss() {
        let settings = FeedContentSettings::from_json(Some(&json!({ "mode": "paywalled" })));
        assert_eq!(settings.mode, FeedContentMode::Rss);
        assert_eq!(FeedContentMode::parse("paywalled"), None);
    }

    #[test]
    fn rejects_unknown_mode_to_safe_rss_default() {
        let settings = FeedContentSettings::from_json(Some(&json!({
            "mode": "anything",
            "enabled": true,
        })));
        assert_eq!(settings.mode, FeedContentMode::Rss);
    }

    #[test]
    fn parses_personal_content_settings_and_infinite_lifetime() {
        let settings = SubscriptionContentSettings::from_json(Some(&json!({
            "content_mode": "article_page",
            "content_lifetime_days": null,
        })));
        assert_eq!(settings.mode, FeedContentMode::ArticlePage);
        assert_eq!(settings.lifetime, ContentLifetime::Infinite);
        assert_eq!(settings.to_json()["content_lifetime_days"], Value::Null);
    }

    #[test]
    fn merges_shared_lifetime_using_longest_request() {
        assert_eq!(
            ContentLifetime::Days(7).merge(ContentLifetime::Days(30)),
            ContentLifetime::Days(30)
        );
        assert_eq!(
            ContentLifetime::Days(30).merge(ContentLifetime::Infinite),
            ContentLifetime::Infinite
        );
    }

    #[test]
    fn enclosure_overrides_are_bounded_and_default_to_inherit() {
        let settings = SubscriptionEnclosureSettings::from_json(Some(&json!({
            "enclosure_delivery_mode": "proxy",
            "enclosure_load_mode": "links-only",
        })));
        assert_eq!(settings.delivery, SubscriptionEnclosureDelivery::Proxy);
        assert_eq!(settings.load, SubscriptionEnclosureLoad::LinksOnly);
        assert_eq!(
            SubscriptionEnclosureSettings::from_json(Some(&json!({
                "enclosure_delivery_mode": "anything",
            }))),
            SubscriptionEnclosureSettings::default()
        );
    }
}
