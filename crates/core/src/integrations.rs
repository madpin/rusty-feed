//! Provider capability catalog shared by settings, delivery, and documentation.

#[derive(Debug, Clone, Copy)]
pub struct ProviderCapability {
    pub id: &'static str,
    pub name: &'static str,
    pub saves_articles: bool,
    pub sends_notifications: bool,
    pub payload_kind: &'static str,
}

pub const PROVIDERS: &[ProviderCapability] = &[
    ProviderCapability {
        id: "apprise",
        name: "Apprise",
        saves_articles: false,
        sends_notifications: true,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "archive-org",
        name: "Archive.org",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "betula",
        name: "Betula",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "cubox",
        name: "Cubox",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "discord",
        name: "Discord",
        saves_articles: true,
        sends_notifications: true,
        payload_kind: "discord",
    },
    ProviderCapability {
        id: "espial",
        name: "Espial",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "instapaper",
        name: "Instapaper",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "karakeep",
        name: "Karakeep",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "linkace",
        name: "LinkAce",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "linkding",
        name: "Linkding",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "linktaco",
        name: "LinkTaco",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "linkwarden",
        name: "Linkwarden",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "matrix",
        name: "Matrix",
        saves_articles: true,
        sends_notifications: true,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "notion",
        name: "Notion",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "ntfy",
        name: "Ntfy",
        saves_articles: false,
        sends_notifications: true,
        payload_kind: "ntfy",
    },
    ProviderCapability {
        id: "nunux-keeper",
        name: "Nunux Keeper",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "pinboard",
        name: "Pinboard",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "pushover",
        name: "Pushover",
        saves_articles: false,
        sends_notifications: true,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "raindrop",
        name: "Raindrop",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "readeck",
        name: "Readeck",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "readwise-reader",
        name: "Readwise Reader",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "rss-bridge",
        name: "RSS-Bridge",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "shaarli",
        name: "Shaarli",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "shiori",
        name: "Shiori",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "slack",
        name: "Slack",
        saves_articles: true,
        sends_notifications: true,
        payload_kind: "slack",
    },
    ProviderCapability {
        id: "telegram",
        name: "Telegram",
        saves_articles: true,
        sends_notifications: true,
        payload_kind: "telegram",
    },
    ProviderCapability {
        id: "wallabag",
        name: "Wallabag",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
    },
    ProviderCapability {
        id: "webhook",
        name: "Signed webhook",
        saves_articles: true,
        sends_notifications: true,
        payload_kind: "json",
    },
];

pub fn provider(id: &str) -> Option<ProviderCapability> {
    PROVIDERS.iter().copied().find(|provider| provider.id == id)
}

pub fn render_template(template: &str, payload: &serde_json::Value, tags: &[String]) -> String {
    let article = payload.get("article").unwrap_or(&serde_json::Value::Null);
    let values = [
        (
            "{{title}}",
            article
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(""),
        ),
        (
            "{{url}}",
            article
                .get("url")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(""),
        ),
        (
            "{{author}}",
            article
                .get("author")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(""),
        ),
        (
            "{{excerpt}}",
            article
                .get("excerpt")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(""),
        ),
        (
            "{{event}}",
            payload
                .get("event_type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(""),
        ),
    ];
    let mut rendered = template.chars().take(8_192).collect::<String>();
    for (token, value) in values {
        rendered = rendered.replace(token, value);
    }
    rendered.replace("{{tags}}", &tags.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exposes_more_than_twenty_five_capability_profiles() {
        assert!(PROVIDERS.len() >= 25);
        assert!(PROVIDERS.iter().any(|provider| provider.id == "webhook"));
        assert!(PROVIDERS.iter().any(|provider| provider.id == "wallabag"));
    }
}
