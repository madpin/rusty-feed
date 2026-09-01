//! Administrator-owned host-pinned publisher acquisition profiles.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use rusty_feed_core::acquisition::AcquisitionSecrets;
use rusty_feed_db::{acquisition_profiles, feeds, User};

use crate::auth::csrf;
use crate::auth::extractor::{AdminReadUser, AdminUser};
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

struct ProfileView {
    id: String,
    name: String,
    hosts: String,
    path: String,
    cookie: bool,
    proxy: bool,
    invalid_tls: bool,
    expires: String,
}

struct FeedView {
    id: String,
    name: String,
    url: String,
}

struct AuditView {
    action: String,
    profile: String,
    feed: String,
    detail: String,
    created: String,
}

#[derive(Template)]
#[template(path = "admin_acquisition.html")]
struct AcquisitionTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    profiles: Vec<ProfileView>,
    feeds: Vec<FeedView>,
    audit: Vec<AuditView>,
    notice: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateForm {
    csrf_token: String,
    name: String,
    allowed_hosts: String,
    #[serde(default)]
    allowed_path_prefix: String,
    #[serde(default)]
    cookie_header: String,
    #[serde(default)]
    proxy_url: String,
    #[serde(default)]
    allow_invalid_tls: Option<String>,
    #[serde(default)]
    expires_days: String,
}

#[derive(Deserialize)]
pub struct AssignForm {
    csrf_token: String,
    feed_id: Uuid,
    #[serde(default)]
    profile_id: String,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

async fn render_page(
    state: &AppState,
    user: &User,
    jar: CookieJar,
    notice: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let (profiles, feeds, audit) = tokio::try_join!(
        acquisition_profiles::list(state.db.pool()),
        feeds::list_admin(state.db.pool(), 1_000),
        acquisition_profiles::list_audit(state.db.pool(), 100),
    )?;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let audit = audit
        .into_iter()
        .map(|entry| AuditView {
            action: entry.action,
            profile: entry
                .profile_name
                .unwrap_or_else(|| "Removed profile".into()),
            feed: entry.feed_name.unwrap_or_else(|| "—".into()),
            detail: entry.detail.to_string(),
            created: ui.format_date(entry.created_at),
        })
        .collect();
    let body = AcquisitionTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui,
        is_admin: true,
        profiles: profiles
            .into_iter()
            .map(|profile| ProfileView {
                id: profile.id.to_string(),
                name: profile.name,
                hosts: profile.allowed_hosts.join(", "),
                path: profile
                    .allowed_path_prefix
                    .unwrap_or_else(|| "All paths".into()),
                cookie: profile.uses_cookie,
                proxy: profile.uses_proxy,
                invalid_tls: profile.allow_invalid_tls,
                expires: profile
                    .expires_at
                    .map_or_else(|| "Never".into(), |value| value.to_string()),
            })
            .collect(),
        feeds: feeds
            .into_iter()
            .map(|feed| FeedView {
                id: feed.id.to_string(),
                name: feed.name,
                url: feed.url,
            })
            .collect(),
        audit,
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
    AdminReadUser(user): AdminReadUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render_page(&state, &user, jar, None, None).await
}

fn parse_hosts(value: &str) -> Result<Vec<String>, String> {
    let mut hosts = Vec::new();
    for host in value
        .split(',')
        .map(|host| host.trim().to_ascii_lowercase())
    {
        if host.is_empty() {
            continue;
        }
        if host.len() > 253 || host.contains(['/', ':']) || host.chars().any(char::is_whitespace) {
            return Err("Hosts must be comma-separated DNS names without schemes or ports.".into());
        }
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    if hosts.is_empty() || hosts.len() > 32 {
        return Err("Use between 1 and 32 explicit host boundaries.".into());
    }
    Ok(hosts)
}

fn valid_path_boundary(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 512
        && !path.contains(['?', '#'])
        && !path.split('/').any(|segment| segment == "..")
}

pub async fn create(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Form(form): Form<CreateForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let name = form.name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return render_page(
            &state,
            &user,
            jar,
            None,
            Some("Profile names must contain 1 to 80 printable characters.".into()),
        )
        .await;
    }
    let hosts = match parse_hosts(&form.allowed_hosts) {
        Ok(hosts) => hosts,
        Err(error) => return render_page(&state, &user, jar, None, Some(error)).await,
    };
    let path = form.allowed_path_prefix.trim();
    let path = (!path.is_empty()).then_some(path);
    if path.is_some_and(|path| !valid_path_boundary(path)) {
        return render_page(
            &state,
            &user,
            jar,
            None,
            Some("Path boundaries must be absolute paths without traversal, query, or fragment components.".into()),
        )
        .await;
    }
    let cookie = form.cookie_header.trim();
    let proxy = form.proxy_url.trim();
    if !cookie.is_empty() && path.is_none() {
        return render_page(
            &state,
            &user,
            jar,
            None,
            Some("Cookie profiles require an explicit allowed path boundary.".into()),
        )
        .await;
    }
    let secrets = AcquisitionSecrets {
        cookie_header: (!cookie.is_empty()).then(|| cookie.to_string()),
        proxy_url: (!proxy.is_empty()).then(|| proxy.to_string()),
    };
    let policy = rusty_feed_feeds::AcquisitionPolicy {
        allowed_hosts: hosts.clone(),
        allowed_path_prefix: path.map(str::to_string),
        cookie_header: secrets.cookie_header.clone(),
        proxy_url: secrets.proxy_url.clone(),
        allow_invalid_tls: form.allow_invalid_tls.is_some(),
    };
    if let Err(error) = state.fetch.for_acquisition(policy) {
        return render_page(
            &state,
            &user,
            jar,
            None,
            Some(format!("Invalid profile: {error}")),
        )
        .await;
    }
    let encrypted = if secrets.cookie_header.is_some() || secrets.proxy_url.is_some() {
        Some(
            state
                .acquisition_credentials
                .encrypt_str(&serde_json::to_string(&secrets).map_err(|_| {
                    AppError::Internal(anyhow::anyhow!("secret serialization failed"))
                })?)
                .map_err(|_| AppError::Internal(anyhow::anyhow!("secret encryption failed")))?,
        )
    } else {
        None
    };
    let expires_at = if form.expires_days.trim().is_empty() {
        None
    } else {
        let days = form
            .expires_days
            .parse::<i64>()
            .map_err(|_| AppError::Validation("Expiry must be a number of days.".into()))?;
        if !(1..=365).contains(&days) {
            return render_page(
                &state,
                &user,
                jar,
                None,
                Some("Expiry must be between 1 and 365 days.".into()),
            )
            .await;
        }
        Some(OffsetDateTime::now_utc() + Duration::days(days))
    };
    if (secrets.cookie_header.is_some() || form.allow_invalid_tls.is_some()) && expires_at.is_none()
    {
        return render_page(
            &state,
            &user,
            jar,
            None,
            Some(
                "Cookie and invalid-certificate profiles require an expiry of 1 to 365 days."
                    .into(),
            ),
        )
        .await;
    }
    acquisition_profiles::create(
        state.db.pool(),
        user.id,
        name,
        &hosts,
        path,
        encrypted.as_deref(),
        secrets.cookie_header.is_some(),
        secrets.proxy_url.is_some(),
        form.allow_invalid_tls.is_some(),
        expires_at,
    )
    .await?;
    render_page(
        &state,
        &user,
        jar,
        Some(
            "Acquisition profile created. Secrets are encrypted and will not be shown again."
                .into(),
        ),
        None,
    )
    .await
}

pub async fn assign(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Form(form): Form<AssignForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let profile_id = if form.profile_id.trim().is_empty() {
        None
    } else {
        Some(
            Uuid::parse_str(&form.profile_id)
                .map_err(|_| AppError::Validation("Invalid profile.".into()))?,
        )
    };
    if feeds::find_by_id(state.db.pool(), form.feed_id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound);
    }
    acquisition_profiles::assign(state.db.pool(), user.id, form.feed_id, profile_id).await?;
    render_page(
        &state,
        &user,
        jar,
        Some("Shared feed acquisition policy updated.".into()),
        None,
    )
    .await
}

pub async fn delete(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if !acquisition_profiles::delete(state.db.pool(), user.id, id).await? {
        return Err(AppError::NotFound);
    }
    render_page(
        &state,
        &user,
        jar,
        Some(
            "Acquisition profile deleted; assigned feeds returned to the safe public client."
                .into(),
        ),
        None,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_path_boundaries_are_absolute_and_non_ambiguous() {
        assert!(valid_path_boundary("/members/feed"));
        assert!(valid_path_boundary("/"));
        for invalid in ["members", "/../admin", "/feed?token=x", "/feed#part"] {
            assert!(!valid_path_boundary(invalid), "{invalid} must be rejected");
        }
    }

    #[test]
    fn acquisition_hosts_are_bounded_dns_names() {
        assert_eq!(
            parse_hosts("Example.test, cdn.example.test, example.test").unwrap(),
            vec!["example.test", "cdn.example.test"]
        );
        for invalid in ["", "https://example.test", "example.test:443"] {
            assert!(parse_hosts(invalid).is_err());
        }
    }
}
