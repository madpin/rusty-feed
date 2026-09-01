//! Passwordless WebAuthn registration, authentication, and credential management.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Response};
use axum::{Form, Json};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use webauthn_rs::prelude::{
    Passkey, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential,
};

use rusty_feed_db::{passkeys, users, User};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::auth::password::verify_password;
use crate::auth::session;
use crate::auth::token;
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

struct CredentialView {
    id: String,
    name: String,
    created: String,
    last_used: Option<String>,
}

#[derive(Template)]
#[template(path = "passkeys.html")]
struct PasskeysTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    enabled: bool,
    has_password: bool,
    credentials: Vec<CredentialView>,
    notice: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
pub struct RegistrationStart {
    csrf_token: String,
    name: String,
    #[serde(default)]
    password: String,
}

#[derive(Deserialize)]
pub struct RegistrationFinish {
    csrf_token: String,
    ceremony_id: Uuid,
    name: String,
    credential: RegisterPublicKeyCredential,
}

#[derive(Deserialize)]
pub struct AuthenticationStart {
    csrf_token: String,
    email: String,
}

#[derive(Deserialize)]
pub struct AuthenticationFinish {
    csrf_token: String,
    ceremony_id: Uuid,
    credential: PublicKeyCredential,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

fn engine(state: &AppState) -> Result<&webauthn_rs::Webauthn, AppError> {
    state
        .webauthn
        .as_deref()
        .ok_or_else(|| AppError::Validation("Passkeys are not enabled for this instance.".into()))
}

async fn has_recent_session(
    state: &AppState,
    jar: &CookieJar,
    user_id: Uuid,
) -> Result<bool, AppError> {
    let Some(raw) = jar
        .get(session::SESSION_COOKIE)
        .map(|cookie| cookie.value())
    else {
        return Ok(false);
    };
    let Some(current) =
        rusty_feed_db::sessions::find_valid(state.db.pool(), &token::hash(raw)).await?
    else {
        return Ok(false);
    };
    Ok(current.user_id == user_id
        && current.created_at >= time::OffsetDateTime::now_utc() - time::Duration::minutes(15))
}

async fn render_page(
    state: &AppState,
    user: &User,
    jar: CookieJar,
    notice: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let credentials = passkeys::list(state.db.pool(), user.id)
        .await?
        .into_iter()
        .map(|credential| CredentialView {
            id: credential.id.to_string(),
            name: credential.name,
            created: ui.format_date(credential.created_at),
            last_used: credential.last_used_at.map(|value| ui.format_date(value)),
        })
        .collect();
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = PasskeysTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui,
        is_admin: user.role == "admin",
        enabled: state.webauthn.is_some(),
        has_password: user.password_hash.is_some(),
        credentials,
        notice,
        error,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    Ok((jar, body).into_response())
}

pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render_page(&state, &user, jar, None, None).await
}

pub async fn registration_start(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Json(input): Json<RegistrationStart>,
) -> Result<Json<Value>, AppError> {
    if !csrf::verify(&jar, &input.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(AppError::Validation(
            "Passkey names must contain 1 to 80 printable characters.".into(),
        ));
    }
    if let Some(hash) = user.password_hash.as_deref() {
        if !verify_password(&input.password, hash) {
            return Err(AppError::Validation(
                "Confirm your current password before adding a passkey.".into(),
            ));
        }
    } else if !has_recent_session(&state, &jar, user.id).await? {
        return Err(AppError::Validation(
            "Sign out and sign in again before adding a passkey.".into(),
        ));
    }
    let existing = passkeys::list(state.db.pool(), user.id).await?;
    if existing.len() >= 20 {
        return Err(AppError::Validation(
            "Remove an existing passkey before adding another.".into(),
        ));
    }
    let mut stored = Vec::with_capacity(existing.len());
    for row in existing {
        stored.push(
            serde_json::from_value::<Passkey>(row.passkey)
                .map_err(|_| AppError::Internal(anyhow::anyhow!("stored passkey is invalid")))?,
        );
    }
    let display_name = user.name.as_deref().unwrap_or(&user.email);
    let exclude = (!stored.is_empty()).then(|| {
        stored
            .iter()
            .map(|passkey| passkey.cred_id().clone())
            .collect()
    });
    let (challenge, ceremony) = engine(&state)?
        .start_passkey_registration(user.id, &user.email, display_name, exclude)
        .map_err(|_| AppError::Validation("Unable to start passkey registration.".into()))?;
    let ceremony_id = passkeys::create_ceremony(
        state.db.pool(),
        user.id,
        "registration",
        &serde_json::to_value(ceremony).map_err(|_| {
            AppError::Internal(anyhow::anyhow!("passkey state serialization failed"))
        })?,
    )
    .await?;
    Ok(Json(
        json!({"ceremony_id": ceremony_id, "options": challenge}),
    ))
}

pub async fn registration_finish(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Json(input): Json<RegistrationFinish>,
) -> Result<Json<Value>, AppError> {
    if !csrf::verify(&jar, &input.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let state_json =
        passkeys::take_ceremony(state.db.pool(), input.ceremony_id, user.id, "registration")
            .await?
            .ok_or_else(|| {
                AppError::Validation("This passkey request expired or was already used.".into())
            })?;
    let ceremony: PasskeyRegistration = serde_json::from_value(state_json)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("stored passkey state is invalid")))?;
    let passkey = engine(&state)?
        .finish_passkey_registration(&input.credential, &ceremony)
        .map_err(|_| AppError::Validation("The authenticator rejected registration.".into()))?;
    let encoded = serde_json::to_value(&passkey)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("passkey serialization failed")))?;
    passkeys::create(
        state.db.pool(),
        user.id,
        input.name.trim(),
        passkey.cred_id().as_ref(),
        &encoded,
    )
    .await
    .map_err(|error| {
        tracing::warn!(error = %error, user_id = %user.id, "passkey credential insert rejected");
        AppError::Validation("That passkey or name is already registered.".into())
    })?;
    Ok(Json(json!({"ok": true})))
}

pub async fn authentication_start(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(input): Json<AuthenticationStart>,
) -> Result<Json<Value>, AppError> {
    if !csrf::verify(&jar, &input.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let user = users::find_by_email(state.db.pool(), input.email.trim())
        .await?
        .ok_or_else(|| {
            AppError::Validation("Passkey sign-in is unavailable for this account.".into())
        })?;
    let rows = passkeys::list(state.db.pool(), user.id).await?;
    if rows.is_empty() {
        return Err(AppError::Validation(
            "Passkey sign-in is unavailable for this account.".into(),
        ));
    }
    let credentials = rows
        .into_iter()
        .map(|row| serde_json::from_value::<Passkey>(row.passkey))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AppError::Internal(anyhow::anyhow!("stored passkey is invalid")))?;
    let (challenge, ceremony) = engine(&state)?
        .start_passkey_authentication(&credentials)
        .map_err(|_| AppError::Validation("Unable to start passkey sign-in.".into()))?;
    let ceremony_id = passkeys::create_ceremony(
        state.db.pool(),
        user.id,
        "authentication",
        &serde_json::to_value(ceremony).map_err(|_| {
            AppError::Internal(anyhow::anyhow!("passkey state serialization failed"))
        })?,
    )
    .await?;
    Ok(Json(
        json!({"ceremony_id": ceremony_id, "options": challenge}),
    ))
}

pub async fn authentication_finish(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(input): Json<AuthenticationFinish>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &input.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let row = passkeys::take_authentication_ceremony(state.db.pool(), input.ceremony_id).await?;
    let (user_id, state_json) = row.ok_or_else(|| {
        AppError::Validation("This passkey request expired or was already used.".into())
    })?;
    let ceremony: PasskeyAuthentication = serde_json::from_value(state_json)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("stored passkey state is invalid")))?;
    let result = engine(&state)?
        .finish_passkey_authentication(&input.credential, &ceremony)
        .map_err(|_| AppError::Validation("Passkey sign-in failed.".into()))?;
    let rows = passkeys::list(state.db.pool(), user_id).await?;
    let mut matched = false;
    for row in rows {
        let mut passkey: Passkey = serde_json::from_value(row.passkey)
            .map_err(|_| AppError::Internal(anyhow::anyhow!("stored passkey is invalid")))?;
        if passkey.update_credential(&result).is_some() {
            passkeys::update_after_authentication(
                state.db.pool(),
                row.id,
                user_id,
                &serde_json::to_value(passkey).map_err(|_| {
                    AppError::Internal(anyhow::anyhow!("passkey serialization failed"))
                })?,
            )
            .await?;
            matched = true;
            break;
        }
    }
    if !matched {
        return Err(AppError::Validation("Passkey sign-in failed.".into()));
    }
    let jar = session::login(jar, &state.db, user_id, state.settings.cookie_secure).await?;
    Ok((jar, Json(json!({"ok": true, "redirect": "/articles"}))).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !has_recent_session(&state, &jar, user.id).await? {
        return render_page(
            &state,
            &user,
            jar,
            None,
            Some("Sign out and sign in again before revoking a passkey.".into()),
        )
        .await;
    }
    let current = passkeys::list(state.db.pool(), user.id).await?;
    if current.len() == 1 && user.password_hash.is_none() {
        return render_page(
            &state,
            &user,
            jar,
            None,
            Some("Add a recovery sign-in method before deleting your last passkey.".into()),
        )
        .await;
    }
    if !passkeys::delete(state.db.pool(), id, user.id).await? {
        return Err(AppError::NotFound);
    }
    render_page(
        &state,
        &user,
        jar,
        Some("Passkey revoked. Existing sessions remain visible under Sessions.".into()),
        None,
    )
    .await
}
