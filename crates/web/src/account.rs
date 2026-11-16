//! Account & data self-service: full data export and account deletion
//! (docs/PUBLIC_LAUNCH_PLAN.md §9, FND-195). Account deletion cascades all
//! per-user rows; shared content is never removed by it.

use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use serde_json::json;

use rusty_feed_db::{categories, feeds, saved_searches, users};

use crate::auth::extractor::AuthUser;
use crate::auth::{csrf, session};
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

#[derive(Template)]
#[template(path = "account.html")]
struct AccountTemplate {
    theme: String,
    user_email: String,
    ui: prefs::Preferences,
    is_admin: bool,
    csrf_token: String,
    email_verified: bool,
    verification_required: bool,
    notice: Option<String>,
    error: Option<String>,
}

async fn render(
    state: &AppState,
    user: &rusty_feed_db::User,
    jar: CookieJar,
    notice: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = AccountTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        ui,
        is_admin: user.role == "admin",
        csrf_token,
        email_verified: user.email_verified_at.is_some() || user.password_hash.is_none(),
        verification_required: state.settings.email_verification_required,
        notice,
        error,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, Html(body)).into_response())
}

/// `GET /settings/account` — export/delete controls.
pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    render(&state, &user, jar, None, None).await
}

/// `POST /settings/account/export` — download all personal data as JSON.
pub async fn export(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    let pool = state.db.pool();
    let preferences = prefs::load(&state.db, user.id).await;
    let subscriptions: Vec<_> = feeds::list_for_user(pool, user.id)
        .await?
        .into_iter()
        .map(|feed| json!({ "url": feed.url, "name": feed.name, "site_url": feed.site_url }))
        .collect();
    let categories: Vec<_> = categories::list_for_user(pool, user.id)
        .await?
        .into_iter()
        .map(|category| json!({ "name": category.name }))
        .collect();
    let searches: Vec<_> = saved_searches::list_for_user(pool, user.id)
        .await?
        .into_iter()
        .map(|search| json!({ "name": search.name, "query": search.query }))
        .collect();
    // Reading state: the user's bookmarked articles (§9 full export).
    let bookmarks: Vec<_> = rusty_feed_db::bookmarks::list_for_export(pool, user.id, 100_000)
        .await?
        .into_iter()
        .map(|bookmark| {
            json!({
                "url": bookmark.url,
                "title": bookmark.title,
                "bookmarked_at": bookmark.bookmarked_at.to_string(),
            })
        })
        .collect();
    let export = json!({
        "exported_at": time::OffsetDateTime::now_utc().to_string(),
        "account": {
            "email": user.email,
            "name": user.name,
            "created_at": user.created_at.to_string(),
            "email_verified": user.email_verified_at.is_some(),
        },
        "preferences": preferences,
        "subscriptions": subscriptions,
        "categories": categories,
        "saved_searches": searches,
        "bookmarks": bookmarks,
        "note": "Full OPML of your feeds is also available at /feeds/opml/export.",
    });
    let body = serde_json::to_vec_pretty(&export)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("export serialization failed: {e}")))?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"rusty-feed-export.json\"",
            ),
        ],
        body,
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct DeleteForm {
    csrf_token: String,
    #[serde(default)]
    confirm: String,
}

/// `POST /settings/account/delete` — permanently delete the account.
pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(form): Form<DeleteForm>,
) -> Result<Response, AppError> {
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }
    if form.confirm.trim() != "DELETE" {
        return render(
            &state,
            &user,
            jar,
            None,
            Some("Type DELETE to confirm account deletion.".to_string()),
        )
        .await;
    }
    if !users::delete(state.db.pool(), user.id).await? {
        return render(
            &state,
            &user,
            jar,
            None,
            Some("You are the last administrator and cannot delete this account. Promote another admin first.".to_string()),
        )
        .await;
    }
    crate::search::invalidate_user_results(&state, user.id).await;
    state.invalidate_unread_counts(user.id).await;
    let jar = session::logout(jar, &state.db).await;
    Ok((jar, Redirect::to("/login?deleted=1")).into_response())
}
