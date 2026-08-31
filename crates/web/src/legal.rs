//! Public legal/compliance pages: Terms, Acceptable Use, Privacy, Abuse/DMCA
//! (docs/PUBLIC_LAUNCH_PLAN.md §9, FND-195). Reachable pre-signup; content is a
//! sensible default operators should review for their jurisdiction.

use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Response};

use crate::error::AppError;
use crate::state::AppState;

#[derive(Template)]
#[template(path = "legal.html")]
struct LegalTemplate {
    kind: &'static str,
    heading: &'static str,
    entity_name: String,
    contact_email: String,
    has_contact: bool,
}

fn page(state: &AppState, kind: &'static str, heading: &'static str) -> Result<Response, AppError> {
    let contact_email = state
        .settings
        .legal_contact_email
        .clone()
        .unwrap_or_else(|| "the site administrator".to_string());
    let body = LegalTemplate {
        kind,
        heading,
        entity_name: state.settings.legal_entity_name.clone(),
        has_contact: state.settings.legal_contact_email.is_some(),
        contact_email,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok(Html(body).into_response())
}

/// `GET /terms`
pub async fn terms(State(state): State<AppState>) -> Result<Response, AppError> {
    page(&state, "terms", "Terms of Service")
}

/// `GET /acceptable-use`
pub async fn acceptable_use(State(state): State<AppState>) -> Result<Response, AppError> {
    page(&state, "acceptable_use", "Acceptable Use Policy")
}

/// `GET /privacy`
pub async fn privacy(State(state): State<AppState>) -> Result<Response, AppError> {
    page(&state, "privacy", "Privacy Policy")
}

/// `GET /abuse`
pub async fn abuse(State(state): State<AppState>) -> Result<Response, AppError> {
    page(&state, "abuse", "Report Abuse / DMCA")
}
