//! Personalization orchestration: apply feedback to patterns, score articles
//! (cached with explanations), and reset learning. Wires the pure math in the
//! crate root to the database + cache.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rusty_feed_cache::Cache;
use rusty_feed_core::html::article_html_to_markdown;
use rusty_feed_db::{feedback, patterns, Article, Db, Result};

use crate::{
    explicit_value, extract_keywords, implicit_value, pattern_delta, relevance_score,
    ExplicitFeedback,
};

/// Keywords considered per article (both for updates and scoring).
const TOP_KEYWORDS: usize = 30;
/// How long a computed relevance score is cached.
const SCORE_TTL: Duration = Duration::from_secs(3600);

fn score_key(user_id: Uuid, article_id: Uuid) -> String {
    format!("score:{user_id}:{article_id}")
}

fn article_text(article: &Article) -> String {
    let markdown = article_html_to_markdown(&article.content, &article.url);
    format!(
        "{} {} {}",
        article.title,
        article.excerpt.as_deref().unwrap_or(""),
        markdown
    )
}

/// A cached relevance score with matched-keyword explanations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scored {
    pub score: f64,
    pub explanations: Vec<(String, f64)>,
}

struct FeedbackInput<'a> {
    feedback_type: &'a str,
    value: f64,
    previous_value: f64,
    times: Option<(i32, i32)>,
}

/// Record explicit 👍/👎 feedback and update patterns.
pub async fn record_explicit(
    db: &Db,
    cache: &Cache,
    user_id: Uuid,
    article: &Article,
    rating: ExplicitFeedback,
) -> Result<()> {
    let value = explicit_value(rating);
    let previous = feedback::get(db.pool(), user_id, article.id)
        .await?
        .map(|record| record.feedback_value)
        .unwrap_or_default();
    apply(
        db,
        cache,
        user_id,
        article,
        FeedbackInput {
            feedback_type: "explicit",
            value,
            previous_value: previous,
            times: None,
        },
    )
    .await?;
    Ok(())
}

/// Record implicit reading feedback (completion/bounce). Returns whether a
/// signal was applied (skipped when neutral, or when explicit feedback exists).
pub async fn record_implicit(
    db: &Db,
    cache: &Cache,
    user_id: Uuid,
    article: &Article,
    time_spent: f64,
    estimated: f64,
    bounce_threshold: f64,
) -> Result<bool> {
    // Explicit feedback is never overwritten by implicit signals.
    let previous = feedback::get(db.pool(), user_id, article.id).await?;
    if previous
        .as_ref()
        .map(|record| record.feedback_type.as_str())
        == Some("explicit")
    {
        return Ok(false);
    }
    let Some(value) = implicit_value(time_spent, estimated, bounce_threshold) else {
        return Ok(false);
    };
    apply(
        db,
        cache,
        user_id,
        article,
        FeedbackInput {
            feedback_type: "implicit",
            value,
            previous_value: previous
                .map(|record| record.feedback_value)
                .unwrap_or_default(),
            times: Some((time_spent as i32, estimated as i32)),
        },
    )
    .await
}

async fn apply(
    db: &Db,
    cache: &Cache,
    user_id: Uuid,
    article: &Article,
    input: FeedbackInput<'_>,
) -> Result<bool> {
    let delta_value = input.value - input.previous_value;
    if delta_value.abs() < f64::EPSILON {
        return Ok(false);
    }
    let (time_spent, estimated) = match input.times {
        Some((t, e)) => (Some(t), Some(e)),
        None => (None, None),
    };
    feedback::upsert(
        db.pool(),
        user_id,
        article.id,
        input.feedback_type,
        input.value,
        time_spent,
        estimated,
    )
    .await?;

    for kw in extract_keywords(&article_text(article), TOP_KEYWORDS) {
        patterns::upsert_delta(
            db.pool(),
            user_id,
            &kw.term,
            pattern_delta(delta_value, kw.relevance),
        )
        .await?;
    }

    // The learned model changed → drop the cached score.
    cache.delete(&score_key(user_id, article.id)).await;
    Ok(true)
}

/// Score an article for a user (cached 1h). Returns 0.5 (neutral) when no
/// patterns match.
pub async fn score_article(
    db: &Db,
    cache: &Cache,
    user_id: Uuid,
    article: &Article,
) -> Result<Scored> {
    let key = score_key(user_id, article.id);
    if let Some(cached) = cache.get::<Scored>(&key).await {
        return Ok(cached);
    }

    let keywords = extract_keywords(&article_text(article), TOP_KEYWORDS);
    let terms: Vec<String> = keywords.iter().map(|k| k.term.clone()).collect();
    let weights: HashMap<String, f64> = patterns::weights_for(db.pool(), user_id, &terms)
        .await?
        .into_iter()
        .collect();

    let mut total = 0.0;
    let mut explanations: Vec<(String, f64)> = Vec::new();
    for kw in &keywords {
        if let Some(weight) = weights.get(&kw.term) {
            let contribution = weight * kw.relevance;
            total += contribution;
            explanations.push((kw.term.clone(), contribution));
        }
    }

    let score = if weights.is_empty() {
        0.5
    } else {
        relevance_score(total)
    };
    explanations.sort_by(|a, b| {
        b.1.abs()
            .partial_cmp(&a.1.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    explanations.truncate(5);

    let scored = Scored {
        score,
        explanations,
    };
    cache.set(&key, &scored, SCORE_TTL).await;
    Ok(scored)
}

/// Clear a user's learned patterns and feedback, and drop cached scores.
pub async fn reset_learning(db: &Db, cache: &Cache, user_id: Uuid) -> Result<()> {
    patterns::delete_for_user(db.pool(), user_id).await?;
    feedback::delete_for_user(db.pool(), user_id).await?;
    cache.delete_prefix(&format!("score:{user_id}:")).await;
    Ok(())
}
