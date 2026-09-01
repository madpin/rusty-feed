//! # rusty-feed-web
//!
//! The Axum web layer: routers, handlers, Askama/HTMX templates, auth, and the
//! JSON API (native + a GReader-compatible adapter). See `docs/ARCHITECTURE.md`
//! §9 and §9.1.
//!
//! So far this exposes: the home page, local-account auth (login/register/
//! logout) with DB-backed sessions, and health/metrics. The reading UI and
//! feature APIs arrive in later milestones per `docs/DEVELOPMENT_PLAN.md`.

mod account;
mod acquisition;
mod admin;
mod ai_settings;
mod api;
pub mod auth;
mod captcha;
mod catalog;
mod catalog_view;
mod customization;
mod error;
pub mod extract;
mod feeds;
mod fever;
mod greader;
mod integrations;
mod legal;
mod live_view;
pub mod mailer;
mod media_proxy;
mod moderation;
pub mod oauth;
mod opml;
mod passkeys;
mod prefs;
mod public_shares;
mod quota;
pub mod ratelimit;
mod reader;
mod saved;
mod search;
mod sessions;
mod state;
mod tokens;
mod verify;
mod welcome;

pub use error::{AppError, Result};
pub use state::{
    AppSettings, AppState, CaptchaSettings, EnvLlmProvider, OAuthProvider, ProviderKind,
    RateLimitSettings, ReverseProxyAuthSettings,
};

use std::time::Instant;

use axum::extract::{DefaultBodyLimit, MatchedPath, Request};
use axum::extract::{Path, State};
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rust_embed::RustEmbed;
use serde_json::json;
use tower_http::trace::TraceLayer;

/// Embedded static assets (CSS/JS), served under `/static`.
#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

/// Build the application router with all current routes.
pub fn router(state: AppState) -> Router {
    let content_security_policy = reader_content_security_policy(
        &state.settings.youtube_policy,
        state.settings.captcha.is_some(),
    );
    let api_policy_state = state.clone();
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
        .route("/demo/start", post(auth::handlers::demo_start))
        .route("/terms", get(legal::terms))
        .route("/acceptable-use", get(legal::acceptable_use))
        .route("/privacy", get(legal::privacy))
        .route("/abuse", get(legal::abuse))
        .route("/verify", get(verify::verify))
        .route("/verify/resend", post(verify::resend))
        .route("/auth/passkey/start", post(passkeys::authentication_start))
        .route(
            "/auth/passkey/finish",
            post(passkeys::authentication_finish),
        )
        .route("/auth/oauth/{provider}", get(oauth::start))
        .route("/auth/oauth/{provider}/callback", get(oauth::callback))
        .route("/articles", get(reader::list))
        .route("/articles/read-all", post(reader::mark_all_read))
        .route("/articles/{id}", get(reader::read))
        .route("/articles/{id}/panel", get(reader::side_panel))
        .route("/articles/{id}/content", get(reader::content_fragment))
        .route("/articles/{id}/live-ticket", get(live_view::ticket))
        .route("/articles/{id}/live", get(live_view::page))
        .route("/articles/{id}/live/resource", get(live_view::resource))
        .route("/articles/{id}/hero", get(media_proxy::hero))
        .route("/articles/{id}/media/{position}", get(media_proxy::get))
        .route("/articles/{id}/read", post(reader::mark_read))
        .route("/articles/{id}/unread", post(reader::mark_unread))
        .route("/articles/{id}/bookmark", post(reader::bookmark))
        .route("/articles/{id}/unbookmark", post(reader::unbookmark))
        .route("/articles/{id}/feedback", post(reader::feedback))
        .route("/articles/{id}/summarize", post(reader::summarize))
        .route("/articles/{id}/summary/status", get(reader::summary_status))
        .route("/articles/{id}/report", post(moderation::report_article))
        .route(
            "/articles/{id}/send",
            get(integrations::article_page).post(integrations::article_send),
        )
        .route(
            "/articles/{id}/send/modal",
            get(integrations::article_modal),
        )
        .route("/articles/{id}/share", post(public_shares::create))
        .route("/articles/{id}/share/revoke", post(public_shares::revoke))
        .route("/shared/{token}/{slug}", get(public_shares::show))
        .route("/feeds", get(feeds::page).post(feeds::add))
        .route("/bookmarklet/subscribe", get(feeds::bookmarklet_subscribe))
        .route(
            "/feeds/opml/preview",
            post(opml::preview).layer(DefaultBodyLimit::max(opml::PREVIEW_BODY_LIMIT)),
        )
        .route(
            "/feeds/opml/import",
            post(opml::import).layer(DefaultBodyLimit::max(opml::IMPORT_BODY_LIMIT)),
        )
        .route("/feeds/opml/export", get(opml::export))
        .route("/feeds/refresh-all", post(feeds::refresh_all))
        .route("/feeds/refresh-everything", post(feeds::refresh_everything))
        .route("/feeds/refresh-selected", post(feeds::refresh_selected))
        .route("/feeds/bulk-move", post(feeds::bulk_move))
        .route("/feeds/bulk-tags", post(feeds::bulk_tags))
        .route("/feeds/bulk-content", post(feeds::bulk_content))
        .route("/feeds/bulk-unsubscribe", post(feeds::bulk_unsubscribe))
        .route("/feeds/categories", post(feeds::create_category))
        .route(
            "/feeds/categories/{id}/collapse",
            post(feeds::set_category_collapsed),
        )
        .route("/feeds/{id}/icon", get(feeds::icon))
        .route("/feeds/{id}/folder", post(feeds::move_to_folder))
        .route("/feeds/{id}/organization", post(feeds::update_organization))
        .route(
            "/feeds/{id}/content-settings",
            post(feeds::update_personal_content_settings),
        )
        .route(
            "/feeds/{id}/settings",
            get(feeds::settings_page).post(feeds::update_settings),
        )
        .route("/feeds/{id}/rules/preview", post(feeds::preview_rules))
        .route(
            "/feeds/{feed_id}/rules/{version_id}/restore",
            post(feeds::restore_rewrite_rules),
        )
        .route("/feeds/{id}/refresh", post(feeds::refresh))
        .route("/feeds/{id}/unsubscribe", post(feeds::unsubscribe))
        .route("/feeds/{id}/report", post(moderation::report_feed))
        .route("/catalog", get(catalog::page))
        .route("/catalog/{feed_id}/preview", get(catalog::preview))
        .route(
            "/catalog/{feed_id}/preview/analyze",
            post(catalog::analyze_preview),
        )
        .route("/catalog/subscribe", post(catalog::subscribe))
        .route("/search", get(search::page))
        .route("/searches", get(saved::page).post(saved::create))
        .route("/searches/{id}", get(saved::results).post(saved::update))
        .route("/searches/{id}/run", post(saved::run))
        .route("/searches/{id}/rematch", post(saved::rematch))
        .route("/searches/{id}/delete", post(saved::delete))
        .route("/notifications", get(saved::notifications_page))
        .route("/notifications/count", get(saved::notification_count))
        .route("/notifications/read", post(saved::mark_read))
        .route("/admin", get(admin::dashboard))
        .route("/admin/users", get(admin::users_page))
        .route("/admin/users/{id}/role", post(admin::update_role))
        .route("/admin/invitations", post(admin::create_invitation))
        .route(
            "/admin/invitations/{id}/revoke",
            post(admin::revoke_invitation),
        )
        .route("/admin/feeds", get(admin::feeds_page))
        .route(
            "/admin/feeds/blocklist",
            get(moderation::blocklist_page).post(moderation::blocklist_add),
        )
        .route(
            "/admin/feeds/blocklist/{id}/delete",
            post(moderation::blocklist_remove),
        )
        .route("/admin/feeds/{id}/block", post(moderation::block_feed))
        .route(
            "/admin/articles/{id}/takedown",
            post(moderation::takedown_article),
        )
        .route(
            "/admin/abuse/{id}/resolve",
            post(moderation::resolve_report),
        )
        .route(
            "/admin/acquisition",
            get(acquisition::page).post(acquisition::create),
        )
        .route("/admin/acquisition/assign", post(acquisition::assign))
        .route("/admin/acquisition/{id}/delete", post(acquisition::delete))
        .route("/admin/feeds/{id}/status", post(admin::set_feed_status))
        .route(
            "/admin/catalog",
            get(admin::catalog_page).post(admin::create_catalog_entry),
        )
        .route("/admin/catalog/{id}", post(admin::update_catalog_entry))
        .route("/admin/activity", get(admin::activity))
        .route("/admin/logs", get(admin::logs))
        .route("/admin/maintenance", post(admin::maintenance))
        .route("/admin/costs", get(admin::costs))
        .route(
            "/admin/ai",
            get(ai_settings::admin_page).post(ai_settings::admin_create),
        )
        .route("/admin/ai/{id}/toggle", post(ai_settings::admin_toggle))
        .route("/admin/ai/{id}/delete", post(ai_settings::admin_delete))
        .route("/settings/preferences", get(prefs::page).post(prefs::save))
        .route("/settings/account", get(account::page))
        .route("/settings/account/export", post(account::export))
        .route("/settings/account/delete", post(account::delete))
        .route("/welcome", get(welcome::page).post(welcome::submit))
        .route("/settings/passkeys", get(passkeys::page))
        .route(
            "/settings/passkeys/register/start",
            post(passkeys::registration_start),
        )
        .route(
            "/settings/passkeys/register/finish",
            post(passkeys::registration_finish),
        )
        .route("/settings/passkeys/{id}/delete", post(passkeys::delete))
        .route(
            "/settings/integrations",
            get(integrations::page).post(integrations::create),
        )
        .route(
            "/settings/integrations/{id}/test",
            post(integrations::test_profile),
        )
        .route(
            "/settings/integrations/{id}/toggle",
            post(integrations::toggle_profile),
        )
        .route(
            "/settings/integrations/{id}/credential",
            post(integrations::rotate_credential),
        )
        .route(
            "/settings/integrations/{id}/delete",
            post(integrations::delete_profile),
        )
        .route(
            "/settings/integrations/deliveries/{id}",
            get(integrations::status),
        )
        .route("/settings/customization", get(customization::page))
        .route("/settings/customization/css", post(customization::save_css))
        .route(
            "/settings/customization/css/reset",
            post(customization::reset_css),
        )
        .route(
            "/settings/customization/css/{id}/restore",
            post(customization::restore_css),
        )
        .route(
            "/settings/customization/javascript",
            post(customization::save_javascript),
        )
        .route(
            "/settings/customization/javascript/reset",
            post(customization::reset_javascript),
        )
        .route(
            "/settings/customization/javascript/{id}/restore",
            post(customization::restore_javascript),
        )
        .route("/assets/personal.css", get(customization::css_asset))
        .route(
            "/assets/instance-customization.js",
            get(customization::javascript_asset),
        )
        .route("/safe-mode", get(customization::enter_safe_mode))
        .route("/safe-mode/leave", get(customization::leave_safe_mode))
        .route(
            "/settings/ai",
            get(ai_settings::user_page).post(ai_settings::user_create),
        )
        .route("/settings/ai/{id}/toggle", post(ai_settings::user_toggle))
        .route("/settings/ai/{id}/delete", post(ai_settings::user_delete))
        .route("/settings/layout", post(prefs::save_layout))
        .route("/settings/reset-learning", post(prefs::reset_learning))
        .route("/settings/tokens", get(tokens::list).post(tokens::create))
        .route("/settings/fever", post(tokens::create_fever))
        .route("/settings/tokens/{id}/revoke", post(tokens::revoke))
        .route("/settings/sessions", get(sessions::list))
        .route("/settings/sessions/{id}/revoke", post(sessions::revoke))
        .route(
            "/settings/sessions/revoke-others",
            post(sessions::revoke_others),
        )
        .route("/manifest.webmanifest", get(web_manifest))
        .route("/service-worker.js", get(service_worker))
        .route("/robots.txt", get(robots_txt))
        .route("/sitemap.xml", get(sitemap_xml))
        .route("/static/{*path}", get(static_handler))
        .route("/api/health", get(health))
        .route("/api/health/ready", get(ready))
        .route("/metrics", get(metrics))
        .route("/accounts/ClientLogin", post(greader::client_login))
        .route("/fever/", post(fever::endpoint))
        .route("/reader/api/0/token", get(greader::edit_token))
        .route("/reader/api/0/user-info", get(greader::user_info))
        .route("/reader/api/0/tag/list", get(greader::tag_list))
        .route(
            "/reader/api/0/subscription/list",
            get(greader::subscription_list),
        )
        .route(
            "/reader/api/0/subscription/edit",
            post(greader::subscription_edit),
        )
        .route("/reader/api/0/edit-tag", post(greader::edit_tag))
        .route(
            "/reader/api/0/stream/items/ids",
            get(greader::stream_item_ids),
        )
        .route(
            "/reader/api/0/stream/items/contents",
            post(greader::stream_item_contents),
        )
        .route(
            "/reader/api/0/stream/contents/{*stream}",
            get(greader::stream_contents),
        )
        .route(
            "/reader/api/0/mark-all-as-read",
            post(greader::mark_all_as_read),
        )
        .route(
            "/reader/api/0/{*path}",
            get(greader::fallback).post(greader::fallback),
        )
        .route("/api/v1/openapi.json", get(api::openapi))
        .route("/api/v1/version", get(api::version))
        .route("/api/v1/user", get(api::user))
        .route("/api/v1/feeds", get(api::feeds_list).post(api::feed_create))
        .route(
            "/api/v1/feeds/{id}",
            axum::routing::delete(api::feed_delete),
        )
        .route("/api/v1/feeds/{id}/refresh", post(api::feed_refresh))
        .route("/api/v1/articles", get(api::articles_list))
        .route("/api/v1/articles/{id}", get(api::article))
        .route("/api/v1/articles/{id}/state", post(api::article_state))
        .route(
            "/api/v1/saved-searches",
            get(api::saved_searches_list).post(api::saved_search_create),
        )
        .route(
            "/api/v1/saved-searches/{id}",
            axum::routing::delete(api::saved_search_delete),
        )
        .route(
            "/api/v1/categories",
            get(api::categories_list).post(api::category_create),
        )
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            api_policy_state,
            enforce_api_policy,
        ))
        .layer(middleware::from_fn(observe_request))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::map_response(move |response| {
            let content_security_policy = content_security_policy.clone();
            async move { enforce_security_headers(response, content_security_policy).await }
        }))
}

async fn enforce_api_policy(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let disabled = (path.starts_with("/api/v1") && !state.settings.api_enabled)
        || ((path == "/accounts/ClientLogin" || path.starts_with("/reader/api/0"))
            && !state.settings.greader_api_enabled)
        || (path.starts_with("/fever/") && !state.settings.fever_api_enabled);
    if disabled {
        StatusCode::NOT_FOUND.into_response()
    } else {
        next.run(request).await
    }
}

/// Record bounded-cardinality request count and latency metrics. Route
/// templates are used instead of raw paths so article IDs and unknown probes
/// never create unbounded Prometheus label sets.
async fn observe_request(request: Request, next: Next) -> Response {
    let method = request.method().as_str().to_owned();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("<unmatched>")
        .to_owned();
    let started = Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16().to_string();

    metrics::counter!(
        "rusty_feed_http_requests_total",
        "method" => method.clone(),
        "route" => route.clone(),
        "status" => status
    )
    .increment(1);
    metrics::histogram!(
        "rusty_feed_http_request_duration_seconds",
        "method" => method,
        "route" => route
    )
    .record(started.elapsed().as_secs_f64());

    response
}

/// Keep the reader's current URL private from every downstream navigation or
/// subresource request, including responses produced by errors and fallbacks.
///
/// When `captcha_enabled`, the Cloudflare Turnstile / hCaptcha origins are added
/// to `script-src`/`frame-src`/`connect-src` so the challenge widget loads
/// without weakening the strict no-inline policy (FND-161, FND-191).
fn reader_content_security_policy(
    policy: &rusty_feed_core::youtube::YouTubePolicy,
    captcha_enabled: bool,
) -> HeaderValue {
    const CAPTCHA_ORIGINS: &str =
        "https://challenges.cloudflare.com https://hcaptcha.com https://*.hcaptcha.com";
    let mut script_src = String::from("'self'");
    let mut frame_src = String::from("'self' https://www.youtube-nocookie.com");
    let mut connect_src = String::from("'self'");
    if let Some(origin) = policy.alternative_origin() {
        frame_src.push(' ');
        frame_src.push_str(&origin);
    }
    if captcha_enabled {
        script_src.push(' ');
        script_src.push_str(CAPTCHA_ORIGINS);
        frame_src.push(' ');
        frame_src.push_str(CAPTCHA_ORIGINS);
        connect_src.push(' ');
        connect_src.push_str(CAPTCHA_ORIGINS);
    }
    let value = format!(
        "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'self'; \
         form-action 'self'; script-src {script_src}; style-src 'self'; connect-src {connect_src}; \
         img-src 'self' data: https:; media-src 'self' data: https:; font-src 'self'; \
         frame-src {frame_src}; worker-src 'self'; manifest-src 'self'"
    );
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("default-src 'self'"))
}

async fn enforce_security_headers(
    mut response: Response,
    content_security_policy: HeaderValue,
) -> Response {
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        HeaderName::from_static("permissions-policy"),
        header::HeaderValue::from_static(
            "camera=(), microphone=(), geolocation=(), payment=(), usb=(), publickey-credentials-create=(self), publickey-credentials-get=(self)",
        ),
    );
    response
        .headers_mut()
        .entry(header::CONTENT_SECURITY_POLICY)
        .or_insert(content_security_policy);
    response
}

/// Liveness probe: the process is up. Always `200`.
async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "data": {
            "status": "ok",
            "service": rusty_feed_core::APP_NAME,
            "version": rusty_feed_core::VERSION,
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

/// Serve the instance-branded install manifest without exposing user state.
async fn web_manifest(State(state): State<AppState>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/manifest+json; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        Json(json!({
            "id": "/",
            "name": state.settings.pwa_name,
            "short_name": state.settings.pwa_short_name,
            "description": "A private, self-hosted RSS reader with shared content and personal reading state.",
            "lang": "en",
            "start_url": "/articles?source=installed-app",
            "scope": "/",
            "display": "standalone",
            "display_override": ["window-controls-overlay", "standalone", "minimal-ui"],
            "theme_color": state.settings.pwa_theme_color,
            "background_color": state.settings.pwa_background_color,
            "categories": ["news", "productivity"],
            "icons": [
                { "src": "/static/icon-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any" },
                { "src": "/static/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any maskable" },
                { "src": "/static/icon.svg", "sizes": "any", "type": "image/svg+xml", "purpose": "any" }
            ],
            "shortcuts": [
                { "name": "Library", "short_name": "Library", "url": "/articles", "icons": [{ "src": "/static/icon-192.png", "sizes": "192x192" }] },
                { "name": "Search articles", "short_name": "Search", "url": "/search", "icons": [{ "src": "/static/icon-192.png", "sizes": "192x192" }] },
                { "name": "Discover feeds", "short_name": "Discover", "url": "/catalog", "icons": [{ "src": "/static/icon-192.png", "sizes": "192x192" }] }
            ]
        })),
    )
        .into_response()
}

/// `GET /robots.txt` — index the public marketing/legal pages, disallow every
/// authenticated app surface (§8.2). Points crawlers at the sitemap.
async fn robots_txt(State(state): State<AppState>) -> Response {
    let base = state.settings.base_url.trim_end_matches('/');
    let body = format!(
        "User-agent: *\n\
         Disallow: /admin\n\
         Disallow: /api\n\
         Disallow: /articles\n\
         Disallow: /feeds\n\
         Disallow: /search\n\
         Disallow: /settings\n\
         Disallow: /catalog\n\
         Disallow: /welcome\n\
         Disallow: /verify\n\
         Disallow: /demo\n\
         Sitemap: {base}/sitemap.xml\n"
    );
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        body,
    )
        .into_response()
}

/// `GET /sitemap.xml` — the public, indexable pages only (§8.2).
async fn sitemap_xml(State(state): State<AppState>) -> Response {
    let base = state.settings.base_url.trim_end_matches('/');
    let paths = [
        "/",
        "/terms",
        "/privacy",
        "/acceptable-use",
        "/abuse",
        "/login",
        "/register",
    ];
    let urls: String = paths
        .iter()
        .map(|path| format!("  <url><loc>{base}{path}</loc></url>\n"))
        .collect();
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n{urls}</urlset>\n"
    );
    (
        [
            (header::CONTENT_TYPE, "application/xml; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        body,
    )
        .into_response()
}

/// Serve the root-scoped worker with update-friendly caching and no user data.
async fn service_worker() -> Response {
    match Assets::get("service-worker.js") {
        Some(content) => (
            [
                (
                    header::CONTENT_TYPE,
                    "application/javascript; charset=utf-8",
                ),
                (header::CACHE_CONTROL, "no-cache"),
                (HeaderName::from_static("service-worker-allowed"), "/"),
            ],
            content.data.into_owned(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
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
    use rusty_feed_cache::Cache;
    use rusty_feed_db::Db;

    async fn test_state() -> AppState {
        // A lazily-connected pool: fine for handlers that don't touch the DB.
        let db = Db::connect_lazy("postgres://user:pass@localhost/rusty_feed_test").unwrap();
        let cache = Cache::in_memory().await;
        AppState::new(db, cache, AppSettings::default(), || {
            "# rusty-feed metrics\n".to_string()
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
        assert_eq!(res.headers()[header::REFERRER_POLICY], "no-referrer");
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["data"]["status"], "ok");
        assert_eq!(v["data"]["service"], rusty_feed_core::APP_NAME);
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
        assert!(String::from_utf8_lossy(&bytes).contains("rusty-feed metrics"));
    }

    #[tokio::test]
    async fn referrer_policy_covers_static_and_not_found_responses() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let app = router(test_state().await);
        for path in ["/static/app.css", "/route-that-does-not-exist"] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.headers()[header::REFERRER_POLICY],
                "no-referrer",
                "missing mandatory privacy header on {path}"
            );
        }
    }

    #[tokio::test]
    async fn install_manifest_is_branded_bounded_and_root_scoped() {
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let settings = AppSettings {
            pwa_name: "My Reader".to_string(),
            pwa_short_name: "Reader".to_string(),
            pwa_theme_color: "#112233".to_string(),
            ..Default::default()
        };
        let app = router(AppState::new(
            Db::connect_lazy("postgres://user:pass@localhost/rusty_feed_test").unwrap(),
            Cache::in_memory().await,
            settings,
            String::new,
        ));
        let response = app
            .oneshot(
                Request::get("/manifest.webmanifest")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/manifest+json; charset=utf-8"
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let manifest: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(manifest["name"], "My Reader");
        assert_eq!(manifest["short_name"], "Reader");
        assert_eq!(manifest["theme_color"], "#112233");
        assert_eq!(manifest["scope"], "/");
        assert_eq!(manifest["display"], "standalone");
        assert_eq!(manifest["icons"].as_array().unwrap().len(), 3);
        assert_eq!(manifest["shortcuts"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn service_worker_is_root_scoped_and_cannot_cache_reader_content() {
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let response = router(test_state().await)
            .oneshot(
                Request::get("/service-worker.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(response.headers()["service-worker-allowed"], "/");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let script = String::from_utf8_lossy(&body);
        assert!(script.contains("STATIC_ASSETS"));
        assert!(!script.contains("/articles"));
        assert!(!script.contains("/api/"));
        assert!(script.contains("credentials: \"omit\""));
    }

    #[test]
    fn external_link_templates_keep_mandatory_privacy_attributes() {
        for (name, template) in [
            ("article", include_str!("../templates/article.html")),
            (
                "article panel",
                include_str!("../templates/article_panel.html"),
            ),
            (
                "article enclosures",
                include_str!("../templates/article_enclosures.html"),
            ),
            (
                "article YouTube",
                include_str!("../templates/article_youtube.html"),
            ),
            ("catalog", include_str!("../templates/catalog.html")),
            (
                "catalog preview",
                include_str!("../templates/catalog_preview.html"),
            ),
        ] {
            for anchor in template.split("<a").skip(1) {
                let tag = anchor.split('>').next().unwrap_or_default();
                if tag.contains("target=\"_blank\"") {
                    assert!(
                        tag.contains("rel=\"noopener noreferrer\""),
                        "{name} has an external link without opener isolation"
                    );
                    assert!(
                        tag.contains("referrerpolicy=\"no-referrer\""),
                        "{name} has an external link without referrer isolation"
                    );
                }
            }
        }
    }

    #[test]
    fn alternative_youtube_origin_is_narrowly_added_to_frame_csp() {
        let policy = rusty_feed_core::youtube::YouTubePolicy::try_new_with_alternative(
            true,
            "",
            "Invidious",
            "https://video.example.org/private",
        )
        .unwrap();
        let header = reader_content_security_policy(&policy, false);
        let csp = header.to_str().unwrap();
        assert!(csp.contains(
            "frame-src 'self' https://www.youtube-nocookie.com https://video.example.org;"
        ));
        assert!(!csp.contains("frame-src *"));
    }

    #[test]
    fn captcha_origins_added_only_when_enabled() {
        let policy = rusty_feed_core::youtube::YouTubePolicy::default();
        let disabled = reader_content_security_policy(&policy, false);
        assert!(!disabled
            .to_str()
            .unwrap()
            .contains("challenges.cloudflare.com"));
        let enabled = reader_content_security_policy(&policy, true);
        let csp = enabled.to_str().unwrap();
        assert!(csp.contains("script-src 'self' https://challenges.cloudflare.com"));
        assert!(csp.contains("https://challenges.cloudflare.com"));
    }

    #[test]
    fn every_page_shell_exposes_the_same_install_metadata() {
        for template in [
            include_str!("../templates/base.html"),
            include_str!("../templates/app_base.html"),
        ] {
            assert!(template.contains("rel=\"manifest\" href=\"/manifest.webmanifest\""));
            assert!(template.contains("rel=\"icon\" href=\"/static/icon.svg\""));
            assert!(template.contains("rel=\"apple-touch-icon\" href=\"/static/icon-192.png\""));
        }
    }
}
