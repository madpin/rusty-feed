//! The single web error type and its HTTP mapping.
//!
//! Handlers return `Result<_, AppError>`; `AppError` implements `IntoResponse`,
//! producing the standard error envelope `{ "error": ..., "details"?: ... }`
//! with the right status code and no leakage of internal details
//! (`docs/ARCHITECTURE.md` §19).

use askama::Template;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Convenience alias for handler results.
pub type Result<T> = std::result::Result<T, AppError>;

/// All error shapes the web layer can return. Domain errors get `From` impls as
/// they are introduced (M2+); anything unexpected becomes `Internal`.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// 404 — resource does not exist (or is not visible to the caller).
    #[error("resource not found")]
    NotFound,

    /// 401 — authentication required or invalid.
    #[error("authentication required")]
    Unauthorized,

    /// 403 — authenticated but not permitted.
    #[error("forbidden")]
    Forbidden,

    /// 400 — request failed validation. Message is safe to show the client.
    #[error("{0}")]
    Validation(String),

    /// 409 — conflict (e.g. duplicate). Message is safe to show the client.
    #[error("{0}")]
    Conflict(String),

    /// 409 — a per-user quota was exceeded. Message is safe to show the client.
    #[error("{0}")]
    QuotaExceeded(String),

    /// 429 — too many requests.
    #[error("too many requests")]
    RateLimited,

    /// 500 — an unexpected error; details are logged, never returned.
    #[error("internal server error")]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::Validation(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::QuotaExceeded(_) => StatusCode::CONFLICT,
            AppError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The client-safe message (internal errors are intentionally generic).
    fn client_message(&self) -> String {
        match self {
            AppError::Internal(_) => "internal server error".to_string(),
            other => other.to_string(),
        }
    }
}

impl From<rusty_feed_db::DbError> for AppError {
    fn from(e: rusty_feed_db::DbError) -> Self {
        AppError::Internal(anyhow::Error::new(e))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        // Log server-side faults with their full context; never send it out.
        if status.is_server_error() {
            tracing::error!(error = ?self, "request failed");
        }
        (status, Json(json!({ "error": self.client_message() }))).into_response()
    }
}

/// A human-friendly HTML error page for browser navigations.
#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPageTemplate {
    status: u16,
    title: &'static str,
    message: &'static str,
}

/// Render a friendly HTML page for a failed browser navigation.
///
/// API and AJAX clients keep receiving the canonical JSON envelope from
/// [`AppError::into_response`]. The router's `render_browser_errors` middleware
/// calls this only for genuine document requests (those sending
/// `Accept: text/html`), so the JSON contract stays the single source of truth
/// and the HTML is layered on top for presentation (`docs/ARCHITECTURE.md` §19).
pub(crate) fn browser_error_page(status: StatusCode) -> Response {
    let (title, message): (&str, &str) = match status {
        StatusCode::BAD_REQUEST => ("Bad request", "We couldn't process that request."),
        StatusCode::FORBIDDEN => (
            "Access denied",
            "You don't have permission to view this page.",
        ),
        StatusCode::NOT_FOUND => (
            "Page not found",
            "We couldn't find the page you were looking for.",
        ),
        StatusCode::CONFLICT => (
            "Conflict",
            "That request conflicts with the current state. Please refresh and try again.",
        ),
        StatusCode::TOO_MANY_REQUESTS => (
            "Slow down",
            "You've made too many requests. Please wait a moment and try again.",
        ),
        status if status.is_server_error() => (
            "Something went wrong",
            "An unexpected error occurred on our end. Please try again shortly.",
        ),
        _ => (
            "Something went wrong",
            "The request could not be completed.",
        ),
    };
    let page = ErrorPageTemplate {
        status: status.as_u16(),
        title,
        message,
    };
    match page.render() {
        Ok(html) => (status, Html(html)).into_response(),
        // Fall back to the JSON envelope if the page itself fails to render.
        Err(error) => {
            tracing::error!(?error, "error page render failed");
            (status, Json(json!({ "error": "internal server error" }))).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_map_correctly() {
        assert_eq!(AppError::NotFound.status(), StatusCode::NOT_FOUND);
        assert_eq!(AppError::Unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(AppError::Forbidden.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            AppError::Validation("x".into()).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AppError::Conflict("x".into()).status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            AppError::QuotaExceeded("x".into()).status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            AppError::RateLimited.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            AppError::Internal(anyhow::anyhow!("boom")).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn internal_message_is_not_leaked() {
        let e = AppError::Internal(anyhow::anyhow!("secret db url in error"));
        assert_eq!(e.client_message(), "internal server error");
    }

    #[test]
    fn browser_error_page_is_html_with_the_right_status() {
        let response = browser_error_page(StatusCode::NOT_FOUND);
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(content_type.contains("text/html"));
    }

    #[test]
    fn browser_error_page_copy_is_friendly_and_status_specific() {
        // Render the template directly to assert on the human-facing copy.
        let not_found = ErrorPageTemplate {
            status: 404,
            title: "Page not found",
            message: "We couldn't find the page you were looking for.",
        }
        .render()
        .unwrap();
        assert!(not_found.contains("Page not found"));
        assert!(not_found.contains("Error 404"));
        // No raw JSON envelope leaks into the browser page.
        assert!(!not_found.contains("\"error\""));
    }
}
