//! First-run setup wizard (docs/PUBLIC_LAUNCH_PLAN.md §8.4). New users pick a
//! layout, theme, and typography with a live preview (CSP-safe: the embedded
//! app.js swaps data-attributes, no inline styles), tell us which topics they
//! care about, and can subscribe to a curated set of feeds in one step.
//! Skippable and re-runnable.

use std::collections::{BTreeSet, HashMap};

use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde_json::{json, Value};
use uuid::Uuid;

use rusty_feed_db::{catalog, feed_icons, subscriptions, user_preferences, users, User};
use rusty_feed_feeds::enqueue_fetch_feed;

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

/// Overall cap on curated feeds shown in the wizard, plus a per-topic spread so
/// every interest chip has at least one source to filter to.
const WIZARD_SUGGESTIONS_TOTAL: usize = 48;
const WIZARD_SUGGESTIONS_PER_CATEGORY: usize = 6;
/// Cap on feeds a user can subscribe to from a single wizard submission.
const WIZARD_MAX_SUBSCRIPTIONS: usize = 30;

/// A curated feed suggestion rendered as a selectable card in the wizard.
struct WizardFeedView {
    feed_id: String,
    title: String,
    description: Option<String>,
    category: String,
    initial: String,
    has_icon: bool,
    featured: bool,
}

/// A topic chip with how many suggested feeds currently match it, so picking
/// one is predictable instead of a surprise (the count is exactly what the
/// live filter in `app.js` will show).
struct WizardInterestView {
    category: String,
    count: usize,
}

#[derive(Template)]
#[template(path = "welcome.html")]
struct WelcomeTemplate {
    theme: String,
    user_email: String,
    ui: prefs::Preferences,
    is_admin: bool,
    is_demo: bool,
    csrf_token: String,
    /// Distinct topics (catalog categories) present in `suggestions`, used for
    /// the interest chips that live-filter the suggestion grid.
    categories: Vec<WizardInterestView>,
    suggestions: Vec<WizardFeedView>,
    /// Titles of featured feeds already present in a demo library, so a demo
    /// user who reopens setup can see where those showcase sources came from.
    starter_feeds: Vec<String>,
    /// Compact, human-readable preview of `starter_feeds` (a few names, plus
    /// a "and N more" tail).
    starter_feeds_preview: String,
}

/// Loaded suggestion data for the wizard's feed-picking step.
struct WizardData {
    suggestions: Vec<WizardFeedView>,
    categories: Vec<WizardInterestView>,
    starter_feeds: Vec<String>,
}

fn feed_initial(name: &str) -> String {
    name.chars()
        .find(|character| character.is_alphanumeric())
        .map(|character| character.to_uppercase().collect())
        .unwrap_or_else(|| "•".to_string())
}

/// Trim a catalog description to a card-friendly length.
fn short_description(text: Option<String>) -> Option<String> {
    let text = text?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out: String = trimmed.chars().take(160).collect();
    if trimmed.chars().count() > 160 {
        out.push('…');
    }
    Some(out)
}

/// Pick a compact, topic-diverse set of curated feeds the user does not already
/// follow. Entries arrive featured-first; we round-robin across categories so a
/// single popular topic cannot crowd out the rest under the overall cap. Also
/// collects the already-subscribed featured ("starter") feeds so the wizard
/// can explain where they came from instead of staying silent about them.
async fn load_suggestions(state: &AppState, user_id: Uuid) -> Result<WizardData, AppError> {
    let entries =
        catalog::list_published_for_user(state.db.pool(), user_id, None, None, None).await?;
    let feed_ids: Vec<Uuid> = entries.iter().map(|entry| entry.feed_id).collect();
    let icon_ids = feed_icons::available_for_feeds(state.db.pool(), &feed_ids).await?;

    let mut starter_feeds: Vec<String> = Vec::new();
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<catalog::CatalogEntry>> = HashMap::new();
    for entry in entries {
        if entry.subscribed {
            // Already in the library. Only call out the instance's featured
            // defaults (applied automatically at signup) — a feed the user
            // subscribed to themselves is not a surprise to explain.
            if entry.featured {
                starter_feeds.push(entry.title.clone());
            }
            continue;
        }
        if !groups.contains_key(&entry.category) {
            order.push(entry.category.clone());
        }
        groups
            .entry(entry.category.clone())
            .or_default()
            .push(entry);
    }
    starter_feeds.sort();

    let mut suggestions = Vec::new();
    'rounds: for round in 0..WIZARD_SUGGESTIONS_PER_CATEGORY {
        for category in &order {
            let Some(entry) = groups.get(category).and_then(|items| items.get(round)) else {
                continue;
            };
            suggestions.push(WizardFeedView {
                feed_id: entry.feed_id.to_string(),
                title: entry.title.clone(),
                description: short_description(entry.description.clone()),
                category: entry.category.clone(),
                initial: feed_initial(&entry.title),
                has_icon: icon_ids.contains(&entry.feed_id),
                featured: entry.featured,
            });
            if suggestions.len() >= WIZARD_SUGGESTIONS_TOTAL {
                break 'rounds;
            }
        }
    }

    // Count the feeds actually rendered per category (after the overall cap),
    // so each topic chip promises exactly what selecting it will show.
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for suggestion in &suggestions {
        *counts.entry(suggestion.category.as_str()).or_insert(0) += 1;
    }
    let categories: Vec<WizardInterestView> = order
        .into_iter()
        .filter_map(|category| {
            let count = *counts.get(category.as_str())?;
            Some(WizardInterestView { category, count })
        })
        .collect();

    Ok(WizardData {
        suggestions,
        categories,
        starter_feeds,
    })
}

/// "A, B, and N more" (or the full list when it already fits in `show`).
fn preview_names(names: &[String], show: usize) -> String {
    if names.len() <= show {
        return names.join(", ");
    }
    format!(
        "{}, and {} more",
        names[..show].join(", "),
        names.len() - show
    )
}

/// `GET /welcome` — the wizard (also reachable via "Re-run setup").
pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let ui = prefs::load(&state.db, user.id).await;
    let data = load_suggestions(&state, user.id).await?;
    let starter_feeds_preview = preview_names(&data.starter_feeds, 3);
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = WelcomeTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        ui,
        is_admin: user.role == "admin",
        is_demo: user.is_demo,
        csrf_token,
        categories: data.categories,
        suggestions: data.suggestions,
        starter_feeds: data.starter_feeds,
        starter_feeds_preview,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, Html(body)).into_response())
}

/// Subscribe the user to the feeds picked in the wizard. Best-effort: the same
/// verification/quota/demo guards as the catalog apply, but a failure here never
/// aborts onboarding (the user still lands in the reader and can subscribe later).
async fn subscribe_wizard_feeds(state: &AppState, user: &User, feed_ids: &BTreeSet<Uuid>) {
    if feed_ids.is_empty() {
        return;
    }
    if crate::verify::require_verified(state, user).is_err() {
        return;
    }
    if crate::quota::assert_within(
        state,
        user.id,
        rusty_feed_core::quotas::QuotaKind::Subscriptions,
    )
    .await
    .is_err()
    {
        return;
    }
    let mut ids: Vec<Uuid> = feed_ids.iter().copied().collect();
    ids.truncate(WIZARD_MAX_SUBSCRIPTIONS);
    // Demo sandboxes get a tighter cap (§7.4): only fill the remaining allowance.
    if user.is_demo && state.settings.demo_max_subscriptions > 0 {
        let existing = subscriptions::count_for_user(state.db.pool(), user.id)
            .await
            .unwrap_or(0);
        let remaining =
            (i64::from(state.settings.demo_max_subscriptions) - existing).max(0) as usize;
        ids.truncate(remaining);
    }
    if ids.is_empty() {
        return;
    }
    if let Ok(result) = catalog::subscribe_published_many(state.db.pool(), user.id, &ids).await {
        for feed_id in &ids {
            let _ = enqueue_fetch_feed(&state.db, *feed_id).await;
        }
        // Best-effort: file each newly subscribed feed under a folder/tag named
        // for its catalog category, so its already-shared articles show up
        // organized right away instead of landing "Unfiled".
        let _ =
            catalog::organize_new_subscriptions(state.db.pool(), user.id, &result.created_feed_ids)
                .await;
    }
}

/// `POST /welcome` — persist selections (unless skipped), subscribe to any picked
/// feeds, and stamp `onboarded_at`. Parsed as raw pairs so the repeated `interest`
/// and `feed_id` checkboxes come through.
pub async fn submit(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(values): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let mut csrf_token = String::new();
    let mut skip = false;
    let mut theme = String::new();
    let mut reading_mode = String::new();
    let mut font_family = String::new();
    let mut article_density = String::new();
    let mut interests: Vec<String> = Vec::new();
    let mut feed_ids: BTreeSet<Uuid> = BTreeSet::new();
    for (key, value) in values {
        match key.as_str() {
            "csrf_token" => csrf_token = value,
            "skip" => skip = matches!(value.trim(), "1" | "true" | "on"),
            "theme" => theme = value,
            "reading_mode" => reading_mode = value,
            "font_family" => font_family = value,
            "article_density" => article_density = value,
            "interest" => interests.push(value),
            "feed_id" => {
                if let Ok(id) = Uuid::parse_str(value.trim()) {
                    feed_ids.insert(id);
                }
            }
            _ => {}
        }
    }

    if !csrf::verify(&jar, &csrf_token) {
        return Err(AppError::Forbidden);
    }

    if !skip {
        let mut patch = serde_json::Map::new();
        if matches!(
            theme.as_str(),
            "system" | "light" | "dark" | "rusty" | "ocean" | "forest" | "plum" | "sunset"
        ) {
            patch.insert("theme".to_string(), json!(theme));
        }
        if matches!(
            reading_mode.as_str(),
            "side-panel" | "standalone" | "inline"
        ) {
            patch.insert("reading_mode".to_string(), json!(reading_mode));
        }
        if matches!(font_family.as_str(), "sans" | "serif") {
            patch.insert("font_family".to_string(), json!(font_family));
        }
        if matches!(
            article_density.as_str(),
            "compact" | "comfortable" | "spacious"
        ) {
            patch.insert("article_density".to_string(), json!(article_density));
        }
        let interests = sanitize_interests(interests);
        if !interests.is_empty() {
            patch.insert("interests".to_string(), json!(interests));
        }
        if !patch.is_empty() {
            user_preferences::upsert(state.db.pool(), user.id, &Value::Object(patch)).await?;
        }
        subscribe_wizard_feeds(&state, &user, &feed_ids).await;
    }

    users::mark_onboarded(state.db.pool(), user.id).await?;
    Ok((jar, Redirect::to("/articles")).into_response())
}

/// Trim, de-duplicate (preserving order), and cap the submitted interests.
fn sanitize_interests(interests: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    interests
        .into_iter()
        .filter_map(|interest| {
            let trimmed = interest.trim();
            (!trimmed.is_empty() && seen.insert(trimmed.to_string())).then(|| trimmed.to_string())
        })
        .take(40)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_and_deduplicates_interests() {
        let interests = sanitize_interests(vec![
            " Technology ".to_string(),
            "Technology".to_string(),
            "  ".to_string(),
            "Science".to_string(),
        ]);
        assert_eq!(
            interests,
            vec!["Technology".to_string(), "Science".to_string()]
        );
    }

    #[test]
    fn preview_names_lists_everything_when_it_fits() {
        let names = vec!["Rust Blog".to_string(), "Hacker News".to_string()];
        assert_eq!(preview_names(&names, 3), "Rust Blog, Hacker News");
    }

    #[test]
    fn preview_names_truncates_with_a_count() {
        let names = [
            "Rust Blog",
            "Hacker News",
            "NASA Science",
            "arXiv AI",
            "BBC",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
        assert_eq!(
            preview_names(&names, 3),
            "Rust Blog, Hacker News, NASA Science, and 2 more"
        );
    }
}
