//! Validated, no-network YouTube URL detection and privacy-player policy.

use std::cell::RefCell;
use std::collections::HashSet;

use lol_html::{element, rewrite_str, RewriteStrSettings};
use serde_json::{json, Value};
use url::Url;

const MAX_SOURCE_HOSTS: usize = 32;
const MAX_ARTICLE_VIDEOS: usize = 8;
const MAX_START_SECONDS: u32 = 7 * 24 * 60 * 60;
const DEFAULT_SOURCE_HOSTS: &[&str] = &["youtube.com", "youtube-nocookie.com", "youtu.be"];

/// One canonical YouTube video reference found in an article.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YouTubeVideo {
    /// Validated eleven-character YouTube video identifier.
    pub id: String,
    /// Optional validated start offset preserved from the source link.
    pub start_seconds: Option<u32>,
    /// Canonical no-tracking-parameter publisher fallback URL.
    pub watch_url: String,
}

/// Per-subscription player-mode override stored in `user_feeds.settings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubscriptionYouTubeMode {
    /// Use the user's reading preference.
    #[default]
    Inherit,
    /// Force the privacy-enhanced player for this subscription.
    Privacy,
    /// Never render an iframe for this subscription.
    LinksOnly,
}

impl SubscriptionYouTubeMode {
    /// Parse the exact value accepted by settings forms and JSON storage.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "inherit" => Some(Self::Inherit),
            "privacy" => Some(Self::Privacy),
            "links_only" => Some(Self::LinksOnly),
            _ => None,
        }
    }

    /// Stable form/JSON representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Privacy => "privacy",
            Self::LinksOnly => "links_only",
        }
    }

    /// Read the override from `user_feeds.settings`, defaulting to inherit.
    pub fn from_json(value: Option<&Value>) -> Self {
        value
            .and_then(|settings| settings.get("youtube_player_mode"))
            .and_then(Value::as_str)
            .and_then(Self::parse)
            .unwrap_or_default()
    }

    /// Serialize only the field merged into `user_feeds.settings`.
    pub fn to_json(self) -> Value {
        json!({ "youtube_player_mode": self.as_str() })
    }
}

/// Immutable instance boundary for recognized YouTube source URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YouTubePolicy {
    enabled: bool,
    source_hosts: Vec<String>,
    alternative_name: Option<String>,
    alternative_base_url: Option<Url>,
}

impl Default for YouTubePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            source_hosts: DEFAULT_SOURCE_HOSTS
                .iter()
                .map(|host| (*host).to_string())
                .collect(),
            alternative_name: None,
            alternative_base_url: None,
        }
    }
}

impl YouTubePolicy {
    /// Build a bounded host policy. An empty host list uses RustSS's safe defaults.
    pub fn try_new(enabled: bool, source_hosts: &str) -> Result<Self, String> {
        Self::try_new_with_alternative(enabled, source_hosts, "", "")
    }

    /// Build the source policy plus an optional administrator-approved
    /// Invidious-compatible player origin.
    pub fn try_new_with_alternative(
        enabled: bool,
        source_hosts: &str,
        alternative_name: &str,
        alternative_base_url: &str,
    ) -> Result<Self, String> {
        let source_hosts = if source_hosts.trim().is_empty() {
            DEFAULT_SOURCE_HOSTS
                .iter()
                .map(|host| (*host).to_string())
                .collect()
        } else {
            parse_hosts(source_hosts)?
        };
        let (alternative_name, alternative_base_url) =
            parse_alternative(alternative_name, alternative_base_url)?;
        Ok(Self {
            enabled,
            source_hosts,
            alternative_name,
            alternative_base_url,
        })
    }

    /// Whether the instance permits iframe players at all.
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Number of configured source-domain boundaries.
    pub fn source_host_count(&self) -> usize {
        self.source_hosts.len()
    }

    /// Display name of the configured alternative player.
    pub fn alternative_name(&self) -> Option<&str> {
        self.alternative_name.as_deref()
    }

    /// Origin admitted to the outer reader CSP, if configured.
    pub fn alternative_origin(&self) -> Option<String> {
        let url = self.alternative_base_url.as_ref()?;
        Some(url.origin().ascii_serialization())
    }

    /// Recognize one allowlisted YouTube URL without making a network request.
    pub fn video_from_url(&self, value: &str) -> Option<YouTubeVideo> {
        let url = Url::parse(value).ok()?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        let host = normalize_host(url.host_str()?);
        if !self
            .source_hosts
            .iter()
            .any(|allowed| host_matches(&host, allowed))
        {
            return None;
        }
        let id = if host_matches(&host, "youtu.be") {
            first_path_segment(&url)?
        } else {
            youtube_path_id(&url)?
        };
        valid_video_id(&id).then(|| {
            let start_seconds = timestamp_seconds(&url);
            let mut watch_url = format!("https://www.youtube.com/watch?v={id}");
            if let Some(start_seconds) = start_seconds {
                watch_url.push_str(&format!("&t={start_seconds}s"));
            }
            YouTubeVideo {
                id,
                start_seconds,
                watch_url,
            }
        })
    }

    /// Find up to eight distinct videos in explicit URLs and sanitized article links.
    pub fn videos<'a>(
        &self,
        explicit_urls: impl IntoIterator<Item = &'a str>,
        sanitized_html: &str,
    ) -> Vec<YouTubeVideo> {
        let videos = RefCell::new(Vec::new());
        let seen = RefCell::new(HashSet::new());
        for value in explicit_urls {
            push_video(self.video_from_url(value), &videos, &seen);
        }
        if videos.borrow().len() < MAX_ARTICLE_VIDEOS {
            let _ = rewrite_str(
                sanitized_html,
                RewriteStrSettings::new().append_element_content_handler(element!(
                    "a[href]",
                    |element| {
                        if let Some(href) = element.get_attribute("href") {
                            push_video(self.video_from_url(&href), &videos, &seen);
                        }
                        Ok(())
                    }
                )),
            );
        }
        videos.into_inner()
    }

    /// Build the fixed privacy-enhanced embed origin and bounded player parameters.
    pub fn embed_url(
        &self,
        id: &str,
        start_seconds: Option<u32>,
        autoplay: bool,
        captions: &str,
    ) -> Option<String> {
        if !self.enabled || !valid_video_id(id) {
            return None;
        }
        let mut url = Url::parse(&format!("https://www.youtube-nocookie.com/embed/{id}")).ok()?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("playsinline", "1");
            query.append_pair("rel", "0");
            if let Some(start_seconds) = start_seconds.filter(|value| *value > 0) {
                query.append_pair("start", &start_seconds.to_string());
            }
            if autoplay {
                query.append_pair("autoplay", "1");
            }
            if captions == "on" {
                query.append_pair("cc_load_policy", "1");
            }
        }
        Some(url.into())
    }

    /// Build either the fixed privacy player or the configured alternative.
    pub fn embed_url_for(
        &self,
        provider: &str,
        id: &str,
        start_seconds: Option<u32>,
        autoplay: bool,
        captions: &str,
    ) -> Option<String> {
        if provider != "alternative" {
            return self.embed_url(id, start_seconds, autoplay, captions);
        }
        if !self.enabled || !valid_video_id(id) {
            return None;
        }
        let mut url = self.alternative_base_url.clone()?;
        {
            let mut segments = url.path_segments_mut().ok()?;
            segments.pop_if_empty();
            segments.push("embed");
            segments.push(id);
        }
        {
            let mut query = url.query_pairs_mut();
            if let Some(start_seconds) = start_seconds.filter(|value| *value > 0) {
                query.append_pair("start", &start_seconds.to_string());
            }
            if autoplay {
                query.append_pair("autoplay", "1");
            }
        }
        Some(url.into())
    }
}

fn parse_alternative(name: &str, base_url: &str) -> Result<(Option<String>, Option<Url>), String> {
    let name = name.trim();
    let base_url = base_url.trim();
    if name.is_empty() && base_url.is_empty() {
        return Ok((None, None));
    }
    if name.is_empty() || base_url.is_empty() {
        return Err(
            "YOUTUBE_ALTERNATIVE_PLAYER_NAME and YOUTUBE_ALTERNATIVE_PLAYER_URL must be configured together"
                .to_string(),
        );
    }
    if name.chars().count() > 40 || name.chars().any(char::is_control) {
        return Err(
            "YOUTUBE_ALTERNATIVE_PLAYER_NAME must contain 1 to 40 printable characters".to_string(),
        );
    }
    let mut url = Url::parse(base_url)
        .map_err(|_| "YOUTUBE_ALTERNATIVE_PLAYER_URL must be an absolute HTTPS URL".to_string())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "YOUTUBE_ALTERNATIVE_PLAYER_URL must be a credential-free HTTPS base URL without query or fragment"
                .to_string(),
        );
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok((Some(name.to_string()), Some(url)))
}

fn timestamp_seconds(url: &Url) -> Option<u32> {
    for (key, value) in url.query_pairs() {
        if matches!(key.as_ref(), "t" | "start" | "time_continue") {
            if let Some(seconds) = parse_timestamp(&value) {
                return Some(seconds);
            }
        }
    }
    let fragment = url.fragment()?;
    let fragment = fragment.strip_prefix("t=").unwrap_or(fragment);
    parse_timestamp(fragment)
}

fn parse_timestamp(value: &str) -> Option<u32> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        return None;
    }
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return bounded_seconds(value.parse().ok()?);
    }
    if value.contains(':') {
        return parse_colon_timestamp(&value);
    }

    let mut total = 0_u32;
    let mut number = String::new();
    let mut previous_rank = 4_u8;
    for byte in value.bytes() {
        if byte.is_ascii_digit() {
            number.push(char::from(byte));
            continue;
        }
        let (rank, multiplier) = match byte {
            b'h' => (3, 60_u32 * 60),
            b'm' => (2, 60),
            b's' => (1, 1),
            _ => return None,
        };
        if number.is_empty() || rank >= previous_rank {
            return None;
        }
        let part = number.parse::<u32>().ok()?;
        total = total.checked_add(part.checked_mul(multiplier)?)?;
        number.clear();
        previous_rank = rank;
    }
    if !number.is_empty() || previous_rank == 4 {
        return None;
    }
    bounded_seconds(total)
}

fn parse_colon_timestamp(value: &str) -> Option<u32> {
    let parts: Vec<_> = value.split(':').collect();
    if !(2..=3).contains(&parts.len())
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let mut total = 0_u32;
    for (index, part) in parts.iter().enumerate() {
        let parsed = part.parse::<u32>().ok()?;
        if index > 0 && parsed >= 60 {
            return None;
        }
        total = total.checked_mul(60)?.checked_add(parsed)?;
    }
    bounded_seconds(total)
}

fn bounded_seconds(value: u32) -> Option<u32> {
    (value > 0 && value <= MAX_START_SECONDS).then_some(value)
}

fn push_video(
    video: Option<YouTubeVideo>,
    videos: &RefCell<Vec<YouTubeVideo>>,
    seen: &RefCell<HashSet<String>>,
) {
    let Some(video) = video else {
        return;
    };
    if videos.borrow().len() >= MAX_ARTICLE_VIDEOS {
        return;
    }
    if seen.borrow_mut().insert(video.id.clone()) {
        videos.borrow_mut().push(video);
    }
}

fn youtube_path_id(url: &Url) -> Option<String> {
    let mut segments = url.path_segments()?;
    match segments.next().unwrap_or_default() {
        "watch" => url
            .query_pairs()
            .find(|(key, _)| key == "v")
            .map(|(_, value)| value.into_owned())
            .filter(|value| valid_video_id(value)),
        "embed" | "shorts" | "live" | "v" => segments.next().map(str::to_string),
        _ => None,
    }
}

fn first_path_segment(url: &Url) -> Option<String> {
    url.path_segments()?
        .find(|segment| !segment.is_empty())
        .map(str::to_string)
}

pub fn valid_video_id(value: &str) -> bool {
    value.len() == 11
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn parse_hosts(value: &str) -> Result<Vec<String>, String> {
    let entries: Vec<_> = value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect();
    if entries.len() > MAX_SOURCE_HOSTS {
        return Err(format!(
            "YOUTUBE_SOURCE_HOSTS accepts at most {MAX_SOURCE_HOSTS} hosts"
        ));
    }
    let mut hosts = Vec::new();
    for entry in entries {
        if entry.contains("://") || entry.contains(['/', '?', '#', '@']) {
            return Err(format!(
                "YOUTUBE_SOURCE_HOSTS entry '{entry}' must be a hostname"
            ));
        }
        let host = normalize_host(entry);
        if !valid_host(&host) {
            return Err(format!(
                "YOUTUBE_SOURCE_HOSTS entry '{entry}' is not a valid hostname"
            ));
        }
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    if hosts.is_empty() {
        return Err("YOUTUBE_SOURCE_HOSTS must contain at least one hostname".to_string());
    }
    Ok(hosts)
}

fn normalize_host(value: &str) -> String {
    value.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
}

fn host_matches(host: &str, allowed: &str) -> bool {
    host == allowed || host.ends_with(&format!(".{allowed}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_supported_urls_and_deduplicates_article_links() {
        let policy = YouTubePolicy::default();
        let videos = policy.videos(
            ["https://youtu.be/dQw4w9WgXcQ?t=42"],
            r#"<p><a href="https://www.youtube.com/watch?v=dQw4w9WgXcQ&utm_source=rss">same</a><a href="https://www.youtube.com/shorts/aqz-KE-bpKQ">second</a></p>"#,
        );
        assert_eq!(videos.len(), 2);
        assert_eq!(videos[0].id, "dQw4w9WgXcQ");
        assert_eq!(videos[1].id, "aqz-KE-bpKQ");
        assert_eq!(
            videos[0].watch_url,
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s"
        );
        assert_eq!(videos[0].start_seconds, Some(42));
    }

    #[test]
    fn rejects_unlisted_hosts_credentials_and_invalid_ids() {
        let policy = YouTubePolicy::default();
        for value in [
            "https://example.com/watch?v=dQw4w9WgXcQ",
            "https://youtube.com.evil.test/watch?v=dQw4w9WgXcQ",
            "https://user@youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/watch?v=too-short",
        ] {
            assert_eq!(policy.video_from_url(value), None, "accepted {value}");
        }
    }

    #[test]
    fn privacy_embed_never_uses_the_source_origin() {
        let policy = YouTubePolicy::default();
        let embed = policy
            .embed_url("dQw4w9WgXcQ", Some(62), true, "on")
            .expect("valid embed");
        assert!(embed.starts_with("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ?"));
        assert!(embed.contains("autoplay=1"));
        assert!(embed.contains("cc_load_policy=1"));
        assert!(embed.contains("start=62"));
    }

    #[test]
    fn validates_and_builds_an_alternative_player() {
        let policy = YouTubePolicy::try_new_with_alternative(
            true,
            "",
            "Private Invidious",
            "https://video.example.org/invidious",
        )
        .unwrap();
        assert_eq!(policy.alternative_name(), Some("Private Invidious"));
        assert_eq!(
            policy.alternative_origin().as_deref(),
            Some("https://video.example.org")
        );
        let embed = policy
            .embed_url_for("alternative", "dQw4w9WgXcQ", Some(62), true, "on")
            .unwrap();
        assert_eq!(
            embed,
            "https://video.example.org/invidious/embed/dQw4w9WgXcQ?start=62&autoplay=1"
        );
        assert!(policy
            .embed_url_for("privacy", "dQw4w9WgXcQ", None, false, "default")
            .unwrap()
            .starts_with("https://www.youtube-nocookie.com/embed/"));

        for (name, url) in [
            ("", "https://video.example.org"),
            ("Invidious", ""),
            ("Invidious", "http://video.example.org"),
            ("Invidious", "https://user@video.example.org"),
            ("Invidious", "https://video.example.org?target=other"),
        ] {
            assert!(YouTubePolicy::try_new_with_alternative(true, "", name, url).is_err());
        }
    }

    #[test]
    fn disabled_policy_still_recognizes_links_but_never_embeds() {
        let policy = YouTubePolicy::try_new(false, "").expect("default hosts");
        assert!(policy
            .video_from_url("https://youtube.com/watch?v=dQw4w9WgXcQ")
            .is_some());
        assert_eq!(
            policy.embed_url("dQw4w9WgXcQ", None, false, "default"),
            None
        );
    }

    #[test]
    fn preserves_common_timestamp_shapes_and_ignores_invalid_offsets() {
        let policy = YouTubePolicy::default();
        for (value, expected) in [
            ("https://youtu.be/dQw4w9WgXcQ?t=1m2s", Some(62)),
            (
                "https://youtube.com/watch?v=dQw4w9WgXcQ&start=1:02",
                Some(62),
            ),
            (
                "https://youtube.com/shorts/dQw4w9WgXcQ#t=1h2m3s",
                Some(3_723),
            ),
            ("https://youtube.com/embed/dQw4w9WgXcQ?t=0", None),
            (
                "https://youtube.com/watch?v=dQw4w9WgXcQ&t=999999999999",
                None,
            ),
            ("https://youtube.com/watch?v=dQw4w9WgXcQ&t=2m1h", None),
        ] {
            assert_eq!(
                policy
                    .video_from_url(value)
                    .and_then(|video| video.start_seconds),
                expected,
                "timestamp parsing differed for {value}"
            );
        }
    }

    #[test]
    fn validates_and_bounds_custom_hosts() {
        let policy = YouTubePolicy::try_new(true, "video.example.org, VIDEO.example.org")
            .expect("valid hosts");
        assert_eq!(policy.source_host_count(), 1);
        assert!(YouTubePolicy::try_new(true, "https://youtube.com").is_err());
        assert!(YouTubePolicy::try_new(true, ".youtube.com").is_err());
        assert!(YouTubePolicy::try_new(true, "you..tube.com").is_err());
        assert!(YouTubePolicy::try_new(true, &vec!["h.example"; 33].join(",")).is_err());
    }

    #[test]
    fn subscription_mode_round_trips_without_touching_other_settings() {
        let existing = json!({
            "content_mode": "article_page",
            "youtube_player_mode": "links_only"
        });
        assert_eq!(
            SubscriptionYouTubeMode::from_json(Some(&existing)),
            SubscriptionYouTubeMode::LinksOnly
        );
        assert_eq!(
            SubscriptionYouTubeMode::Privacy.to_json()["youtube_player_mode"],
            "privacy"
        );
    }
}
