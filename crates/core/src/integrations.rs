//! Provider capability catalog shared by settings, delivery, and documentation.

#[derive(Debug, Clone, Copy)]
pub struct ProviderCapability {
    pub id: &'static str,
    pub name: &'static str,
    pub saves_articles: bool,
    pub sends_notifications: bool,
    pub payload_kind: &'static str,
    /// What to paste into the "HTTPS endpoint" box for this destination.
    pub endpoint_hint: &'static str,
    /// Which credential mode/value the "Credential" box expects.
    pub credential_hint: &'static str,
}

impl ProviderCapability {
    /// Human label for the JSON shape RustSS posts. `slack`/`discord`/
    /// `telegram`/`ntfy` receive a body those services accept directly; every
    /// other destination receives RustSS's generic JSON envelope
    /// (`{ rustss, message, tags }`) and may need a small relay to reshape it.
    pub fn payload_summary(self) -> &'static str {
        match self.payload_kind {
            "slack" => "Slack message: { text }",
            "discord" => "Discord message: { content }",
            "telegram" => "Telegram sendMessage: { text }",
            "ntfy" => "ntfy publish: { topic: rustss, title, message, tags }",
            _ => "Generic RustSS JSON: { rustss, message, tags }",
        }
    }
}

pub const PROVIDERS: &[ProviderCapability] = &[
    ProviderCapability {
        id: "apprise",
        name: "Apprise",
        saves_articles: false,
        sends_notifications: true,
        payload_kind: "json",
        endpoint_hint: "Your Apprise API server's persistent notify URL, e.g. https://apprise.example/notify/<config-key>.",
        credential_hint: "None, unless you protected the server — then Bearer with its token.",
    },
    ProviderCapability {
        id: "archive-org",
        name: "Archive.org",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "https://web.archive.org/save/ to trigger Save Page Now.",
        credential_hint: "None for anonymous captures; Bearer with an archive.org token to attribute them.",
    },
    ProviderCapability {
        id: "betula",
        name: "Betula",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "Your single-user Betula instance's add-bookmark URL.",
        credential_hint: "Basic — your Betula username:password.",
    },
    ProviderCapability {
        id: "cubox",
        name: "Cubox",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "Your Cubox inbound API URL (it already contains your personal key), e.g. https://cubox.pro/c/api/save/<key>.",
        credential_hint: "None — the key is embedded in the URL.",
    },
    ProviderCapability {
        id: "discord",
        name: "Discord",
        saves_articles: true,
        sends_notifications: true,
        payload_kind: "discord",
        endpoint_hint: "A Discord channel webhook URL: https://discord.com/api/webhooks/<id>/<token>.",
        credential_hint: "None — the token is in the URL.",
    },
    ProviderCapability {
        id: "espial",
        name: "Espial",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "Your Espial instance's add-bookmark API URL.",
        credential_hint: "Basic — your Espial username:password.",
    },
    ProviderCapability {
        id: "instapaper",
        name: "Instapaper",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "https://www.instapaper.com/api/add (Simple API).",
        credential_hint: "Basic — your Instapaper email:password.",
    },
    ProviderCapability {
        id: "karakeep",
        name: "Karakeep",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "Your Karakeep (Hoarder) API, e.g. https://karakeep.example/api/v1/bookmarks.",
        credential_hint: "Bearer — a Karakeep API key.",
    },
    ProviderCapability {
        id: "linkace",
        name: "LinkAce",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "Your LinkAce API, e.g. https://linkace.example/api/v1/links.",
        credential_hint: "Bearer — a LinkAce API token.",
    },
    ProviderCapability {
        id: "linkding",
        name: "Linkding",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "Your Linkding API, e.g. https://linkding.example/api/bookmarks/.",
        credential_hint: "Bearer — your Linkding REST API token.",
    },
    ProviderCapability {
        id: "linktaco",
        name: "LinkTaco",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "Your LinkTaco API endpoint.",
        credential_hint: "Bearer — a LinkTaco API token.",
    },
    ProviderCapability {
        id: "linkwarden",
        name: "Linkwarden",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "Your Linkwarden API, e.g. https://linkwarden.example/api/v1/links.",
        credential_hint: "Bearer — a Linkwarden access token.",
    },
    ProviderCapability {
        id: "matrix",
        name: "Matrix",
        saves_articles: true,
        sends_notifications: true,
        payload_kind: "json",
        endpoint_hint: "A Matrix room send URL, e.g. https://matrix.example/_matrix/client/v3/rooms/<roomId>/send/m.room.message.",
        credential_hint: "Bearer — a Matrix access token. Matrix expects an m.room.message body, so a relay is recommended.",
    },
    ProviderCapability {
        id: "notion",
        name: "Notion",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "https://api.notion.com/v1/pages.",
        credential_hint: "Bearer — a Notion internal integration token. Notion needs a parent database, so a relay is recommended.",
    },
    ProviderCapability {
        id: "ntfy",
        name: "Ntfy",
        saves_articles: false,
        sends_notifications: true,
        payload_kind: "ntfy",
        endpoint_hint: "Your ntfy server base URL, e.g. https://ntfy.sh. RustSS publishes to the 'rustss' topic.",
        credential_hint: "None, or Bearer with an ntfy access token if the topic is protected.",
    },
    ProviderCapability {
        id: "nunux-keeper",
        name: "Nunux Keeper",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "Your Nunux Keeper API, e.g. https://keeper.example/api/documents.",
        credential_hint: "Bearer — a Nunux Keeper API key.",
    },
    ProviderCapability {
        id: "pinboard",
        name: "Pinboard",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "https://api.pinboard.in/v1/posts/add. Pinboard expects query params, so a relay is recommended.",
        credential_hint: "Basic — username:token from your Pinboard settings.",
    },
    ProviderCapability {
        id: "pushover",
        name: "Pushover",
        saves_articles: false,
        sends_notifications: true,
        payload_kind: "json",
        endpoint_hint: "https://api.pushover.net/1/messages.json. Pushover needs token+user fields, so use a relay that adds them.",
        credential_hint: "None here — supply token and user via the relay or query string.",
    },
    ProviderCapability {
        id: "raindrop",
        name: "Raindrop",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "https://api.raindrop.io/rest/v1/raindrop.",
        credential_hint: "Bearer — a Raindrop test token or OAuth access token.",
    },
    ProviderCapability {
        id: "readeck",
        name: "Readeck",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "Your Readeck API, e.g. https://readeck.example/api/bookmarks.",
        credential_hint: "Bearer — a Readeck API token.",
    },
    ProviderCapability {
        id: "readwise-reader",
        name: "Readwise Reader",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "https://readwise.io/api/v3/save/. Readwise expects a url field, so a relay is recommended.",
        credential_hint: "Bearer — your Readwise access token.",
    },
    ProviderCapability {
        id: "rss-bridge",
        name: "RSS-Bridge",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "Your RSS-Bridge instance URL.",
        credential_hint: "None, unless your instance is protected — then Basic.",
    },
    ProviderCapability {
        id: "shaarli",
        name: "Shaarli",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "Your Shaarli API, e.g. https://shaarli.example/api/v1/links.",
        credential_hint: "Bearer — a Shaarli JWT minted from your API secret.",
    },
    ProviderCapability {
        id: "shiori",
        name: "Shiori",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "Your Shiori API, e.g. https://shiori.example/api/bookmarks.",
        credential_hint: "Bearer — a Shiori session token.",
    },
    ProviderCapability {
        id: "slack",
        name: "Slack",
        saves_articles: true,
        sends_notifications: true,
        payload_kind: "slack",
        endpoint_hint: "A Slack incoming webhook URL: https://hooks.slack.com/services/<...>.",
        credential_hint: "None — the webhook URL is itself the secret.",
    },
    ProviderCapability {
        id: "telegram",
        name: "Telegram",
        saves_articles: true,
        sends_notifications: true,
        payload_kind: "telegram",
        endpoint_hint: "https://api.telegram.org/bot<token>/sendMessage?chat_id=<chat-id>.",
        credential_hint: "None — the bot token and chat id live in the URL.",
    },
    ProviderCapability {
        id: "wallabag",
        name: "Wallabag",
        saves_articles: true,
        sends_notifications: false,
        payload_kind: "json",
        endpoint_hint: "Your Wallabag entries API, e.g. https://wallabag.example/api/entries.json.",
        credential_hint: "Bearer — a Wallabag OAuth access token. Wallabag expects a url field, so a relay is recommended.",
    },
    ProviderCapability {
        id: "webhook",
        name: "Signed webhook",
        saves_articles: true,
        sends_notifications: true,
        payload_kind: "json",
        endpoint_hint: "Any HTTPS endpoint you control (your own service, or an automation like n8n/Make/Zapier).",
        credential_hint: "Bearer, Basic, or HMAC. HMAC signs the body as the x-rustss-signature: sha256=<hex> header.",
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
