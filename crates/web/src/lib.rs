//! # rustss-web
//!
//! The Axum web layer: routers, handlers, Askama/HTMX templates, auth, and the
//! JSON API (native + a GReader-compatible adapter). See `docs/ARCHITECTURE.md`
//! §9 and §9.1.
//!
//! So far this exposes the health + metrics surface (milestones **M0/M1**); the
//! reading UI, auth, and feature APIs arrive in later milestones per
//! `docs/DEVELOPMENT_PLAN.md`.

mod error;
mod state;

pub use error::{AppError, Result};
pub use state::AppState;

use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::{routing::get, Json, Router};
use serde_json::json;

/// Build the base application router.
///
/// Currently exposes health endpoints (PRD FR-API-3) and Prometheus `/metrics`.
/// As milestones land, feature routers (auth, reading UI, feeds, search, admin,
/// GReader) are merged in here.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/health/ready", get(ready))
        .route("/metrics", get(metrics))
        .with_state(state)
}

/// Liveness probe: the process is up. Always `200`.
async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "data": {
            "status": "ok",
            "service": rustss_core::APP_NAME,
            "version": rustss_core::VERSION,
        }
    }))
}

/// Readiness probe. Once dependencies are wired (M2+), this will check the DB
/// (required) and report optional dependencies (Redis/LLM/Chromium) as degraded
/// rather than failing. For now it mirrors liveness.
async fn ready() -> Json<serde_json::Value> {
    Json(json!({ "data": { "status": "ready" } }))
}

/// Prometheus metrics exposition endpoint.
async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.render_metrics(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt; // for `oneshot`

    #[tokio::test]
    async fn health_returns_ok_envelope() {
        let app = router(AppState::default());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::OK);
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["data"]["status"], "ok");
        assert_eq!(v["data"]["service"], rustss_core::APP_NAME);
    }

    #[tokio::test]
    async fn metrics_endpoint_responds() {
        let app = router(AppState::new(|| "# rustss metrics\n".to_string()));
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&bytes).contains("rustss metrics"));
    }
}
