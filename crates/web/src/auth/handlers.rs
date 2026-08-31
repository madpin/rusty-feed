//! Local-account authentication handlers (login / register / logout) and the
//! home page. Server-rendered with Askama; CSRF-protected form posts.

use askama::Template;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use time::OffsetDateTime;

use rustss_core::config::RegistrationMode;
use rustss_db::users;

use crate::auth::extractor::MaybeUser;
use crate::auth::password::{hash_password, validate_strength, verify_password};
use crate::auth::{csrf, session};
use crate::captcha;
use crate::error::AppError;
use crate::extract::ClientIp;
use crate::state::AppState;
use crate::verify;

#[derive(Template)]
#[template(path = "landing.html")]
struct LandingTemplate {
    csrf_token: String,
    demo_mode: bool,
    demo_admin_readonly: bool,
    registration_open: bool,
    comparison_date: &'static str,
}

/// A configured OAuth provider, for rendering "Continue with …" buttons.
#[derive(Clone)]
struct ProviderButton {
    slug: String,
    name: String,
}

fn provider_buttons(state: &AppState) -> Vec<ProviderButton> {
    state
        .settings
        .oauth_providers
        .iter()
        .map(|p| ProviderButton {
            slug: p.slug.clone(),
            name: p.display_name.clone(),
        })
        .collect()
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    error: Option<String>,
    csrf_token: String,
    providers: Vec<ProviderButton>,
    passkeys_enabled: bool,
}

#[derive(Template)]
#[template(path = "register.html")]
struct RegisterTemplate {
    error: Option<String>,
    csrf_token: String,
    registration_open: bool,
    invite_token: String,
    providers: Vec<ProviderButton>,
    min_password_length: usize,
    captcha_provider: Option<String>,
    captcha_site_key: Option<String>,
    require_tos_acceptance: bool,
}

#[derive(Deserialize)]
pub struct LoginForm {
    email: String,
    password: String,
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct RegisterForm {
    #[serde(default)]
    name: String,
    email: String,
    password: String,
    #[serde(default)]
    invite_token: String,
    csrf_token: String,
    /// Cloudflare Turnstile token, when that CAPTCHA is enabled.
    #[serde(default, rename = "cf-turnstile-response")]
    cf_turnstile_response: String,
    /// hCaptcha token, when that CAPTCHA is enabled.
    #[serde(default, rename = "h-captcha-response")]
    h_captcha_response: String,
    /// Terms/Privacy acceptance (checkbox present only when required).
    #[serde(default)]
    accept_terms: String,
}

#[derive(Default, Deserialize)]
pub struct RegisterQuery {
    #[serde(default)]
    invite: String,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

fn render<T: Template>(tpl: &T) -> Result<Html<String>, AppError> {
    tpl.render()
        .map(Html)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))
}

/// `GET /` — landing page for anonymous visitors; signed-in users go to the
/// reader.
pub async fn home(
    State(state): State<AppState>,
    jar: CookieJar,
    MaybeUser(user): MaybeUser,
) -> Result<Response, AppError> {
    if user.is_some() {
        return Ok((jar, Redirect::to("/articles")).into_response());
    }
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = render(&LandingTemplate {
        csrf_token,
        demo_mode: state.settings.demo_mode,
        demo_admin_readonly: state.settings.demo_mode && state.settings.demo_admin_readonly,
        registration_open: matches!(
            state.settings.registration_mode,
            rustss_core::config::RegistrationMode::Open
        ),
        // Verify the Miniflux column against current docs before changing this.
        comparison_date: "2026-08",
    })?;
    Ok((jar, body).into_response())
}

#[derive(Deserialize)]
pub struct DemoStartForm {
    csrf_token: String,
    #[serde(default)]
    admin: String,
}

/// `POST /demo/start` — provision an ephemeral, sandboxed demo account and sign
/// in. Available only when `DEMO_MODE=true` (§7). Data resets daily.
pub async fn demo_start(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    jar: CookieJar,
    Form(form): Form<DemoStartForm>,
) -> Result<Response, AppError> {
    if !state.settings.demo_mode {
        return Err(AppError::NotFound);
    }
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !state.auth_limiter.check(&ip).await {
        return Err(AppError::RateLimited);
    }
    let want_admin = state.settings.demo_admin_readonly
        && matches!(form.admin.trim(), "1" | "on" | "true" | "yes");
    let role = if want_admin { "admin_readonly" } else { "user" };
    let handle = crate::auth::token::generate();
    let email = format!(
        "demo-{}@demo.invalid",
        &handle.raw[..24.min(handle.raw.len())]
    );
    let name = if want_admin {
        "Demo admin (read-only)"
    } else {
        "Demo user"
    };
    let user = users::create_demo(state.db.pool(), &email, name, role).await?;
    metrics::counter!("rustss_demo_provisions_total", "role" => role).increment(1);
    // Demo sessions get a short TTL so abandoned ones expire before the reset.
    let ttl = time::Duration::hours(i64::from(state.settings.demo_session_ttl_hours).max(1));
    let jar =
        session::login_with_ttl(jar, &state.db, user.id, state.settings.cookie_secure, ttl).await?;
    // A readable (non-HttpOnly) marker so the client shows the demo banner.
    let jar = jar.add(
        axum_extra::extract::cookie::Cookie::build(("rustss_demo", "1"))
            .path("/")
            .same_site(axum_extra::extract::cookie::SameSite::Lax)
            .secure(state.settings.cookie_secure)
            .build(),
    );
    Ok((jar, Redirect::to("/articles")).into_response())
}

/// `GET /login` — show the sign-in form (redirects home if already signed in).
pub async fn login_page(
    State(state): State<AppState>,
    jar: CookieJar,
    MaybeUser(user): MaybeUser,
) -> Result<Response, AppError> {
    if user.is_some() {
        return Ok((jar, Redirect::to("/")).into_response());
    }
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = render(&LoginTemplate {
        error: None,
        csrf_token,
        providers: provider_buttons(&state),
        passkeys_enabled: state.webauthn.is_some(),
    })?;
    Ok((jar, body).into_response())
}

/// `POST /login` — verify credentials and start a session.
pub async fn login_submit(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> Result<Response, AppError> {
    if !state.auth_limiter.check(&ip).await {
        return Err(AppError::RateLimited);
    }
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let secure = state.settings.cookie_secure;
    let max_failures = state.settings.login_max_failures as i32;
    let lockout_seconds = state.settings.login_lockout_seconds as i64;

    // Re-render the sign-in form with an error message.
    let reshow = |state: &AppState, jar: CookieJar, message: &str| -> Result<Response, AppError> {
        let (csrf_token, jar) = csrf::issue(jar, secure);
        let body = render(&LoginTemplate {
            error: Some(message.to_string()),
            csrf_token,
            providers: provider_buttons(state),
            passkeys_enabled: state.webauthn.is_some(),
        })?;
        Ok((jar, body).into_response())
    };

    if let Some(user) = users::find_by_email(state.db.pool(), form.email.trim()).await? {
        // Temporary per-account lock after too many failed attempts (FND-191).
        if user
            .locked_until
            .is_some_and(|until| until > OffsetDateTime::now_utc())
        {
            return reshow(
                &state,
                jar,
                "This account is temporarily locked after too many failed attempts. Please try again later.",
            );
        }
        if let Some(ref hash) = user.password_hash {
            if verify_password(&form.password, hash) {
                let _ = users::clear_failed_logins(state.db.pool(), user.id).await;
                let destination = if user.onboarded_at.is_none() {
                    "/welcome"
                } else {
                    "/"
                };
                let jar = session::login(jar, &state.db, user.id, secure).await?;
                return Ok((jar, Redirect::to(destination)).into_response());
            }
        }
        // Wrong password for a real account: count the failure toward lockout.
        let _ = users::record_failed_login(state.db.pool(), user.id, max_failures, lockout_seconds)
            .await;
    }

    // Generic failure message (don't reveal whether the email exists).
    reshow(&state, jar, "Invalid email or password.")
}

/// `GET /register` — show the registration form.
pub async fn register_page(
    State(state): State<AppState>,
    Query(query): Query<RegisterQuery>,
    jar: CookieJar,
    MaybeUser(user): MaybeUser,
) -> Result<Response, AppError> {
    if user.is_some() {
        return Ok((jar, Redirect::to("/")).into_response());
    }
    let invite_token = query.invite.trim().to_string();
    let open = registration_open(&state, &invite_token).await?;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let (captcha_provider, captcha_site_key) = captcha_fields(&state);
    let body = render(&RegisterTemplate {
        error: None,
        csrf_token,
        registration_open: open,
        invite_token,
        providers: provider_buttons(&state),
        min_password_length: state.settings.min_password_length,
        captcha_provider,
        captcha_site_key,
        require_tos_acceptance: state.settings.require_tos_acceptance,
    })?;
    Ok((jar, body).into_response())
}

/// The CAPTCHA provider name and public site key to render, if enabled.
fn captcha_fields(state: &AppState) -> (Option<String>, Option<String>) {
    match &state.settings.captcha {
        Some(captcha) => (
            Some(captcha.provider.clone()),
            Some(captcha.site_key.clone()),
        ),
        None => (None, None),
    }
}

/// `POST /register` — create a local account and start a session.
pub async fn register_submit(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    jar: CookieJar,
    Form(form): Form<RegisterForm>,
) -> Result<Response, AppError> {
    if !state.auth_limiter.check(&ip).await {
        return Err(AppError::RateLimited);
    }
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let secure = state.settings.cookie_secure;

    let user_count = users::count(state.db.pool()).await?;
    let bootstrap = user_count == 0; // first user always allowed + becomes admin
    let invite_token = form.invite_token.trim();
    let invitation_allowed = matches!(state.settings.registration_mode, RegistrationMode::Invite)
        && !invite_token.is_empty()
        && rustss_db::invitations::is_valid(
            state.db.pool(),
            &crate::auth::token::hash(invite_token),
            Some(form.email.trim()),
        )
        .await?;
    let allowed = matches!(state.settings.registration_mode, RegistrationMode::Open)
        || bootstrap
        || invitation_allowed;
    let providers = provider_buttons(&state);
    let (captcha_provider, captcha_site_key) = captcha_fields(&state);
    let min_password_length = state.settings.min_password_length;

    // Helper to re-render the form with an error message.
    let reshow = |jar: CookieJar, message: &str| -> Result<Response, AppError> {
        let (csrf_token, jar) = csrf::issue(jar, secure);
        let body = render(&RegisterTemplate {
            error: Some(message.to_string()),
            csrf_token,
            registration_open: true,
            invite_token: invite_token.to_string(),
            providers: providers.clone(),
            min_password_length,
            captcha_provider: captcha_provider.clone(),
            captcha_site_key: captcha_site_key.clone(),
            require_tos_acceptance: state.settings.require_tos_acceptance,
        })?;
        Ok((jar, body).into_response())
    };

    if !allowed {
        let (csrf_token, jar) = csrf::issue(jar, secure);
        let body = render(&RegisterTemplate {
            error: None,
            csrf_token,
            registration_open: false,
            invite_token: String::new(),
            providers: providers.clone(),
            min_password_length,
            captcha_provider: captcha_provider.clone(),
            captcha_site_key: captcha_site_key.clone(),
            require_tos_acceptance: state.settings.require_tos_acceptance,
        })?;
        return Ok((jar, body).into_response());
    }

    // Anti-bot challenge (no-op when CAPTCHA is disabled).
    let captcha_token = if !form.cf_turnstile_response.trim().is_empty() {
        form.cf_turnstile_response.trim()
    } else {
        form.h_captcha_response.trim()
    };
    match captcha::verify(&state, captcha_token, &ip).await {
        Ok(()) => {}
        Err(AppError::Validation(message)) => return reshow(jar, &message),
        Err(other) => return Err(other),
    }

    let accepted_terms = matches!(form.accept_terms.trim(), "on" | "true" | "1" | "yes");
    if state.settings.require_tos_acceptance && !accepted_terms {
        return reshow(
            jar,
            "Please accept the Terms and Privacy Policy to continue.",
        );
    }

    let email = form.email.trim();
    if email.is_empty() || !email.contains('@') {
        return reshow(jar, "Please enter a valid email address.");
    }
    if let Err(message) = validate_strength(&form.password, min_password_length) {
        return reshow(jar, &message);
    }
    if users::find_by_email(state.db.pool(), email)
        .await?
        .is_some()
    {
        return reshow(jar, "An account with that email already exists.");
    }

    let role = if bootstrap { "admin" } else { "user" };
    let password_hash = hash_password(&form.password)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("password hash failed: {e}")))?;
    let name = {
        let n = form.name.trim();
        if n.is_empty() {
            None
        } else {
            Some(n)
        }
    };

    let user = if invitation_allowed && !bootstrap {
        users::create_with_invitation(
            state.db.pool(),
            &crate::auth::token::hash(invite_token),
            email,
            name,
            &password_hash,
        )
        .await?
        .ok_or_else(|| AppError::Validation("This invitation is no longer available.".into()))?
    } else {
        users::create_with_default_feeds(
            state.db.pool(),
            email,
            name,
            None,
            role,
            Some(&password_hash),
        )
        .await?
    };

    metrics::counter!("rustss_signups_total").increment(1);
    // Record consent for lawful contact/consent (FND-195).
    if accepted_terms || state.settings.require_tos_acceptance {
        let _ = users::record_consent(state.db.pool(), user.id).await;
    }

    // Best-effort verification email (never fatal): local accounts start
    // unverified; privileged actions are gated until they confirm (FND-191).
    if state.settings.email_verification_required {
        verify::send_verification_email(&state, user.id, email).await;
    }

    let jar = session::login(jar, &state.db, user.id, secure).await?;
    // New accounts start at the first-run wizard (§8.4); it is skippable.
    Ok((jar, Redirect::to("/welcome")).into_response())
}

/// `POST /logout` — end the current session.
pub async fn logout_submit(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let jar = session::logout(jar, &state.db).await;
    let destination = state
        .settings
        .reverse_proxy_auth
        .logout_url
        .as_deref()
        .unwrap_or("/login");
    Ok((jar, Redirect::to(destination)).into_response())
}

/// Whether registration is currently possible (open, or no users yet).
async fn registration_open(state: &AppState, invite_token: &str) -> Result<bool, AppError> {
    if matches!(state.settings.registration_mode, RegistrationMode::Open) {
        return Ok(true);
    }
    if users::count(state.db.pool()).await? == 0 {
        return Ok(true);
    }
    if matches!(state.settings.registration_mode, RegistrationMode::Invite)
        && !invite_token.is_empty()
    {
        return Ok(rustss_db::invitations::is_valid(
            state.db.pool(),
            &crate::auth::token::hash(invite_token),
            None,
        )
        .await?);
    }
    Ok(false)
}
