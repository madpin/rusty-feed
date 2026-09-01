//! OPML subscription import/export web flow.
//!
//! Imports preview without writing, then apply selected subscriptions through
//! the shared feed repository and personal subscription transaction.

use std::collections::{BTreeMap, HashMap, HashSet};

use askama::Template;
use axum::extract::{Multipart, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use time::OffsetDateTime;
use uuid::Uuid;

use rustss_db::{categories, feeds, subscriptions};
use rustss_feeds::{
    enqueue_fetch_feed, export_opml, normalize_url, parse_opml, OpmlExportEntry, OpmlSubscription,
};

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

const MAX_OPML_BYTES: usize = 2 * 1024 * 1024;
const MAX_OPML_SUBSCRIPTIONS: usize = 2_000;
const MAX_IMPORT_FORM_BYTES: usize = 8 * 1024 * 1024;
const MAX_FAILURE_DETAILS: usize = 20;

pub(crate) const PREVIEW_BODY_LIMIT: usize = MAX_OPML_BYTES + 64 * 1024;
pub(crate) const IMPORT_BODY_LIMIT: usize = MAX_IMPORT_FORM_BYTES;

struct PreviewFeed {
    payload: String,
    title: String,
    url: String,
    existing: bool,
}

struct PreviewGroup {
    id: String,
    name: String,
    feeds: Vec<PreviewFeed>,
}

#[derive(Template)]
#[template(path = "opml_preview.html")]
struct PreviewTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: crate::prefs::Preferences,
    is_admin: bool,
    document_title: String,
    groups: Vec<PreviewGroup>,
    valid_count: usize,
    new_count: usize,
    existing_count: usize,
    skipped_count: usize,
    skipped_examples: Vec<String>,
}

struct ImportFailure {
    title: String,
    reason: String,
}

#[derive(Default)]
struct ImportReport {
    selected: usize,
    created: usize,
    updated: usize,
    unchanged: usize,
    failed: usize,
    folders_created: usize,
    refreshes_queued: usize,
    refreshes_already_queued: usize,
    failures: Vec<ImportFailure>,
}

#[derive(Template)]
#[template(path = "opml_result.html")]
struct ResultTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: crate::prefs::Preferences,
    is_admin: bool,
    report: ImportReport,
    additional_failures: usize,
}

#[derive(Template)]
#[template(path = "opml_error.html")]
struct ErrorTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: crate::prefs::Preferences,
    is_admin: bool,
    error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingPolicy {
    Keep,
    Replace,
}

struct ImportForm {
    csrf_token: String,
    policy: ExistingPolicy,
    entries: Vec<OpmlSubscription>,
}

struct CategoryCache {
    by_path_segment: HashMap<(Option<Uuid>, String), Uuid>,
    created: usize,
}

impl CategoryCache {
    async fn load(state: &AppState, user_id: Uuid) -> Result<Self, AppError> {
        let categories = categories::list_for_user(state.db.pool(), user_id).await?;
        Ok(Self {
            by_path_segment: categories
                .into_iter()
                .map(|category| ((category.parent_id, category.name), category.id))
                .collect(),
            created: 0,
        })
    }

    async fn ensure_path(
        &mut self,
        state: &AppState,
        user_id: Uuid,
        path: &[String],
    ) -> Result<Option<Uuid>, AppError> {
        let mut parent_id = None;
        for name in path {
            let key = (parent_id, name.clone());
            if let Some(category_id) = self.by_path_segment.get(&key) {
                parent_id = Some(*category_id);
                continue;
            }
            // OPML folders create category rows — subject to the quota (FND-188).
            let quotas = &state.settings.quotas;
            let kind = rustss_core::quotas::QuotaKind::Categories;
            if quotas.limit(kind) > 0 {
                let count = categories::count_for_user(state.db.pool(), user_id).await?;
                if !quotas.allows(kind, count) {
                    return Err(AppError::QuotaExceeded(quotas.exceeded_message(kind)));
                }
            }
            let category = categories::create(state.db.pool(), user_id, name, parent_id).await?;
            self.by_path_segment.insert(key, category.id);
            parent_id = Some(category.id);
            self.created += 1;
        }
        Ok(parent_id)
    }
}

/// `POST /feeds/opml/preview` — parse a local file or safely fetched URL without writing.
pub async fn preview(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    if !state.opml_limiter.check(&user.id.to_string()).await {
        return Err(AppError::RateLimited);
    }
    crate::verify::deny_demo(&user)?;
    let mut csrf_token = None;
    let mut opml_url = None;
    let mut file = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| AppError::Validation("The OPML upload could not be read.".to_string()))?
    {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "csrf_token" if csrf_token.is_none() => {
                csrf_token = Some(field.text().await.map_err(|_| {
                    AppError::Validation("The OPML upload could not be read.".to_string())
                })?);
            }
            "opml_url" if opml_url.is_none() => {
                opml_url = Some(field.text().await.map_err(|_| {
                    AppError::Validation("The OPML URL could not be read.".to_string())
                })?);
            }
            "opml_file" if file.is_none() => {
                let bytes = field.bytes().await.map_err(|_| {
                    AppError::Validation("The OPML file could not be read.".to_string())
                })?;
                if !bytes.is_empty() {
                    file = Some(bytes.to_vec());
                }
            }
            _ => {}
        }
    }

    let csrf_token = csrf_token.ok_or(AppError::Forbidden)?;
    if !csrf::verify(&jar, &csrf_token) {
        return Err(AppError::Forbidden);
    }
    let opml_url = opml_url.unwrap_or_default();
    let opml_url = opml_url.trim();
    if file.is_some() != opml_url.is_empty() {
        return render_source_error(
            &state,
            &user,
            jar,
            "Choose either one OPML file or one OPML URL.".to_string(),
        )
        .await;
    }

    let bytes = if let Some(file) = file {
        if file.len() > MAX_OPML_BYTES {
            return render_source_error(
                &state,
                &user,
                jar,
                format!(
                    "OPML files must be no larger than {} MiB.",
                    MAX_OPML_BYTES / 1024 / 1024
                ),
            )
            .await;
        }
        file
    } else {
        if opml_url.chars().count() > 8_192 {
            return render_source_error(
                &state,
                &user,
                jar,
                "The OPML URL is too long.".to_string(),
            )
            .await;
        }
        match state.fetch.get_bytes(opml_url, MAX_OPML_BYTES).await {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(error = %error, "OPML URL import failed");
                return render_source_error(
                    &state,
                    &user,
                    jar,
                    "The OPML URL could not be fetched safely. Check that it is public and returns a valid document."
                        .to_string(),
                )
                .await;
            }
        }
    };

    let document = match parse_opml(&bytes, MAX_OPML_SUBSCRIPTIONS) {
        Ok(document) => document,
        Err(error) => return render_source_error(&state, &user, jar, error.to_string()).await,
    };
    let blocklist = state.blocklist().await?;
    let mut seen = HashSet::new();
    let mut normalized_subscriptions = Vec::new();
    let mut skipped_count = 0;
    let mut skipped_examples = Vec::new();
    for mut subscription in document.subscriptions {
        let normalized = match normalize_url(&subscription.xml_url) {
            Ok(url) if state.fetch.validate_url(&url).is_ok() && !blocklist.is_blocked(&url) => url,
            _ => {
                skipped_count += 1;
                push_skipped(
                    &mut skipped_examples,
                    format!("{} — invalid or unsafe feed URL", subscription.title),
                );
                continue;
            }
        };
        if !seen.insert(normalized.clone()) {
            skipped_count += 1;
            push_skipped(
                &mut skipped_examples,
                format!("{} — duplicate feed URL", subscription.title),
            );
            continue;
        }
        subscription.xml_url = normalized;
        subscription.html_url = subscription.html_url.as_deref().and_then(public_http_url);
        normalized_subscriptions.push(subscription);
    }
    let urls: Vec<_> = normalized_subscriptions
        .iter()
        .map(|subscription| subscription.xml_url.clone())
        .collect();
    let existing_urls: HashSet<_> = subscriptions::subscribed_urls(state.db.pool(), user.id, &urls)
        .await?
        .into_iter()
        .collect();
    let mut groups: BTreeMap<String, Vec<PreviewFeed>> = BTreeMap::new();
    let mut existing_count = 0;
    for subscription in normalized_subscriptions {
        let group_name = if subscription.category_path.is_empty() {
            "Unfiled".to_string()
        } else {
            subscription.category_path.join(" / ")
        };
        let existing = existing_urls.contains(&subscription.xml_url);
        if existing {
            existing_count += 1;
        }
        let payload = encode_entry(&subscription)?;
        groups.entry(group_name).or_default().push(PreviewFeed {
            payload,
            title: subscription.title,
            url: subscription.xml_url,
            existing,
        });
    }
    let groups: Vec<_> = groups
        .into_iter()
        .enumerate()
        .map(|(index, (name, feeds))| PreviewGroup {
            id: format!("opml-group-{index}"),
            name,
            feeds,
        })
        .collect();
    let valid_count = groups.iter().map(|group| group.feeds.len()).sum();
    let new_count = valid_count - existing_count;
    let ui = crate::prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = PreviewTemplate {
        theme: ui.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui,
        is_admin: user.role == "admin",
        document_title: document
            .title
            .unwrap_or_else(|| "Imported subscriptions".to_string()),
        groups,
        valid_count,
        new_count,
        existing_count,
        skipped_count,
        skipped_examples,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    Ok((jar, body).into_response())
}

/// `POST /feeds/opml/import` — apply only the subscriptions selected in preview.
pub async fn import(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(values): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    if !state.opml_limiter.check(&user.id.to_string()).await {
        return Err(AppError::RateLimited);
    }
    // Demo accounts add feeds only from the curated catalog, not via OPML — this
    // keeps arbitrary feeds out of the shared tables and respects the demo cap
    // (§7.4). Blocked here, not just at the per-entry level.
    crate::verify::deny_demo(&user)?;
    crate::verify::require_verified(&state, &user)?;
    crate::quota::assert_within(
        &state,
        user.id,
        rustss_core::quotas::QuotaKind::Subscriptions,
    )
    .await?;
    let form = match parse_import_form(values) {
        Ok(form) => form,
        Err(AppError::Validation(error)) => {
            return render_source_error(&state, &user, jar, error).await;
        }
        Err(error) => return Err(error),
    };
    if !csrf::verify(&jar, &form.csrf_token) {
        return Err(AppError::Forbidden);
    }

    let mut report = ImportReport {
        selected: form.entries.len(),
        ..ImportReport::default()
    };
    let mut categories = CategoryCache::load(&state, user.id).await?;
    for entry in form.entries {
        if let Err(error) = import_entry(
            &state,
            user.id,
            &mut categories,
            &entry,
            form.policy,
            &mut report,
        )
        .await
        {
            report.failed += 1;
            tracing::warn!(feed_url = %entry.xml_url, error = %error, "OPML entry import failed");
            if report.failures.len() < MAX_FAILURE_DETAILS {
                report.failures.push(ImportFailure {
                    title: entry.title,
                    reason:
                        "RustSS could not save this subscription. You can retry it individually."
                            .to_string(),
                });
            }
        }
    }
    report.folders_created = categories.created;
    let additional_failures = report.failed.saturating_sub(report.failures.len());
    render_result(&state, user, jar, report, additional_failures).await
}

async fn import_entry(
    state: &AppState,
    user_id: Uuid,
    categories: &mut CategoryCache,
    entry: &OpmlSubscription,
    policy: ExistingPolicy,
    report: &mut ImportReport,
) -> Result<(), AppError> {
    let normalized = normalize_url(&entry.xml_url)
        .map_err(|_| AppError::Validation("The selected feed URL is invalid.".to_string()))?;
    state
        .fetch
        .validate_url(&normalized)
        .map_err(|_| AppError::Validation("The selected feed URL is unsafe.".to_string()))?;
    crate::moderation::assert_feed_allowed(state, &normalized).await?;
    let host = url::Url::parse(&normalized)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "Feed".to_string());
    let html_url = entry.html_url.as_deref().and_then(public_http_url);
    let (feed, _) =
        feeds::upsert_by_url(state.db.pool(), &normalized, &host, html_url.as_deref()).await?;

    let existing = subscriptions::find(state.db.pool(), user_id, feed.id).await?;
    if existing.is_some() && policy == ExistingPolicy::Keep {
        report.unchanged += 1;
        return Ok(());
    }
    let category_id = categories
        .ensure_path(state, user_id, &entry.category_path)
        .await?;
    let outcome = subscriptions::import_subscription(
        state.db.pool(),
        user_id,
        feed.id,
        Some(&entry.title),
        category_id,
        policy == ExistingPolicy::Replace,
    )
    .await?;
    match outcome {
        subscriptions::ImportSubscriptionOutcome::Created => {
            report.created += 1;
            match enqueue_fetch_feed(&state.db, feed.id).await? {
                Some(_) => report.refreshes_queued += 1,
                None => report.refreshes_already_queued += 1,
            }
        }
        subscriptions::ImportSubscriptionOutcome::Updated => report.updated += 1,
        subscriptions::ImportSubscriptionOutcome::Unchanged => report.unchanged += 1,
    }
    Ok(())
}

/// `GET /feeds/opml/export` — download the caller's personal subscription tree.
pub async fn export(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    let (feed_rows, subscription_rows, category_rows, organization_rows) = tokio::try_join!(
        feeds::list_for_user(state.db.pool(), user.id),
        subscriptions::list_for_user(state.db.pool(), user.id),
        categories::list_for_user(state.db.pool(), user.id),
        subscriptions::list_organization(state.db.pool(), user.id),
    )?;
    let subscriptions: HashMap<_, _> = subscription_rows
        .into_iter()
        .map(|subscription| (subscription.feed_id, subscription))
        .collect();
    let organizations: HashMap<_, _> = organization_rows
        .into_iter()
        .map(|organization| (organization.feed_id, organization.category_id))
        .collect();
    let category_paths = category_paths(&category_rows);
    let entries = feed_rows.into_iter().map(|feed| {
        let subscription = subscriptions.get(&feed.id);
        let title = subscription
            .and_then(|subscription| subscription.custom_name.clone())
            .unwrap_or_else(|| feed.name.clone());
        let category_path = organizations
            .get(&feed.id)
            .copied()
            .flatten()
            .and_then(|category_id| category_paths.get(&category_id).cloned())
            .unwrap_or_default();
        OpmlExportEntry {
            title,
            xml_url: feed.url,
            html_url: feed.site_url.and_then(|url| public_http_url(&url)),
            category_path,
        }
    });
    let document = export_opml("RustSS subscriptions", entries);
    let filename = format!(
        "rustss-subscriptions-{}.opml",
        OffsetDateTime::now_utc().date()
    );
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
        .map_err(|error| AppError::Internal(anyhow::Error::new(error)))?;
    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/x-opml; charset=utf-8"),
            ),
            (header::CONTENT_DISPOSITION, disposition),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, no-store"),
            ),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        document,
    )
        .into_response())
}

fn parse_import_form(values: Vec<(String, String)>) -> Result<ImportForm, AppError> {
    let mut csrf_token = None;
    let mut policy = None;
    let mut entries = Vec::new();
    for (name, value) in values {
        match name.as_str() {
            "csrf_token" if csrf_token.is_none() => csrf_token = Some(value),
            "existing_policy" if policy.is_none() => {
                policy = Some(match value.as_str() {
                    "keep" => ExistingPolicy::Keep,
                    "replace" => ExistingPolicy::Replace,
                    _ => {
                        return Err(AppError::Validation(
                            "Choose how existing subscriptions should be handled.".to_string(),
                        ));
                    }
                });
            }
            "entry" => {
                if entries.len() >= MAX_OPML_SUBSCRIPTIONS {
                    return Err(AppError::Validation(format!(
                        "Choose at most {MAX_OPML_SUBSCRIPTIONS} subscriptions."
                    )));
                }
                entries.push(decode_entry(&value)?);
            }
            _ => {
                return Err(AppError::Validation(
                    "The OPML import selection could not be processed.".to_string(),
                ));
            }
        }
    }
    if entries.is_empty() {
        return Err(AppError::Validation(
            "Choose at least one subscription to import.".to_string(),
        ));
    }
    Ok(ImportForm {
        csrf_token: csrf_token.ok_or(AppError::Forbidden)?,
        policy: policy.unwrap_or(ExistingPolicy::Keep),
        entries,
    })
}

async fn render_result(
    state: &AppState,
    user: rustss_db::User,
    jar: CookieJar,
    report: ImportReport,
    additional_failures: usize,
) -> Result<Response, AppError> {
    let ui = crate::prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = ResultTemplate {
        theme: ui.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui,
        is_admin: user.role == "admin",
        report,
        additional_failures,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    Ok((jar, body).into_response())
}

async fn render_source_error(
    state: &AppState,
    user: &rustss_db::User,
    jar: CookieJar,
    error: String,
) -> Result<Response, AppError> {
    let ui = crate::prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = ErrorTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui,
        is_admin: user.role == "admin",
        error,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    Ok((jar, body).into_response())
}

fn encode_entry(entry: &OpmlSubscription) -> Result<String, AppError> {
    let json = serde_json::to_vec(entry).map_err(|error| AppError::Internal(error.into()))?;
    Ok(URL_SAFE_NO_PAD.encode(json))
}

fn decode_entry(value: &str) -> Result<OpmlSubscription, AppError> {
    let bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| {
        AppError::Validation("One selected OPML subscription was invalid.".to_string())
    })?;
    let entry: OpmlSubscription = serde_json::from_slice(&bytes).map_err(|_| {
        AppError::Validation("One selected OPML subscription was invalid.".to_string())
    })?;
    if entry.title.is_empty()
        || entry.title.chars().count() > 200
        || entry.title.chars().any(char::is_control)
        || entry.xml_url.is_empty()
        || entry.xml_url.chars().count() > 8_192
        || entry.xml_url.chars().any(char::is_control)
        || entry.html_url.as_ref().is_some_and(|url| {
            url.is_empty() || url.chars().count() > 8_192 || url.chars().any(char::is_control)
        })
        || entry.category_path.len() > 32
        || entry.category_path.iter().any(|segment| {
            segment.is_empty()
                || segment.chars().count() > 80
                || segment.chars().any(char::is_control)
        })
    {
        return Err(AppError::Validation(
            "One selected OPML subscription was invalid.".to_string(),
        ));
    }
    Ok(entry)
}

fn public_http_url(value: &str) -> Option<String> {
    let mut url = url::Url::parse(value.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    url.set_fragment(None);
    Some(url.to_string())
}

fn push_skipped(skipped: &mut Vec<String>, value: String) {
    if skipped.len() < MAX_FAILURE_DETAILS {
        skipped.push(value);
    }
}

fn category_paths(categories: &[categories::UserCategory]) -> HashMap<Uuid, Vec<String>> {
    let by_id: HashMap<_, _> = categories
        .iter()
        .map(|category| (category.id, category))
        .collect();
    categories
        .iter()
        .map(|category| {
            let mut path = Vec::new();
            let mut current = Some(category.id);
            let mut visited = HashSet::new();
            while let Some(category_id) = current {
                let Some(category) = by_id.get(&category_id) else {
                    break;
                };
                if !visited.insert(category_id) {
                    break;
                }
                path.push(category.name.clone());
                current = category.parent_id;
            }
            path.reverse();
            (category.id, path)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_form_decodes_repeated_selected_entries() {
        let first = OpmlSubscription {
            title: "One".to_string(),
            xml_url: "https://one.example/feed".to_string(),
            html_url: None,
            category_path: vec!["News".to_string()],
        };
        let second = OpmlSubscription {
            title: "Two".to_string(),
            xml_url: "https://two.example/feed".to_string(),
            html_url: None,
            category_path: Vec::new(),
        };
        let form = parse_import_form(vec![
            ("csrf_token".to_string(), "token".to_string()),
            ("existing_policy".to_string(), "replace".to_string()),
            ("entry".to_string(), encode_entry(&first).unwrap()),
            ("entry".to_string(), encode_entry(&second).unwrap()),
        ])
        .unwrap();
        assert_eq!(form.policy, ExistingPolicy::Replace);
        assert_eq!(form.entries, vec![first, second]);
    }

    #[test]
    fn category_paths_are_cycle_safe() {
        let root_id = Uuid::now_v7();
        let child_id = Uuid::now_v7();
        let categories = vec![
            categories::UserCategory {
                id: root_id,
                name: "Root".to_string(),
                parent_id: None,
                sort_order: 0,
                collapsed: false,
            },
            categories::UserCategory {
                id: child_id,
                name: "Child".to_string(),
                parent_id: Some(root_id),
                sort_order: 0,
                collapsed: false,
            },
        ];
        assert_eq!(
            category_paths(&categories).get(&child_id),
            Some(&vec!["Root".to_string(), "Child".to_string()])
        );
    }
}
