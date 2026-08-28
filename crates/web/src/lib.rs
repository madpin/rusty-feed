//! # rustss-web
//!
//! The Axum web layer: routers, handlers, Askama/HTMX templates, auth, and the
//! JSON API (native + a GReader-compatible adapter). See `docs/ARCHITECTURE.md`
//! §9 and §9.1.
//!
//! Only the health surface exists so far (milestone **M0/M1**); the reading UI,
//! auth, and API arrive in later milestones per `docs/DEVELOPMENT_PLAN.md`.

use axum::{routing::get, Json, Router};
use serde_json::json;

/// Build the base application router.
///
/// For now this only exposes health endpoints (PRD FR-API-3). As milestones
/// land, feature routers (auth, reading UI, feeds, search, admin, GReader) are
/// merged in here.
pub fn router() -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/health/ready", get(ready))
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt; // for `oneshot`

    #[tokio::test]
    async fn health_returns_ok_envelope() {
        let app = router();
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
}
