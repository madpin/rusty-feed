//! # rustss-web
//!
//! The Axum web layer: routers, handlers, Askama/HTMX templates, auth, and the
//! JSON API (native + a GReader-compatible adapter). See `docs/ARCHITECTURE.md`
//! §9 and §9.1.
//!
//! So far this exposes: the home page, local-account auth (login/register/
//! logout) with DB-backed sessions, and health/metrics. The reading UI and
//! feature APIs arrive in later milestones per `docs/DEVELOPMENT_PLAN.md`.

pub mod auth;
mod error;
pub mod extract;
mod feeds;
pub mod oauth;
mod prefs;
pub mod ratelimit;
mod reader;
mod saved;
mod search;
mod state;
mod tokens;

pub use error::{AppError, Result};
pub use state::{AppSettings, AppState, OAuthProvider, ProviderKind};

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rust_embed::RustEmbed;
use serde_json::json;

/// Embedded static assets (CSS/JS), served under `/static`.
#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

/// Build the application router with all current routes.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(auth::handlers::home))
        .route(
            "/login",
            get(auth::handlers::login_page).post(auth::handlers::login_submit),
        )
        .route(
            "/register",
            get(auth::handlers::register_page).post(auth::handlers::register_submit),
        )
        .route("/logout", post(auth::handlers::logout_submit))
        .route("/auth/oauth/{provider}", get(oauth::start))
        .route("/auth/oauth/{provider}/callback", get(oauth::callback))
        .route("/articles", get(reader::list))
        .route("/articles/read-all", post(reader::mark_all_read))
        .route("/articles/{id}", get(reader::read))
        .route("/articles/{id}/content", get(reader::content_fragment))
        .route("/articles/{id}/read", post(reader::mark_read))
        .route("/articles/{id}/unread", post(reader::mark_unread))
        .route("/articles/{id}/bookmark", post(reader::bookmark))
        .route("/articles/{id}/unbookmark", post(reader::unbookmark))
        .route("/articles/{id}/feedback", post(reader::feedback))
        .route("/feeds", get(feeds::page).post(feeds::add))
        .route("/feeds/refresh-all", post(feeds::refresh_all))
        .route("/feeds/{id}/refresh", post(feeds::refresh))
        .route("/feeds/{id}/unsubscribe", post(feeds::unsubscribe))
        .route("/search", get(search::page))
        .route("/searches", get(saved::page).post(saved::create))
        .route("/searches/{id}", get(saved::results))
        .route("/searches/{id}/run", post(saved::run))
        .route("/searches/{id}/delete", post(saved::delete))
        .route("/notifications", get(saved::notifications_page))
        .route("/notifications/read", post(saved::mark_read))
        .route("/settings/preferences", get(prefs::page).post(prefs::save))
        .route("/settings/reset-learning", post(prefs::reset_learning))
        .route("/settings/tokens", get(tokens::list).post(tokens::create))
        .route("/settings/tokens/{id}/revoke", post(tokens::revoke))
        .route("/static/{*path}", get(static_handler))
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

/// Readiness probe: checks the database (required). Optional dependencies
/// (Redis) are reported but do not fail readiness.
async fn ready(State(state): State<AppState>) -> Response {
    let db_ok = state.db.ping().await.is_ok();
    let status = if db_ok {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    let body = Json(json!({
        "data": {
            "status": if db_ok { "ready" } else { "degraded" },
            "database": if db_ok { "ok" } else { "unavailable" },
            "cache": if state.cache.l2_connected() { "connected" } else { "l1-only" },
        }
    }));
    (status, body).into_response()
}

/// Prometheus metrics exposition endpoint.
async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.render_metrics(),
    )
}

/// Serve an embedded static asset under `/static/{*path}`.
async fn static_handler(Path(path): Path<String>) -> Response {
    match Assets::get(&path) {
        Some(content) => (
            [(header::CONTENT_TYPE, content_type(&path))],
            content.data.into_owned(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustss_cache::Cache;
    use rustss_db::Db;

    async fn test_state() -> AppState {
        // A lazily-connected pool: fine for handlers that don't touch the DB.
        let db = Db::connect_lazy("postgres://user:pass@localhost/rustss_test").unwrap();
        let cache = Cache::in_memory().await;
        AppState::new(db, cache, AppSettings::default(), || {
            "# rustss metrics\n".to_string()
        })
    }

    #[tokio::test]
    async fn health_returns_ok_envelope() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let app = router(test_state().await);
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
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let app = router(test_state().await);
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
