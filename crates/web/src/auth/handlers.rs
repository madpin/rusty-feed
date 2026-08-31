//! Local-account authentication handlers (login / register / logout) and the
//! home page. Server-rendered with Askama; CSRF-protected form posts.

use askama::Template;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;

use rustss_core::config::RegistrationMode;
use rustss_db::users;

use crate::auth::extractor::MaybeUser;
use crate::auth::password::{hash_password, verify_password};
use crate::auth::{csrf, session};
use crate::error::AppError;
use crate::extract::ClientIp;
use crate::state::AppState;

#[derive(Template)]
#[template(path = "home.html")]
struct HomeTemplate {
    user_name: Option<String>,
    csrf_token: String,
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
    let body = render(&HomeTemplate {
        user_name: None,
        csrf_token,
    })?;
    Ok((jar, body).into_response())
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

    if let Some(user) = users::find_by_email(state.db.pool(), form.email.trim()).await? {
        if let Some(ref hash) = user.password_hash {
            if verify_password(&form.password, hash) {
                let jar = session::login(jar, &state.db, user.id, secure).await?;
                return Ok((jar, Redirect::to("/")).into_response());
            }
        }
    }

    // Generic failure message (don't reveal whether the email exists).
    let (csrf_token, jar) = csrf::issue(jar, secure);
    let body = render(&LoginTemplate {
        error: Some("Invalid email or password.".to_string()),
        csrf_token,
        providers: provider_buttons(&state),
        passkeys_enabled: state.webauthn.is_some(),
    })?;
    Ok((jar, body).into_response())
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
    let body = render(&RegisterTemplate {
        error: None,
        csrf_token,
        registration_open: open,
        invite_token,
        providers: provider_buttons(&state),
    })?;
    Ok((jar, body).into_response())
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

    // Helper to re-render the form with an error message.
    let reshow = |jar: CookieJar, message: &str| -> Result<Response, AppError> {
        let (csrf_token, jar) = csrf::issue(jar, secure);
        let body = render(&RegisterTemplate {
            error: Some(message.to_string()),
            csrf_token,
            registration_open: true,
            invite_token: invite_token.to_string(),
            providers: providers.clone(),
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
        })?;
        return Ok((jar, body).into_response());
    }

    let email = form.email.trim();
    if email.is_empty() || !email.contains('@') {
        return reshow(jar, "Please enter a valid email address.");
    }
    if form.password.len() < 8 {
        return reshow(jar, "Password must be at least 8 characters.");
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

    let jar = session::login(jar, &state.db, user.id, secure).await?;
    Ok((jar, Redirect::to("/")).into_response())
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
