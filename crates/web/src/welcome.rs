//! First-run setup wizard (docs/PUBLIC_LAUNCH_PLAN.md §8.4). New users pick a
//! layout, theme, and typography with a live preview (CSP-safe: the embedded
//! app.js swaps data-attributes, no inline styles). Skippable and re-runnable.

use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use serde_json::{json, Value};

use rustss_db::{user_preferences, users};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

#[derive(Template)]
#[template(path = "welcome.html")]
struct WelcomeTemplate {
    theme: String,
    user_email: String,
    ui: prefs::Preferences,
    is_admin: bool,
    csrf_token: String,
}

/// `GET /welcome` — the wizard (also reachable via "Re-run setup").
pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = WelcomeTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        ui,
        is_admin: user.role == "admin",
        csrf_token,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, Html(body)).into_response())
}

#[derive(Deserialize)]
pub struct WelcomeForm {
    csrf_token: String,
    #[serde(default)]
    skip: String,
    #[serde(default)]
    theme: String,
    #[serde(default)]
    reading_mode: String,
    #[serde(default)]
    font_family: String,
    #[serde(default)]
    article_density: String,
}

/// `POST /welcome` — persist selections (unless skipped) and stamp `onboarded_at`.
pub async fn submit(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<WelcomeForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let skipping = matches!(form.skip.trim(), "1" | "true" | "on");
    if !skipping {
        let mut patch = serde_json::Map::new();
        if matches!(
            form.theme.as_str(),
            "system" | "light" | "dark" | "ocean" | "forest" | "plum" | "sunset"
        ) {
            patch.insert("theme".to_string(), json!(form.theme));
        }
        if matches!(
            form.reading_mode.as_str(),
            "side-panel" | "standalone" | "inline"
        ) {
            patch.insert("reading_mode".to_string(), json!(form.reading_mode));
        }
        if matches!(form.font_family.as_str(), "sans" | "serif") {
            patch.insert("font_family".to_string(), json!(form.font_family));
        }
        if matches!(
            form.article_density.as_str(),
            "compact" | "comfortable" | "spacious"
        ) {
            patch.insert("article_density".to_string(), json!(form.article_density));
        }
        if !patch.is_empty() {
            user_preferences::upsert(state.db.pool(), user.id, &Value::Object(patch)).await?;
        }
    }
    users::mark_onboarded(state.db.pool(), user.id).await?;
    Ok((jar, Redirect::to("/articles")).into_response())
}
