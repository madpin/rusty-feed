//! Shared request extractors.

use std::convert::Infallible;
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;

/// The best-effort client IP address, used for rate-limiting keys.
///
/// Resolution order: `X-Forwarded-For` (first hop) → `X-Real-IP` → the socket
/// peer address (when the server is started with connect-info) → `"unknown"`.
pub struct ClientIp(pub String);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Infallible> {
        let ip = header_first(parts, "x-forwarded-for")
            .or_else(|| header_first(parts, "x-real-ip"))
            .or_else(|| {
                parts
                    .extensions
                    .get::<ConnectInfo<SocketAddr>>()
                    .map(|ci| ci.0.ip().to_string())
            })
            .unwrap_or_else(|| "unknown".to_string());
        Ok(ClientIp(ip))
    }
}

fn header_first(parts: &Parts, name: &str) -> Option<String> {
    parts
        .headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
