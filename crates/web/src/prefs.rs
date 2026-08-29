//! User preferences: a typed view over the `user_preferences` JSONB document,
//! plus the settings page handlers. (A small subset for the M5 start; more
//! fields are added alongside the reading UI.)

use askama::Template;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rustss_db::{user_preferences, Db, User};

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
    rustss_search::personalize::reset_learning(&state.db, &state.cache, user.id).await?;
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
    pub theme: String,
    pub articles_per_page: u32,
    pub show_read_articles: bool,
    /// `side-panel`, `standalone`, or `inline`.
    pub reading_mode: String,
    /// Mark an article read automatically when opened.
    pub auto_mark_read: bool,
    pub article_density: String,
    pub reader_font_size: String,
    pub reader_line_height: String,
    pub sidebar_width: u8,
    pub sidebar_collapsed: bool,
    pub reader_panel_width: u8,
    pub shortcuts: KeyboardShortcuts,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: "system".to_string(),
            articles_per_page: 20,
            show_read_articles: true,
            reading_mode: "side-panel".to_string(),
            auto_mark_read: true,
            article_density: "comfortable".to_string(),
            reader_font_size: "medium".to_string(),
            reader_line_height: "relaxed".to_string(),
            sidebar_width: 17,
            sidebar_collapsed: false,
            reader_panel_width: 56,
            shortcuts: KeyboardShortcuts::default(),
        }
    }
}

impl Preferences {
    fn sanitized(mut self) -> Self {
        if !matches!(
            self.theme.as_str(),
            "system" | "light" | "dark" | "ocean" | "forest" | "plum" | "sunset"
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
        if !matches!(self.reader_font_size.as_str(), "small" | "medium" | "large") {
            self.reader_font_size = "medium".to_string();
        }
        if !matches!(
            self.reader_line_height.as_str(),
            "compact" | "relaxed" | "airy"
        ) {
            self.reader_line_height = "relaxed".to_string();
        }
        self.articles_per_page = self.articles_per_page.clamp(5, 100);
        self.sidebar_width = self.sidebar_width.clamp(13, 24);
        self.reader_panel_width = self.reader_panel_width.clamp(38, 68);
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
    prefs_theme: String,
    prefs_per_page: u32,
    prefs_show_read: bool,
    prefs_reading_mode: String,
    prefs_auto_mark_read: bool,
    prefs_density: String,
    prefs_reader_font_size: String,
    prefs_reader_line_height: String,
    prefs_sidebar_width: u8,
    prefs_sidebar_collapsed: bool,
    prefs_reader_panel_width: u8,
    saved: bool,
    error: Option<String>,
}

#[derive(Deserialize)]
pub struct PrefsForm {
    csrf_token: String,
    theme: String,
    articles_per_page: u32,
    reading_mode: String,
    #[serde(default)]
    show_read_articles: Option<String>,
    #[serde(default)]
    auto_mark_read: Option<String>,
    #[serde(default)]
    article_density: Option<String>,
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
    reader_panel_width: Option<u8>,
}

fn render(tpl: &PreferencesTemplate) -> Result<Html<String>, AppError> {
    tpl.render()
        .map(Html)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))
}

fn preferences_template(
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
        prefs_theme: prefs.theme,
        prefs_per_page: prefs.articles_per_page,
        prefs_show_read: prefs.show_read_articles,
        prefs_reading_mode: prefs.reading_mode,
        prefs_auto_mark_read: prefs.auto_mark_read,
        prefs_density: prefs.article_density,
        prefs_reader_font_size: prefs.reader_font_size,
        prefs_reader_line_height: prefs.reader_line_height,
        prefs_sidebar_width: prefs.sidebar_width,
        prefs_sidebar_collapsed: prefs.sidebar_collapsed,
        prefs_reader_panel_width: prefs.reader_panel_width,
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
        render(&preferences_template(user, csrf_token, prefs, saved, error))?,
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
        theme: form.theme,
        articles_per_page: form.articles_per_page,
        show_read_articles: form.show_read_articles.is_some(),
        reading_mode: form.reading_mode,
        auto_mark_read: form.auto_mark_read.is_some(),
        article_density: form.article_density.unwrap_or(current.article_density),
        reader_font_size: form.reader_font_size.unwrap_or(current.reader_font_size),
        reader_line_height: form
            .reader_line_height
            .unwrap_or(current.reader_line_height),
        sidebar_width: form.sidebar_width.unwrap_or(current.sidebar_width),
        sidebar_collapsed: form.sidebar_collapsed.is_some(),
        reader_panel_width: form
            .reader_panel_width
            .unwrap_or(current.reader_panel_width),
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
    let mut prefs = load(&state.db, user.id).await;
    if let Some(width) = form.sidebar_width {
        prefs.sidebar_width = width;
    }
    if let Some(collapsed) = form.sidebar_collapsed.as_deref() {
        prefs.sidebar_collapsed = match collapsed {
            "true" => true,
            "false" => false,
            _ => return Err(AppError::Validation("Invalid sidebar state.".to_string())),
        };
    }
    if let Some(width) = form.reader_panel_width {
        prefs.reader_panel_width = width;
    }
    let prefs = prefs.sanitized();
    let patch = serde_json::json!({
        "sidebar_width": prefs.sidebar_width,
        "sidebar_collapsed": prefs.sidebar_collapsed,
        "reader_panel_width": prefs.reader_panel_width,
    });
    user_preferences::upsert(state.db.pool(), user.id, &patch).await?;
    if is_ajax {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Ok((jar, Redirect::to("/settings/preferences")).into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
