//! User preferences: a typed view over the `user_preferences` JSONB document,
//! plus the settings page handlers. (A small subset for the M5 start; more
//! fields are added alongside the reading UI.)

use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rustss_db::{user_preferences, Db};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

/// Typed user preferences. `#[serde(default)]` makes missing fields fall back to
/// defaults, so the stored document can evolve without breaking older rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub theme: String,
    pub articles_per_page: u32,
    pub show_read_articles: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: "system".to_string(),
            articles_per_page: 20,
            show_read_articles: true,
        }
    }
}

impl Preferences {
    fn sanitized(mut self) -> Self {
        if !matches!(self.theme.as_str(), "system" | "light" | "dark") {
            self.theme = "system".to_string();
        }
        self.articles_per_page = self.articles_per_page.clamp(5, 100);
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
    prefs_theme: String,
    prefs_per_page: u32,
    prefs_show_read: bool,
    saved: bool,
}

#[derive(Deserialize)]
pub struct PrefsForm {
    csrf_token: String,
    theme: String,
    articles_per_page: u32,
    #[serde(default)]
    show_read_articles: Option<String>,
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
        prefs_theme: prefs.theme,
        prefs_per_page: prefs.articles_per_page,
        prefs_show_read: prefs.show_read_articles,
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
    let prefs = Preferences {
        theme: form.theme,
        articles_per_page: form.articles_per_page,
        show_read_articles: form.show_read_articles.is_some(),
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
        prefs_theme: prefs.theme,
        prefs_per_page: prefs.articles_per_page,
        prefs_show_read: prefs.show_read_articles,
        saved: true,
    })?;
    Ok((jar, body).into_response())
}
