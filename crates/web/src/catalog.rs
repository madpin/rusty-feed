use std::collections::BTreeSet;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use rustss_db::{catalog, feed_icons, User};
use rustss_feeds::enqueue_fetch_feed;

use crate::auth::csrf;
use crate::auth::extractor::AuthUser;
use crate::catalog_view::{content_size, frequency};
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

const MAX_CATALOG_SELECTION: usize = 30;

struct Facet {
    value: String,
    selected: bool,
}

struct CatalogView {
    feed_id: String,
    initial: String,
    has_icon: bool,
    title: String,
    description: Option<String>,
    category: String,
    country: String,
    language: String,
    featured: bool,
    url: String,
    health: String,
    article_count: i64,
    average_content: String,
    last_post: String,
    frequency: String,
    subscribed: bool,
}

#[derive(Template)]
#[template(path = "catalog.html")]
struct CatalogTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    is_admin: bool,
    entries: Vec<CatalogView>,
    categories: Vec<Facet>,
    countries: Vec<Facet>,
    languages: Vec<Facet>,
    message: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
pub struct CatalogQuery {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    language: Option<String>,
}

fn selected(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn feed_initial(name: &str) -> String {
    name.chars()
        .find(|character| character.is_alphanumeric())
        .map(|character| character.to_uppercase().collect())
        .unwrap_or_else(|| "•".to_string())
}

fn view(entry: catalog::CatalogEntry, has_icon: bool) -> CatalogView {
    CatalogView {
        feed_id: entry.feed_id.to_string(),
        initial: feed_initial(&entry.title),
        has_icon,
        title: entry.title,
        description: entry.description,
        category: entry.category,
        country: entry.country,
        language: entry.language,
        featured: entry.featured,
        url: entry.url,
        health: entry.health_status,
        article_count: entry.article_count,
        average_content: content_size(entry.average_rss_content_length),
        last_post: entry
            .last_post_at
            .map(|date| date.date().to_string())
            .unwrap_or_else(|| "No dated posts yet".to_string()),
        frequency: frequency(entry.average_interval_hours),
        subscribed: entry.subscribed,
    }
}

async fn render_page(
    state: &AppState,
    user: &User,
    jar: CookieJar,
    query: CatalogQuery,
    message: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    let category = selected(query.category);
    let country = selected(query.country);
    let language = selected(query.language);
    let ui = prefs::load(&state.db, user.id).await;
    let entries = catalog::list_published_for_user(
        state.db.pool(),
        user.id,
        category.as_deref(),
        country.as_deref(),
        language.as_deref(),
    )
    .await?;
    let feed_ids: Vec<Uuid> = entries.iter().map(|entry| entry.feed_id).collect();
    let icon_ids = feed_icons::available_for_feeds(state.db.pool(), &feed_ids).await?;
    let entries = entries
        .into_iter()
        .map(|entry| {
            let has_icon = icon_ids.contains(&entry.feed_id);
            view(entry, has_icon)
        })
        .collect();
    let (categories, countries, languages) = tokio::try_join!(
        catalog::categories(state.db.pool()),
        catalog::countries(state.db.pool()),
        catalog::languages(state.db.pool()),
    )?;
    let categories = categories
        .into_iter()
        .map(|value| Facet {
            selected: category.as_deref() == Some(value.as_str()),
            value,
        })
        .collect();
    let countries = countries
        .into_iter()
        .map(|value| Facet {
            selected: country.as_deref() == Some(value.as_str()),
            value,
        })
        .collect();
    let languages = languages
        .into_iter()
        .map(|value| Facet {
            selected: language.as_deref() == Some(value.as_str()),
            value,
        })
        .collect();
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = CatalogTemplate {
        theme: ui.theme.clone(),
        user_email: user.email.clone(),
        csrf_token,
        ui,
        is_admin: user.role == "admin",
        entries,
        categories,
        countries,
        languages,
        message,
        error,
    }
    .render()
    .map(Html)
    .map_err(|error| AppError::Internal(anyhow::anyhow!("template render failed: {error}")))?;
    Ok((jar, body).into_response())
}

fn parse_subscription_form(values: Vec<(String, String)>) -> Result<(String, Vec<Uuid>), String> {
    let mut csrf_token = None;
    let mut feed_ids = BTreeSet::new();
    for (name, value) in values {
        match name.as_str() {
            "csrf_token" if csrf_token.is_none() => csrf_token = Some(value),
            "feed_id" => {
                let feed_id =
                    Uuid::parse_str(&value).map_err(|_| "The selected source was invalid.")?;
                feed_ids.insert(feed_id);
            }
            _ => return Err("The catalog selection could not be processed.".to_string()),
        }
    }
    let csrf_token =
        csrf_token.ok_or_else(|| "The catalog selection could not be processed.".to_string())?;
    if feed_ids.is_empty() {
        return Err("Choose at least one source to subscribe.".to_string());
    }
    if feed_ids.len() > MAX_CATALOG_SELECTION {
        return Err(format!(
            "Choose at most {MAX_CATALOG_SELECTION} sources at once."
        ));
    }
    Ok((csrf_token, feed_ids.into_iter().collect()))
}

pub async fn page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Query(query): Query<CatalogQuery>,
) -> Result<Response, AppError> {
    render_page(&state, &user, jar, query, None, None).await
}

pub async fn subscribe(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    Form(values): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let (csrf_token, feed_ids) = match parse_subscription_form(values) {
        Ok(form) => form,
        Err(error) => {
            return render_page(
                &state,
                &user,
                jar,
                CatalogQuery {
                    category: None,
                    country: None,
                    language: None,
                },
                None,
                Some(error),
            )
            .await;
        }
    };
    if !csrf::verify(&jar, &csrf_token) {
        return Err(AppError::Forbidden);
    }
    let result = match catalog::subscribe_published_many(state.db.pool(), user.id, &feed_ids).await
    {
        Ok(result) => result,
        Err(rustss_db::DbError::NotFound) => {
            return render_page(
                &state,
                &user,
                jar,
                CatalogQuery {
                    category: None,
                    country: None,
                    language: None,
                },
                None,
                Some("One or more selected sources are no longer available.".to_string()),
            )
            .await;
        }
        Err(error) => return Err(error.into()),
    };
    let existing = result.attempted - result.created;
    let mut queued = 0;
    for feed_id in &feed_ids {
        if enqueue_fetch_feed(&state.db, *feed_id).await?.is_some() {
            queued += 1;
        }
    }
    let message = if existing == 0 {
        format!(
            "Subscribed to {} source(s); queued {queued} shared refresh(es).",
            result.created
        )
    } else {
        format!(
            "Subscribed to {} new source(s); {} were already in your library. Queued {queued} shared refresh(es).",
            result.created, existing
        )
    };
    render_page(
        &state,
        &user,
        jar,
        CatalogQuery {
            category: None,
            country: None,
            language: None,
        },
        Some(message),
        None,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_deduplicated_catalog_choices() {
        let id = Uuid::now_v7();
        let (csrf, ids) = parse_subscription_form(vec![
            ("csrf_token".to_string(), "token".to_string()),
            ("feed_id".to_string(), id.to_string()),
            ("feed_id".to_string(), id.to_string()),
        ])
        .unwrap();
        assert_eq!(csrf, "token");
        assert_eq!(ids, vec![id]);
    }
}
