//! The single web error type and its HTTP mapping.
//!
//! Handlers return `Result<_, AppError>`; `AppError` implements `IntoResponse`,
//! producing the standard error envelope `{ "error": ..., "details"?: ... }`
//! with the right status code and no leakage of internal details
//! (`docs/ARCHITECTURE.md` §19).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
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
}
