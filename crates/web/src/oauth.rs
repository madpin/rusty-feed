//! OAuth 2.0 / OIDC authorization-code login (with PKCE + state).
//!
//! Supports Google, GitHub, and a generic OIDC provider. Implemented directly
//! over `reqwest` (rustls) for TLS control and testability (FND-117). The `state`
//! parameter (CSRF) and the PKCE verifier are stashed in short-lived HTTP-only
//! cookies between the redirect and the callback.

use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use base64::Engine;
use rand::RngCore;
use serde::Deserialize;

use rustss_db::{oauth_accounts, users, User};

use crate::auth::csrf::constant_time_eq;
use crate::auth::{session, token};
use crate::error::AppError;
use crate::extract::ClientIp;
use crate::state::{AppState, OAuthProvider, ProviderKind};

const STATE_COOKIE: &str = "rustss_oauth_state";
const PKCE_COOKIE: &str = "rustss_oauth_pkce";

/// A normalized identity extracted from a provider's userinfo response.
struct Profile {
    subject: String,
    email: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

/// `GET /auth/oauth/{provider}` — redirect the user to the provider.
pub async fn start(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(slug): Path<String>,
    jar: CookieJar,
) -> Result<Response, AppError> {
    if !state.auth_limiter.check(&ip).await {
        return Err(AppError::RateLimited);
    }
    let provider = find_provider(&state, &slug).ok_or(AppError::NotFound)?;

    let (verifier, challenge) = pkce_pair();
    let csrf_state = token::generate().raw;
    let redirect_uri = callback_uri(&state, &slug);

    let authorize = build_authorize_url(provider, &redirect_uri, &csrf_state, &challenge)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("oauth authorize url: {e}")))?;

    let secure = state.settings.cookie_secure;
    let jar = jar
        .add(short_cookie(STATE_COOKIE, csrf_state, secure))
        .add(short_cookie(PKCE_COOKIE, verifier, secure));

    Ok((jar, Redirect::to(&authorize)).into_response())
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// `GET /auth/oauth/{provider}/callback` — verify, exchange, and sign in.
pub async fn callback(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    jar: CookieJar,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, AppError> {
    let provider = find_provider(&state, &slug).ok_or(AppError::NotFound)?;

    if query.error.is_some() {
        return Ok((clear_oauth(jar), Redirect::to("/login")).into_response());
    }

    let code = query
        .code
        .ok_or_else(|| AppError::Validation("missing authorization code".into()))?;
    let returned_state = query.state.unwrap_or_default();
    let expected_state = jar
        .get(STATE_COOKIE)
        .map(|c| c.value().to_string())
        .unwrap_or_default();
    if expected_state.is_empty() || !constant_time_eq(&expected_state, &returned_state) {
        return Err(AppError::Forbidden); // CSRF / state mismatch
    }
    let verifier = jar
        .get(PKCE_COOKIE)
        .map(|c| c.value().to_string())
        .unwrap_or_default();

    let redirect_uri = callback_uri(&state, &slug);
    let profile = exchange_and_fetch(provider, &code, &verifier, &redirect_uri)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("oauth exchange failed: {e}")))?;

    let user = upsert_oauth_user(&state, provider, profile).await?;

    let jar = clear_oauth(jar);
    let jar = session::login(jar, &state.db, user.id, state.settings.cookie_secure).await?;
    Ok((jar, Redirect::to("/")).into_response())
}

fn find_provider<'a>(state: &'a AppState, slug: &str) -> Option<&'a OAuthProvider> {
    state
        .settings
        .oauth_providers
        .iter()
        .find(|p| p.slug == slug)
}

fn callback_uri(state: &AppState, slug: &str) -> String {
    format!(
        "{}/auth/oauth/{}/callback",
        state.settings.base_url.trim_end_matches('/'),
        slug
    )
}

fn build_authorize_url(
    provider: &OAuthProvider,
    redirect_uri: &str,
    csrf_state: &str,
    challenge: &str,
) -> Result<String, reqwest::Error> {
    let url = reqwest::Url::parse_with_params(
        &provider.auth_url,
        &[
            ("response_type", "code"),
            ("client_id", provider.client_id.as_str()),
            ("redirect_uri", redirect_uri),
            ("scope", provider.scopes.as_str()),
            ("state", csrf_state),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
        ],
    )
    .expect("provider auth_url is a valid base URL");
    Ok(url.to_string())
}

async fn exchange_and_fetch(
    provider: &OAuthProvider,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> anyhow::Result<Profile> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("RustSS")
        .build()?;

    // Exchange the code for an access token.
    let token: TokenResponse = http
        .post(&provider.token_url)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", provider.client_id.as_str()),
            ("client_secret", provider.client_secret.expose()),
            ("code_verifier", verifier),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    // Fetch the user profile.
    let json: serde_json::Value = http
        .get(&provider.userinfo_url)
        .bearer_auth(&token.access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let profile = match provider.kind {
        ProviderKind::Oidc => Profile {
            subject: json
                .get("sub")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            email: json.get("email").and_then(|v| v.as_str()).map(String::from),
            name: json.get("name").and_then(|v| v.as_str()).map(String::from),
        },
        ProviderKind::GitHub => {
            let subject = json
                .get("id")
                .map(|v| {
                    v.as_i64()
                        .map(|i| i.to_string())
                        .unwrap_or_else(|| v.as_str().unwrap_or_default().to_string())
                })
                .unwrap_or_default();
            let name = json
                .get("name")
                .and_then(|v| v.as_str())
                .or_else(|| json.get("login").and_then(|v| v.as_str()))
                .map(String::from);
            let mut email = json.get("email").and_then(|v| v.as_str()).map(String::from);
            if email.is_none() {
                email = fetch_github_email(&http, &token.access_token)
                    .await
                    .ok()
                    .flatten();
            }
            Profile {
                subject,
                email,
                name,
            }
        }
    };

    if profile.subject.is_empty() {
        anyhow::bail!("provider did not return an account id");
    }
    Ok(profile)
}

#[derive(Deserialize)]
struct GithubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

async fn fetch_github_email(
    http: &reqwest::Client,
    access_token: &str,
) -> anyhow::Result<Option<String>> {
    let emails: Vec<GithubEmail> = http
        .get("https://api.github.com/user/emails")
        .bearer_auth(access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let pick = emails
        .iter()
        .find(|e| e.primary && e.verified)
        .or_else(|| emails.iter().find(|e| e.verified))
        .or_else(|| emails.first());
    Ok(pick.map(|e| e.email.clone()))
}

async fn upsert_oauth_user(
    state: &AppState,
    provider: &OAuthProvider,
    profile: Profile,
) -> Result<User, AppError> {
    let pool = state.db.pool();

    // 1. Already linked → sign in.
    if let Some(account) = oauth_accounts::find(pool, &provider.slug, &profile.subject).await? {
        return users::find_by_id(pool, account.user_id)
            .await?
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("linked user missing")));
    }

    // 2. We need an email to create or match an account.
    let email = profile
        .email
        .ok_or_else(|| AppError::Validation("the provider did not return an email".into()))?;

    // 3. Match an existing local user by email (link), else create one.
    let user = match users::find_by_email(pool, &email).await? {
        Some(existing) => existing,
        None => {
            let bootstrap = users::count(pool).await? == 0;
            if !bootstrap
                && !matches!(
                    state.settings.registration_mode,
                    rustss_core::config::RegistrationMode::Open
                )
            {
                return Err(AppError::Validation(
                    "New account registration is not open on this instance. Ask an administrator for an invitation and create a local account first."
                        .into(),
                ));
            }
            let role = if bootstrap { "admin" } else { "user" };
            users::create_with_default_feeds(
                pool,
                &email,
                profile.name.as_deref(),
                None,
                role,
                None,
            )
            .await?
        }
    };

    oauth_accounts::create(pool, user.id, &provider.slug, &profile.subject).await?;
    Ok(user)
}

fn pkce_pair() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(rustss_core::hash::sha256_bytes(verifier.as_bytes()));
    (verifier, challenge)
}

fn short_cookie(name: &'static str, value: String, secure: bool) -> Cookie<'static> {
    Cookie::build((name, value))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::minutes(10))
        .build()
}

fn clear_oauth(jar: CookieJar) -> CookieJar {
    jar.remove(Cookie::build((STATE_COOKIE, "")).path("/").build())
        .remove(Cookie::build((PKCE_COOKIE, "")).path("/").build())
}
