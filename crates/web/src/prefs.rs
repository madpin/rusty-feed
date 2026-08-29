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

use rustss_db::{user_preferences, Db};

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
    pub reader_panel_width: u8,
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
            reader_panel_width: 56,
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
    prefs_theme: String,
    prefs_per_page: u32,
    prefs_show_read: bool,
    prefs_reading_mode: String,
    prefs_auto_mark_read: bool,
    prefs_density: String,
    prefs_reader_font_size: String,
    prefs_reader_line_height: String,
    prefs_sidebar_width: u8,
    prefs_reader_panel_width: u8,
    saved: bool,
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
    reader_panel_width: Option<u8>,
}

#[derive(Deserialize)]
pub struct LayoutForm {
    csrf_token: String,
    #[serde(default)]
    sidebar_width: Option<u8>,
    #[serde(default)]
    reader_panel_width: Option<u8>,
}

fn render(tpl: &PreferencesTemplate) -> Result<Html<String>, AppError> {
    tpl.render()
        .map(Html)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))
}

/// `GET /settings/preferences`.
pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let prefs = load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = render(&PreferencesTemplate {
        theme: prefs.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui: prefs.clone(),
        prefs_theme: prefs.theme,
        prefs_per_page: prefs.articles_per_page,
        prefs_show_read: prefs.show_read_articles,
        prefs_reading_mode: prefs.reading_mode,
        prefs_auto_mark_read: prefs.auto_mark_read,
        prefs_density: prefs.article_density,
        prefs_reader_font_size: prefs.reader_font_size,
        prefs_reader_line_height: prefs.reader_line_height,
        prefs_sidebar_width: prefs.sidebar_width,
        prefs_reader_panel_width: prefs.reader_panel_width,
        saved: false,
    })?;
    Ok((jar, body).into_response())
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
        reader_panel_width: form
            .reader_panel_width
            .unwrap_or(current.reader_panel_width),
    }
    .sanitized();

    let value = serde_json::to_value(&prefs)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("serialize prefs: {e}")))?;
    user_preferences::upsert(state.db.pool(), user.id, &value).await?;

    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = render(&PreferencesTemplate {
        theme: prefs.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui: prefs.clone(),
        prefs_theme: prefs.theme,
        prefs_per_page: prefs.articles_per_page,
        prefs_show_read: prefs.show_read_articles,
        prefs_reading_mode: prefs.reading_mode,
        prefs_auto_mark_read: prefs.auto_mark_read,
        prefs_density: prefs.article_density,
        prefs_reader_font_size: prefs.reader_font_size,
        prefs_reader_line_height: prefs.reader_line_height,
        prefs_sidebar_width: prefs.sidebar_width,
        prefs_reader_panel_width: prefs.reader_panel_width,
        saved: true,
    })?;
    Ok((jar, body).into_response())
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
    if let Some(width) = form.reader_panel_width {
        prefs.reader_panel_width = width;
    }
    let prefs = prefs.sanitized();
    let patch = serde_json::json!({
        "sidebar_width": prefs.sidebar_width,
        "reader_panel_width": prefs.reader_panel_width,
    });
    user_preferences::upsert(state.db.pool(), user.id, &patch).await?;
    if is_ajax {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Ok((jar, Redirect::to("/settings/preferences")).into_response())
    }
}
