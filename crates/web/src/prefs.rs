//! User preferences: a typed view over the `user_preferences` JSONB document,
//! plus the settings page handlers. (A small subset for the M5 start; more
//! fields are added alongside the reading UI.)

use askama::Template;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use rusty_feed_db::{user_preferences, Db, User};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::extract::Ajax;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

/// `POST /settings/reset-learning` — clear the user's learned patterns/feedback.
pub async fn reset_learning(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    rusty_feed_search::personalize::reset_learning(&state.db, &state.cache, user.id).await?;
    Ok((jar, Redirect::to("/settings/preferences")).into_response())
}

/// Typed user preferences. `#[serde(default)]` makes missing fields fall back to
/// defaults, so the stored document can evolve without breaking older rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardShortcuts {
    #[serde(default = "default_shortcuts_enabled")]
    pub enabled: bool,
    #[serde(default = "default_next_article")]
    pub next_article: String,
    #[serde(default = "default_previous_article")]
    pub previous_article: String,
    #[serde(default = "default_save_article")]
    pub save_article: String,
    #[serde(default = "default_mark_unread")]
    pub mark_unread: String,
    #[serde(default = "default_next_feed")]
    pub next_feed: String,
    #[serde(default = "default_previous_feed")]
    pub previous_feed: String,
}

fn default_shortcuts_enabled() -> bool {
    true
}

fn default_next_article() -> String {
    "ArrowDown".to_string()
}

fn default_previous_article() -> String {
    "ArrowUp".to_string()
}

fn default_save_article() -> String {
    "s".to_string()
}

fn default_mark_unread() -> String {
    "u".to_string()
}

fn default_next_feed() -> String {
    "ArrowRight".to_string()
}

fn default_previous_feed() -> String {
    "ArrowLeft".to_string()
}

impl Default for KeyboardShortcuts {
    fn default() -> Self {
        Self {
            enabled: default_shortcuts_enabled(),
            next_article: default_next_article(),
            previous_article: default_previous_article(),
            save_article: default_save_article(),
            mark_unread: default_mark_unread(),
            next_feed: default_next_feed(),
            previous_feed: default_previous_feed(),
        }
    }
}

impl KeyboardShortcuts {
    fn sanitized(mut self) -> Self {
        let mut used = std::collections::HashSet::new();
        for binding in [
            &mut self.next_article,
            &mut self.previous_article,
            &mut self.save_article,
            &mut self.mark_unread,
            &mut self.next_feed,
            &mut self.previous_feed,
        ] {
            *binding = normalize_shortcut(binding);
            if !binding.is_empty() && !used.insert(binding.clone()) {
                binding.clear();
            }
        }
        self
    }

    fn validated(mut self) -> Result<Self, String> {
        let mut used = std::collections::HashMap::new();
        for (label, binding) in [
            ("Next article", &mut self.next_article),
            ("Previous article", &mut self.previous_article),
            ("Save for later", &mut self.save_article),
            ("Mark unread", &mut self.mark_unread),
            ("Next feed", &mut self.next_feed),
            ("Previous feed", &mut self.previous_feed),
        ] {
            let submitted = binding.trim();
            let normalized = normalize_shortcut(submitted);
            if !submitted.is_empty() && normalized.is_empty() {
                return Err(format!("{label} has an unsupported shortcut."));
            }
            if let Some(previous) = used.insert(normalized.clone(), label) {
                if !normalized.is_empty() {
                    return Err(format!("{label} conflicts with {previous}."));
                }
            }
            *binding = normalized;
        }
        Ok(self)
    }
}

fn normalize_shortcut(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return String::new();
    }
    let mut modifiers = Vec::new();
    let mut key = None;
    for part in value.split('+') {
        let part = part.trim();
        match part.to_ascii_lowercase().as_str() {
            "shift" if !modifiers.contains(&"Shift") => modifiers.push("Shift"),
            "ctrl" | "control" | "alt" | "meta" | "cmd" | "command" => return String::new(),
            _ if key.is_none() => key = normalize_shortcut_key(part),
            _ => return String::new(),
        }
    }
    let Some(key) = key else {
        return String::new();
    };
    let mut parts: Vec<String> = modifiers.into_iter().map(str::to_string).collect();
    parts.push(key);
    parts.join("+")
}

fn normalize_shortcut_key(value: &str) -> Option<String> {
    if value.len() == 1 && value.chars().all(|character| character.is_ascii_graphic()) {
        return Some(value.to_ascii_lowercase());
    }
    match value.to_ascii_lowercase().as_str() {
        "arrowdown" => Some("ArrowDown".to_string()),
        "arrowup" => Some("ArrowUp".to_string()),
        "arrowleft" => Some("ArrowLeft".to_string()),
        "arrowright" => Some("ArrowRight".to_string()),
        "pagedown" => Some("PageDown".to_string()),
        "pageup" => Some("PageUp".to_string()),
        "home" => Some("Home".to_string()),
        "end" => Some("End".to_string()),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub locale: String,
    pub fallback_locale: String,
    pub timezone: String,
    pub date_format: String,
    pub number_format: String,
    pub custom_script_enabled: bool,
    pub theme: String,
    pub articles_per_page: u32,
    pub show_read_articles: bool,
    /// `side-panel`, `standalone`, or `inline`.
    pub reading_mode: String,
    /// Mark an article read automatically when opened.
    pub auto_mark_read: bool,
    pub article_density: String,
    /// `sans` or `serif`, applied with every color theme.
    pub font_family: String,
    pub reader_font_size: String,
    pub reader_line_height: String,
    pub sidebar_width: u8,
    pub sidebar_collapsed: bool,
    /// Keep the per-feed quick tools (content/tags shortcuts) pinned open in the
    /// sidebar across navigation until the user toggles them off again.
    pub quick_tools_visible: bool,
    pub reader_panel_width: u8,
    /// `clean` (default) or `original` publisher article links.
    pub external_link_mode: String,
    /// `new-tab` (default) or `same-tab`; privacy attributes are always fixed.
    pub external_open_mode: String,
    /// Show the per-article count of tracking images removed during shared cleanup.
    pub show_blocked_tracking_notice: bool,
    /// `instance`, `hybrid`, `keyword`, or `semantic`.
    pub search_mode: String,
    /// `instance`, `simple`, or `english` PostgreSQL text configuration.
    pub search_text_config: String,
    /// Personal semantic contribution to hybrid ranking; `None` inherits the instance.
    pub search_semantic_weight_percent: Option<u8>,
    /// `on-demand`, `metadata`, or `links-only`.
    pub enclosure_load_mode: String,
    /// `proxy`, `mixed-content`, or `direct`.
    pub enclosure_delivery_mode: String,
    /// Media kinds that may use an inline player after MIME safety checks.
    pub enclosure_player_kinds: Vec<String>,
    pub enclosure_show_links: bool,
    /// `privacy` (youtube-nocookie.com) or `links_only`.
    pub youtube_player_mode: String,
    /// `privacy` or `alternative`; ignored when links-only mode is selected.
    pub youtube_player_provider: String,
    /// Start playback after the user explicitly loads the player.
    pub youtube_autoplay: bool,
    /// `default` or `on` (YouTube exposes no supported force-off parameter).
    pub youtube_captions: String,
    /// Enable horizontal swipe navigation on touch-first devices.
    pub touch_gestures_enabled: bool,
    /// `low`, `medium`, or `high`; higher sensitivity needs a shorter swipe.
    pub touch_gesture_sensitivity: String,
    /// `natural` or `reversed` article-navigation direction.
    pub touch_gesture_direction: String,
    /// Content interests (catalog categories) chosen in the setup wizard; used to
    /// pre-filter feed suggestions and remembered so the wizard can be re-run.
    pub interests: Vec<String>,
    pub shortcuts: KeyboardShortcuts,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            locale: "en".to_string(),
            fallback_locale: "en".to_string(),
            timezone: "UTC".to_string(),
            date_format: "mdy".to_string(),
            number_format: "comma".to_string(),
            custom_script_enabled: false,
            theme: "system".to_string(),
            articles_per_page: 20,
            show_read_articles: true,
            reading_mode: "side-panel".to_string(),
            auto_mark_read: true,
            article_density: "comfortable".to_string(),
            font_family: "sans".to_string(),
            reader_font_size: "medium".to_string(),
            reader_line_height: "relaxed".to_string(),
            sidebar_width: 17,
            sidebar_collapsed: false,
            quick_tools_visible: false,
            reader_panel_width: 56,
            external_link_mode: "clean".to_string(),
            external_open_mode: "new-tab".to_string(),
            show_blocked_tracking_notice: true,
            search_mode: "instance".to_string(),
            search_text_config: "instance".to_string(),
            search_semantic_weight_percent: None,
            enclosure_load_mode: "on-demand".to_string(),
            enclosure_delivery_mode: "proxy".to_string(),
            enclosure_player_kinds: vec![
                "audio".to_string(),
                "video".to_string(),
                "image".to_string(),
            ],
            enclosure_show_links: true,
            youtube_player_mode: "privacy".to_string(),
            youtube_player_provider: "privacy".to_string(),
            youtube_autoplay: false,
            youtube_captions: "default".to_string(),
            touch_gestures_enabled: false,
            touch_gesture_sensitivity: "medium".to_string(),
            touch_gesture_direction: "natural".to_string(),
            interests: Vec::new(),
            shortcuts: KeyboardShortcuts::default(),
        }
    }
}

impl Preferences {
    pub fn text(&self, key: &str) -> &'static str {
        let translated = rusty_feed_core::i18n::message(&self.locale, key);
        if translated.is_empty() {
            rusty_feed_core::i18n::message(&self.fallback_locale, key)
        } else {
            translated
        }
    }

    pub fn format_date(&self, value: time::OffsetDateTime) -> String {
        rusty_feed_core::i18n::format_date(value, &self.timezone, &self.date_format)
    }

    pub fn format_number(&self, value: &i64) -> String {
        rusty_feed_core::i18n::format_number(*value, &self.number_format)
    }

    pub fn text_direction(&self) -> &'static str {
        rusty_feed_core::i18n::text_direction(&self.locale)
    }

    pub(crate) fn app_version(&self) -> &'static str {
        rusty_feed_core::VERSION
    }

    /// Whether the user selected `value` as a content interest (used by the
    /// setup wizard to pre-check interest chips on re-run).
    pub fn has_interest(&self, value: &str) -> bool {
        self.interests.iter().any(|interest| interest == value)
    }

    fn sanitized(mut self) -> Self {
        if !rusty_feed_core::i18n::is_supported_locale(&self.locale) {
            self.locale = "en".to_string();
        }
        if !rusty_feed_core::i18n::is_supported_locale(&self.fallback_locale) {
            self.fallback_locale = "en".to_string();
        }
        if !rusty_feed_core::i18n::is_supported_timezone(&self.timezone) {
            self.timezone = "UTC".to_string();
        }
        if !matches!(self.date_format.as_str(), "mdy" | "dmy" | "ymd") {
            self.date_format = "mdy".to_string();
        }
        if !matches!(self.number_format.as_str(), "comma" | "dot") {
            self.number_format = "comma".to_string();
        }
        if !matches!(
            self.theme.as_str(),
            "system" | "light" | "dark" | "rusty" | "ocean" | "forest" | "plum" | "sunset"
        ) {
            self.theme = "system".to_string();
        }
        if !matches!(
            self.reading_mode.as_str(),
            "side-panel" | "standalone" | "inline"
        ) {
            self.reading_mode = "side-panel".to_string();
        }
        if !matches!(
            self.article_density.as_str(),
            "compact" | "comfortable" | "spacious"
        ) {
            self.article_density = "comfortable".to_string();
        }
        if !matches!(self.font_family.as_str(), "sans" | "serif") {
            self.font_family = "sans".to_string();
        }
        if !matches!(self.reader_font_size.as_str(), "small" | "medium" | "large") {
            self.reader_font_size = "medium".to_string();
        }
        if !matches!(
            self.reader_line_height.as_str(),
            "compact" | "relaxed" | "airy"
        ) {
            self.reader_line_height = "relaxed".to_string();
        }
        if !matches!(
            self.enclosure_load_mode.as_str(),
            "on-demand" | "metadata" | "links-only"
        ) {
            self.enclosure_load_mode = "on-demand".to_string();
        }
        if !matches!(self.external_link_mode.as_str(), "clean" | "original") {
            self.external_link_mode = "clean".to_string();
        }
        if !matches!(self.external_open_mode.as_str(), "new-tab" | "same-tab") {
            self.external_open_mode = "new-tab".to_string();
        }
        if !matches!(
            self.search_mode.as_str(),
            "instance" | "hybrid" | "keyword" | "semantic"
        ) {
            self.search_mode = "instance".to_string();
        }
        if !matches!(
            self.search_text_config.as_str(),
            "instance" | "simple" | "english"
        ) {
            self.search_text_config = "instance".to_string();
        }
        self.search_semantic_weight_percent = self
            .search_semantic_weight_percent
            .filter(|weight| *weight <= 100);
        if !matches!(
            self.enclosure_delivery_mode.as_str(),
            "proxy" | "mixed-content" | "direct"
        ) {
            self.enclosure_delivery_mode = "proxy".to_string();
        }
        self.enclosure_player_kinds
            .retain(|kind| matches!(kind.as_str(), "audio" | "video" | "image"));
        self.enclosure_player_kinds.sort();
        self.enclosure_player_kinds.dedup();
        if !matches!(self.youtube_player_mode.as_str(), "privacy" | "links_only") {
            self.youtube_player_mode = "privacy".to_string();
        }
        if !matches!(
            self.youtube_player_provider.as_str(),
            "privacy" | "alternative"
        ) {
            self.youtube_player_provider = "privacy".to_string();
        }
        if !matches!(self.youtube_captions.as_str(), "default" | "on") {
            self.youtube_captions = "default".to_string();
        }
        if !matches!(
            self.touch_gesture_sensitivity.as_str(),
            "low" | "medium" | "high"
        ) {
            self.touch_gesture_sensitivity = "medium".to_string();
        }
        if !matches!(
            self.touch_gesture_direction.as_str(),
            "natural" | "reversed"
        ) {
            self.touch_gesture_direction = "natural".to_string();
        }
        self.articles_per_page = self.articles_per_page.clamp(5, 100);
        self.sidebar_width = self.sidebar_width.clamp(13, 24);
        self.reader_panel_width = self.reader_panel_width.clamp(38, 68);
        let mut seen = std::collections::HashSet::new();
        self.interests = std::mem::take(&mut self.interests)
            .into_iter()
            .filter_map(|interest| {
                let trimmed = interest.trim();
                (!trimmed.is_empty() && seen.insert(trimmed.to_string()))
                    .then(|| trimmed.to_string())
            })
            .take(40)
            .collect();
        self.shortcuts = self.shortcuts.sanitized();
        self
    }
}

/// Load a user's preferences (defaults on missing/invalid).
pub async fn load(db: &Db, user_id: Uuid) -> Preferences {
    match user_preferences::get(db.pool(), user_id).await {
        Ok(Some(json)) => serde_json::from_value(json).unwrap_or_default(),
        _ => Preferences::default(),
    }
    .sanitized()
}

#[derive(Template)]
#[template(path = "preferences.html")]
struct PreferencesTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: Preferences,
    is_admin: bool,
    pwa_name: String,
    media_proxy_enabled: bool,
    youtube_embeds_enabled: bool,
    youtube_source_host_count: usize,
    prefs_theme: String,
    prefs_per_page: u32,
    prefs_show_read: bool,
    prefs_reading_mode: String,
    prefs_auto_mark_read: bool,
    prefs_density: String,
    prefs_font_family: String,
    prefs_reader_font_size: String,
    prefs_reader_line_height: String,
    prefs_sidebar_width: u8,
    prefs_sidebar_collapsed: bool,
    prefs_reader_panel_width: u8,
    prefs_external_link_mode: String,
    prefs_external_open_mode: String,
    external_links_new_tab_required: bool,
    prefs_show_blocked_tracking_notice: bool,
    tracking_pixel_strictness: String,
    tracking_pixel_blocked_host_count: usize,
    tracking_pixel_allowed_host_count: usize,
    embeddings_enabled: bool,
    prefs_search_mode: String,
    prefs_search_text_config: String,
    prefs_search_semantic_weight_percent: String,
    instance_search_mode: String,
    instance_search_text_config: String,
    instance_search_semantic_weight_percent: u8,
    prefs_enclosure_load_mode: String,
    prefs_enclosure_delivery_mode: String,
    prefs_enclosure_audio: bool,
    prefs_enclosure_video: bool,
    prefs_enclosure_image: bool,
    prefs_enclosure_show_links: bool,
    prefs_youtube_player_mode: String,
    prefs_youtube_player_provider: String,
    youtube_alternative_player_name: Option<String>,
    prefs_youtube_autoplay: bool,
    prefs_youtube_captions: String,
    prefs_touch_gestures_enabled: bool,
    prefs_touch_gesture_sensitivity: String,
    prefs_touch_gesture_direction: String,
    saved: bool,
    error: Option<String>,
    locales: Vec<LocaleOption>,
    prefs_locale: String,
    prefs_fallback_locale: String,
    prefs_timezone: String,
    prefs_date_format: String,
    prefs_number_format: String,
    prefs_custom_script_enabled: bool,
    custom_javascript_enabled: bool,
}

struct LocaleOption {
    code: String,
    name: String,
}

fn empty_string_as_none<'de, D>(deserializer: D) -> Result<Option<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    let value = value.trim();
    if value.is_empty() {
        Ok(None)
    } else {
        value.parse().map(Some).map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
pub struct PrefsForm {
    csrf_token: String,
    theme: String,
    articles_per_page: u32,
    reading_mode: String,
    #[serde(default)]
    locale: Option<String>,
    #[serde(default)]
    fallback_locale: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    date_format: Option<String>,
    #[serde(default)]
    number_format: Option<String>,
    #[serde(default)]
    custom_script_enabled: Option<String>,
    #[serde(default)]
    show_read_articles: Option<String>,
    #[serde(default)]
    auto_mark_read: Option<String>,
    #[serde(default)]
    article_density: Option<String>,
    #[serde(default)]
    font_family: Option<String>,
    #[serde(default)]
    reader_font_size: Option<String>,
    #[serde(default)]
    reader_line_height: Option<String>,
    #[serde(default)]
    sidebar_width: Option<u8>,
    #[serde(default)]
    sidebar_collapsed: Option<String>,
    #[serde(default)]
    reader_panel_width: Option<u8>,
    #[serde(default)]
    external_link_mode: Option<String>,
    #[serde(default)]
    external_open_mode: Option<String>,
    #[serde(default)]
    privacy_settings_present: Option<String>,
    #[serde(default)]
    show_blocked_tracking_notice: Option<String>,
    #[serde(default)]
    search_settings_present: Option<String>,
    #[serde(default)]
    search_mode: Option<String>,
    #[serde(default)]
    search_text_config: Option<String>,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    search_semantic_weight_percent: Option<u8>,
    #[serde(default)]
    enclosure_settings_present: Option<String>,
    #[serde(default)]
    enclosure_load_mode: Option<String>,
    #[serde(default)]
    enclosure_delivery_mode: Option<String>,
    #[serde(default)]
    enclosure_audio: Option<String>,
    #[serde(default)]
    enclosure_video: Option<String>,
    #[serde(default)]
    enclosure_image: Option<String>,
    #[serde(default)]
    enclosure_show_links: Option<String>,
    #[serde(default)]
    youtube_settings_present: Option<String>,
    #[serde(default)]
    youtube_player_mode: Option<String>,
    #[serde(default)]
    youtube_player_provider: Option<String>,
    #[serde(default)]
    youtube_autoplay: Option<String>,
    #[serde(default)]
    youtube_captions: Option<String>,
    #[serde(default)]
    touch_gestures_enabled: Option<String>,
    #[serde(default)]
    touch_gesture_sensitivity: Option<String>,
    #[serde(default)]
    touch_gesture_direction: Option<String>,
    #[serde(default)]
    shortcuts_enabled: Option<String>,
    #[serde(default)]
    shortcut_next_article: Option<String>,
    #[serde(default)]
    shortcut_previous_article: Option<String>,
    #[serde(default)]
    shortcut_save_article: Option<String>,
    #[serde(default)]
    shortcut_mark_unread: Option<String>,
    #[serde(default)]
    shortcut_next_feed: Option<String>,
    #[serde(default)]
    shortcut_previous_feed: Option<String>,
}

#[derive(Deserialize)]
pub struct LayoutForm {
    csrf_token: String,
    #[serde(default)]
    sidebar_width: Option<u8>,
    #[serde(default)]
    sidebar_collapsed: Option<String>,
    #[serde(default)]
    quick_tools_visible: Option<String>,
    #[serde(default)]
    reader_panel_width: Option<u8>,
}

fn render(tpl: &PreferencesTemplate) -> Result<Html<String>, AppError> {
    tpl.render()
        .map(Html)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))
}

fn preferences_template(
    state: &AppState,
    user: &User,
    csrf_token: String,
    prefs: Preferences,
    saved: bool,
    error: Option<String>,
) -> PreferencesTemplate {
    PreferencesTemplate {
        theme: prefs.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui: prefs.clone(),
        is_admin: user.role == "admin",
        pwa_name: state.settings.pwa_name.clone(),
        media_proxy_enabled: state.settings.media_proxy_enabled,
        youtube_embeds_enabled: state.settings.youtube_policy.enabled(),
        youtube_source_host_count: state.settings.youtube_policy.source_host_count(),
        prefs_theme: prefs.theme,
        prefs_per_page: prefs.articles_per_page,
        prefs_show_read: prefs.show_read_articles,
        prefs_reading_mode: prefs.reading_mode,
        prefs_auto_mark_read: prefs.auto_mark_read,
        prefs_density: prefs.article_density,
        prefs_font_family: prefs.font_family,
        prefs_reader_font_size: prefs.reader_font_size,
        prefs_reader_line_height: prefs.reader_line_height,
        prefs_sidebar_width: prefs.sidebar_width,
        prefs_sidebar_collapsed: prefs.sidebar_collapsed,
        prefs_reader_panel_width: prefs.reader_panel_width,
        prefs_external_link_mode: prefs.external_link_mode.clone(),
        prefs_external_open_mode: if state.settings.external_links_new_tab_required {
            "new-tab".to_string()
        } else {
            prefs.external_open_mode.clone()
        },
        external_links_new_tab_required: state.settings.external_links_new_tab_required,
        prefs_show_blocked_tracking_notice: prefs.show_blocked_tracking_notice,
        tracking_pixel_strictness: state
            .settings
            .tracking_pixel_policy
            .strictness()
            .as_str()
            .to_string(),
        tracking_pixel_blocked_host_count: state
            .settings
            .tracking_pixel_policy
            .blocked_host_count(),
        tracking_pixel_allowed_host_count: state
            .settings
            .tracking_pixel_policy
            .allowed_host_count(),
        embeddings_enabled: state.settings.embeddings_enabled,
        prefs_search_mode: prefs.search_mode.clone(),
        prefs_search_text_config: prefs.search_text_config.clone(),
        prefs_search_semantic_weight_percent: prefs
            .search_semantic_weight_percent
            .map(|weight| weight.to_string())
            .unwrap_or_default(),
        instance_search_mode: state.settings.search_mode.as_str().to_string(),
        instance_search_text_config: state.settings.search_text_config.as_str().to_string(),
        instance_search_semantic_weight_percent: state.settings.search_semantic_weight_percent,
        prefs_enclosure_load_mode: prefs.enclosure_load_mode.clone(),
        prefs_enclosure_delivery_mode: prefs.enclosure_delivery_mode.clone(),
        prefs_enclosure_audio: prefs
            .enclosure_player_kinds
            .iter()
            .any(|kind| kind == "audio"),
        prefs_enclosure_video: prefs
            .enclosure_player_kinds
            .iter()
            .any(|kind| kind == "video"),
        prefs_enclosure_image: prefs
            .enclosure_player_kinds
            .iter()
            .any(|kind| kind == "image"),
        prefs_enclosure_show_links: prefs.enclosure_show_links,
        prefs_youtube_player_mode: prefs.youtube_player_mode.clone(),
        prefs_youtube_player_provider: if state.settings.youtube_policy.alternative_name().is_some()
        {
            prefs.youtube_player_provider.clone()
        } else {
            "privacy".to_string()
        },
        youtube_alternative_player_name: state
            .settings
            .youtube_policy
            .alternative_name()
            .map(str::to_string),
        prefs_youtube_autoplay: prefs.youtube_autoplay,
        prefs_youtube_captions: prefs.youtube_captions.clone(),
        prefs_touch_gestures_enabled: prefs.touch_gestures_enabled,
        prefs_touch_gesture_sensitivity: prefs.touch_gesture_sensitivity.clone(),
        prefs_touch_gesture_direction: prefs.touch_gesture_direction.clone(),
        locales: rusty_feed_core::i18n::SUPPORTED_LOCALES
            .iter()
            .map(|(code, name)| LocaleOption {
                code: (*code).to_string(),
                name: (*name).to_string(),
            })
            .collect(),
        prefs_locale: prefs.locale.clone(),
        prefs_fallback_locale: prefs.fallback_locale.clone(),
        prefs_timezone: prefs.timezone.clone(),
        prefs_date_format: prefs.date_format.clone(),
        prefs_number_format: prefs.number_format.clone(),
        prefs_custom_script_enabled: prefs.custom_script_enabled,
        custom_javascript_enabled: state.settings.custom_javascript_enabled,
        saved,
        error,
    }
}

fn render_page(
    state: &AppState,
    user: &User,
    jar: CookieJar,
    prefs: Preferences,
    saved: bool,
    error: Option<String>,
) -> Result<Response, AppError> {
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    Ok((
        jar,
        render(&preferences_template(
            state, user, csrf_token, prefs, saved, error,
        ))?,
    )
        .into_response())
}

/// `GET /settings/preferences`.
pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let prefs = load(&state.db, user.id).await;
    render_page(&state, &user, jar, prefs, false, None)
}

/// `POST /settings/preferences`.
pub async fn save(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<PrefsForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let current = load(&state.db, user.id).await;
    let current_for_error = current.clone();
    let current_shortcuts = current.shortcuts.clone();
    let search_settings_present = form.search_settings_present.is_some();
    let enclosure_settings_present = form.enclosure_settings_present.is_some();
    let privacy_settings_present = form.privacy_settings_present.is_some();
    let youtube_settings_present = form.youtube_settings_present.is_some();
    let enclosure_player_kinds = if enclosure_settings_present {
        [
            ("audio", form.enclosure_audio.is_some()),
            ("video", form.enclosure_video.is_some()),
            ("image", form.enclosure_image.is_some()),
        ]
        .into_iter()
        .filter(|(_, selected)| *selected)
        .map(|(kind, _)| kind.to_string())
        .collect()
    } else {
        current.enclosure_player_kinds.clone()
    };
    let shortcuts = match (KeyboardShortcuts {
        enabled: form.shortcuts_enabled.is_some(),
        next_article: form
            .shortcut_next_article
            .unwrap_or(current_shortcuts.next_article),
        previous_article: form
            .shortcut_previous_article
            .unwrap_or(current_shortcuts.previous_article),
        save_article: form
            .shortcut_save_article
            .unwrap_or(current_shortcuts.save_article),
        mark_unread: form
            .shortcut_mark_unread
            .unwrap_or(current_shortcuts.mark_unread),
        next_feed: form
            .shortcut_next_feed
            .unwrap_or(current_shortcuts.next_feed),
        previous_feed: form
            .shortcut_previous_feed
            .unwrap_or(current_shortcuts.previous_feed),
    })
    .validated()
    {
        Ok(shortcuts) => shortcuts,
        Err(error) => {
            return render_page(&state, &user, jar, current_for_error, false, Some(error))
        }
    };
    let prefs = Preferences {
        locale: form.locale.unwrap_or(current.locale),
        fallback_locale: form.fallback_locale.unwrap_or(current.fallback_locale),
        timezone: form.timezone.unwrap_or(current.timezone),
        date_format: form.date_format.unwrap_or(current.date_format),
        number_format: form.number_format.unwrap_or(current.number_format),
        custom_script_enabled: state.settings.custom_javascript_enabled
            && form.custom_script_enabled.is_some(),
        theme: form.theme,
        articles_per_page: form.articles_per_page,
        show_read_articles: form.show_read_articles.is_some(),
        reading_mode: form.reading_mode,
        auto_mark_read: form.auto_mark_read.is_some(),
        article_density: form.article_density.unwrap_or(current.article_density),
        font_family: form.font_family.unwrap_or(current.font_family),
        reader_font_size: form.reader_font_size.unwrap_or(current.reader_font_size),
        reader_line_height: form
            .reader_line_height
            .unwrap_or(current.reader_line_height),
        sidebar_width: form.sidebar_width.unwrap_or(current.sidebar_width),
        sidebar_collapsed: form.sidebar_collapsed.is_some(),
        quick_tools_visible: current.quick_tools_visible,
        reader_panel_width: form
            .reader_panel_width
            .unwrap_or(current.reader_panel_width),
        external_link_mode: form
            .external_link_mode
            .unwrap_or(current.external_link_mode),
        external_open_mode: form
            .external_open_mode
            .unwrap_or(current.external_open_mode),
        show_blocked_tracking_notice: if privacy_settings_present {
            form.show_blocked_tracking_notice.is_some()
        } else {
            current.show_blocked_tracking_notice
        },
        // A disabled instance is authoritative: never persist a stray
        // semantic/hybrid choice or weight even if the (hidden) form fields
        // were submitted directly, bypassing the preferences UI.
        search_mode: if !state.settings.embeddings_enabled {
            "instance".to_string()
        } else if search_settings_present {
            form.search_mode.unwrap_or_else(|| "instance".to_string())
        } else {
            current.search_mode
        },
        search_text_config: if search_settings_present {
            form.search_text_config
                .unwrap_or_else(|| "instance".to_string())
        } else {
            current.search_text_config
        },
        search_semantic_weight_percent: if !state.settings.embeddings_enabled {
            None
        } else if search_settings_present {
            form.search_semantic_weight_percent
        } else {
            current.search_semantic_weight_percent
        },
        enclosure_load_mode: form
            .enclosure_load_mode
            .unwrap_or(current.enclosure_load_mode),
        enclosure_delivery_mode: form
            .enclosure_delivery_mode
            .unwrap_or(current.enclosure_delivery_mode),
        enclosure_player_kinds,
        enclosure_show_links: if enclosure_settings_present {
            form.enclosure_show_links.is_some()
        } else {
            current.enclosure_show_links
        },
        youtube_player_mode: if youtube_settings_present {
            form.youtube_player_mode
                .unwrap_or_else(|| "privacy".to_string())
        } else {
            current.youtube_player_mode
        },
        youtube_player_provider: if youtube_settings_present {
            form.youtube_player_provider
                .unwrap_or_else(|| "privacy".to_string())
        } else {
            current.youtube_player_provider
        },
        youtube_autoplay: if youtube_settings_present {
            form.youtube_autoplay.is_some()
        } else {
            current.youtube_autoplay
        },
        youtube_captions: if youtube_settings_present {
            form.youtube_captions
                .unwrap_or_else(|| "default".to_string())
        } else {
            current.youtube_captions
        },
        touch_gestures_enabled: form.touch_gestures_enabled.is_some(),
        touch_gesture_sensitivity: form
            .touch_gesture_sensitivity
            .unwrap_or(current.touch_gesture_sensitivity),
        touch_gesture_direction: form
            .touch_gesture_direction
            .unwrap_or(current.touch_gesture_direction),
        interests: current.interests,
        shortcuts,
    }
    .sanitized();

    let value = serde_json::to_value(&prefs)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("serialize prefs: {e}")))?;
    user_preferences::upsert(state.db.pool(), user.id, &value).await?;

    render_page(&state, &user, jar, prefs, true, None)
}

/// `POST /settings/layout`.
pub async fn save_layout(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Ajax(is_ajax): Ajax,
    jar: CookieJar,
    Form(form): Form<LayoutForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let mut patch = serde_json::Map::new();
    if let Some(width) = form.sidebar_width {
        patch.insert(
            "sidebar_width".to_string(),
            serde_json::json!(width.clamp(13, 24)),
        );
    }
    if let Some(collapsed) = form.sidebar_collapsed.as_deref() {
        let collapsed = match collapsed {
            "true" => true,
            "false" => false,
            _ => return Err(AppError::Validation("Invalid sidebar state.".to_string())),
        };
        patch.insert(
            "sidebar_collapsed".to_string(),
            serde_json::json!(collapsed),
        );
    }
    if let Some(visible) = form.quick_tools_visible.as_deref() {
        let visible = match visible {
            "true" => true,
            "false" => false,
            _ => {
                return Err(AppError::Validation(
                    "Invalid quick tools state.".to_string(),
                ))
            }
        };
        patch.insert(
            "quick_tools_visible".to_string(),
            serde_json::json!(visible),
        );
    }
    if let Some(width) = form.reader_panel_width {
        patch.insert(
            "reader_panel_width".to_string(),
            serde_json::json!(width.clamp(38, 68)),
        );
    }
    if !patch.is_empty() {
        user_preferences::upsert(state.db.pool(), user.id, &serde_json::Value::Object(patch))
            .await?;
    }
    if is_ajax {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Ok((jar, Redirect::to("/settings/preferences")).into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::value::{Error as ValueError, StrDeserializer};

    #[test]
    fn exposes_application_version() {
        assert_eq!(
            Preferences::default().app_version(),
            rusty_feed_core::VERSION
        );
        assert!(include_str!("../templates/app_base.html").contains("{{ ui.app_version() }}"));
    }

    #[test]
    fn optional_percentage_accepts_empty_form_values() {
        let empty = empty_string_as_none(StrDeserializer::<ValueError>::new("")).unwrap();
        let value = empty_string_as_none(StrDeserializer::<ValueError>::new("35")).unwrap();
        assert_eq!(empty, None);
        assert_eq!(value, Some(35));
        assert!(empty_string_as_none(StrDeserializer::<ValueError>::new("invalid")).is_err());
    }

    #[test]
    fn normalizes_shortcuts_and_removes_conflicts() {
        assert_eq!(normalize_shortcut(" shift + J "), "Shift+j");
        assert_eq!(normalize_shortcut("ArrowDown"), "ArrowDown");
        assert_eq!(normalize_shortcut("Ctrl+j"), "");
        assert_eq!(normalize_shortcut("not-a-key"), "");

        let shortcuts = KeyboardShortcuts {
            next_article: "j".to_string(),
            previous_article: "J".to_string(),
            ..KeyboardShortcuts::default()
        }
        .sanitized();
        assert_eq!(shortcuts.next_article, "j");
        assert_eq!(shortcuts.previous_article, "");

        let partial: KeyboardShortcuts = serde_json::from_value(serde_json::json!({
            "next_article": "ArrowDown",
        }))
        .unwrap();
        assert!(partial.enabled);
        assert_eq!(partial.next_article, "ArrowDown");
        assert_eq!(partial.previous_article, "ArrowUp");
    }

    #[test]
    fn sanitizes_attachment_player_preferences() {
        let preferences = Preferences {
            enclosure_load_mode: "autoplay".to_string(),
            enclosure_delivery_mode: "unsafe".to_string(),
            enclosure_player_kinds: vec![
                "video".to_string(),
                "script".to_string(),
                "audio".to_string(),
                "video".to_string(),
            ],
            ..Preferences::default()
        }
        .sanitized();
        assert_eq!(preferences.enclosure_load_mode, "on-demand");
        assert_eq!(preferences.enclosure_delivery_mode, "proxy");
        assert_eq!(
            preferences.enclosure_player_kinds,
            vec!["audio".to_string(), "video".to_string()]
        );
    }

    #[test]
    fn accepts_rusty_theme_and_rejects_unknown_themes() {
        let rusty = Preferences {
            theme: "rusty".to_string(),
            ..Preferences::default()
        }
        .sanitized();
        assert_eq!(rusty.theme, "rusty");

        let unknown = Preferences {
            theme: "remote-stylesheet".to_string(),
            ..Preferences::default()
        }
        .sanitized();
        assert_eq!(unknown.theme, "system");
    }

    #[test]
    fn sanitizes_font_family() {
        let preferences = Preferences {
            font_family: "remote-font-url".to_string(),
            ..Preferences::default()
        }
        .sanitized();
        assert_eq!(preferences.font_family, "sans");
    }

    #[test]
    fn sanitizes_youtube_player_preferences() {
        let preferences = Preferences {
            youtube_player_mode: "standard-tracking-player".to_string(),
            youtube_player_provider: "arbitrary-origin".to_string(),
            youtube_captions: "script".to_string(),
            ..Preferences::default()
        }
        .sanitized();
        assert_eq!(preferences.youtube_player_mode, "privacy");
        assert_eq!(preferences.youtube_player_provider, "privacy");
        assert_eq!(preferences.youtube_captions, "default");
    }

    #[test]
    fn sanitizes_touch_gesture_preferences() {
        let preferences = Preferences {
            touch_gesture_sensitivity: "instant".to_string(),
            touch_gesture_direction: "sideways".to_string(),
            ..Preferences::default()
        }
        .sanitized();
        assert!(!preferences.touch_gestures_enabled);
        assert_eq!(preferences.touch_gesture_sensitivity, "medium");
        assert_eq!(preferences.touch_gesture_direction, "natural");
    }

    #[test]
    fn sanitizes_external_link_preference() {
        let preferences = Preferences {
            external_link_mode: "unsafe".to_string(),
            external_open_mode: "popup-with-opener".to_string(),
            ..Preferences::default()
        }
        .sanitized();
        assert_eq!(preferences.external_link_mode, "clean");
        assert_eq!(preferences.external_open_mode, "new-tab");
    }

    #[test]
    fn sanitizes_search_preferences() {
        let preferences = Preferences {
            search_mode: "magic".to_string(),
            search_text_config: "arbitrary-regconfig".to_string(),
            search_semantic_weight_percent: Some(101),
            ..Preferences::default()
        }
        .sanitized();
        assert_eq!(preferences.search_mode, "instance");
        assert_eq!(preferences.search_text_config, "instance");
        assert_eq!(preferences.search_semantic_weight_percent, None);
    }
}
