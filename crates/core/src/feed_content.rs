use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedContentSettings {
    pub mode: FeedContentMode,
    pub selector: Option<String>,
}

impl Default for FeedContentSettings {
    fn default() -> Self {
        Self {
            mode: FeedContentMode::Rss,
            selector: None,
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
        Self { mode, selector }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "mode": self.mode.as_str(),
            "selector": self.selector,
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
}
