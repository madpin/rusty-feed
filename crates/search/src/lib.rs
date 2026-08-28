//! # rustss-search
//!
//! Personalization (M10): TF keyword extraction, feedback→pattern math, decay,
//! and sigmoid relevance scoring — implemented **exactly** per
//! `docs/ARCHITECTURE.md` §11.2 so results are reproducible. The pure functions
//! here are covered by exact-value unit tests; [`personalize`] wires them to the
//! database + cache.

pub mod dsl;
pub mod personalize;
pub mod saved;
mod stopwords;

use std::collections::HashMap;

/// A weighted keyword extracted from article text.
#[derive(Debug, Clone, PartialEq)]
pub struct Keyword {
    pub term: String,
    pub relevance: f64,
}

/// Extract the top-`n` keywords: strip HTML → lowercase → tokenize on non-word
/// → drop len ≤ 2 and stop-words → TF normalized by kept-token count, with a
/// heuristic weight (×1.5 for raw count 2..=10, ×0.5 for >10).
pub fn extract_keywords(text: &str, n: usize) -> Vec<Keyword> {
    let text = strip_html(text).to_lowercase();
    let tokens: Vec<&str> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() > 2 && !stopwords::is_stop_word(t))
        .collect();
    let total = tokens.len() as f64;
    if total == 0.0 {
        return Vec::new();
    }

    let mut counts: HashMap<&str, u32> = HashMap::new();
    for t in &tokens {
        *counts.entry(t).or_insert(0) += 1;
    }

    let mut keywords: Vec<Keyword> = counts
        .into_iter()
        .map(|(term, count)| {
            let tf = count as f64 / total;
            let heuristic = if (2..=10).contains(&count) {
                1.5
            } else if count > 10 {
                0.5
            } else {
                1.0
            };
            Keyword {
                term: term.to_string(),
                relevance: tf * heuristic,
            }
        })
        .collect();

    // Deterministic: by relevance desc, then term asc for ties.
    keywords.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.term.cmp(&b.term))
    });
    keywords.truncate(n);
    keywords
}

/// Explicit thumbs feedback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplicitFeedback {
    Up,
    Down,
}

/// Explicit feedback value: 👍 `+1.0`, 👎 `−1.0`.
pub fn explicit_value(feedback: ExplicitFeedback) -> f64 {
    match feedback {
        ExplicitFeedback::Up => 1.0,
        ExplicitFeedback::Down => -1.0,
    }
}

/// Implicit feedback from reading time: completion (≥ 0.9 of estimated) `+0.5`,
/// bounce (< `bounce_threshold`) `−0.5`, otherwise no signal (`None`).
pub fn implicit_value(time_spent: f64, estimated: f64, bounce_threshold: f64) -> Option<f64> {
    if estimated <= 0.0 {
        return None;
    }
    let ratio = time_spent / estimated;
    if ratio >= 0.9 {
        Some(0.5)
    } else if ratio < bounce_threshold {
        Some(-0.5)
    } else {
        None
    }
}

/// Pattern weight delta: `feedback_value × relevance × 0.1`.
pub fn pattern_delta(feedback_value: f64, relevance: f64) -> f64 {
    feedback_value * relevance * 0.1
}

/// Decay multiplier for a pattern last updated `days_since_update` days ago:
/// `0.9^floor(days/30)` (i.e. 1.0 for < 30 days).
pub fn decay_factor(days_since_update: i64) -> f64 {
    let periods = (days_since_update.max(0) / 30) as i32;
    0.9f64.powi(periods)
}

/// Sigmoid relevance score from the summed weighted match: `1/(1+e^(−5·total))`
/// (0.5 when `total == 0`, i.e. no matching patterns).
pub fn relevance_score(total: f64) -> f64 {
    1.0 / (1.0 + (-5.0 * total).exp())
}

/// Remove HTML tags and collapse whitespace.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_drops_short_and_stop_words_and_weights() {
        // "rust" x3 (heuristic 1.5), "async" x2 (1.5); "the"/"of"/"is" are stop
        // words; "ai" is len 2 (dropped).
        let text = "<p>Rust rust RUST async Async the of is ai</p>";
        let kws = extract_keywords(text, 10);
        // Kept tokens: rust,rust,rust,async,async  → total = 5
        let rust = kws.iter().find(|k| k.term == "rust").unwrap();
        let async_kw = kws.iter().find(|k| k.term == "async").unwrap();
        assert!((rust.relevance - (3.0 / 5.0 * 1.5)).abs() < 1e-9); // 0.9
        assert!((async_kw.relevance - (2.0 / 5.0 * 1.5)).abs() < 1e-9); // 0.6
        assert_eq!(kws[0].term, "rust"); // highest relevance first
        assert!(!kws.iter().any(|k| k.term == "the" || k.term == "ai"));
    }

    #[test]
    fn single_occurrence_has_no_heuristic_multiplier() {
        let kws = extract_keywords("alpha beta gamma", 10);
        // 3 unique tokens, each count 1 → tf = 1/3, heuristic 1.0.
        for k in &kws {
            assert!((k.relevance - (1.0 / 3.0)).abs() < 1e-9);
        }
    }

    #[test]
    fn feedback_values_match_spec() {
        assert_eq!(explicit_value(ExplicitFeedback::Up), 1.0);
        assert_eq!(explicit_value(ExplicitFeedback::Down), -1.0);
        assert_eq!(implicit_value(100.0, 100.0, 0.25), Some(0.5)); // ratio 1.0
        assert_eq!(implicit_value(95.0, 100.0, 0.25), Some(0.5)); // ratio 0.95
        assert_eq!(implicit_value(10.0, 100.0, 0.25), Some(-0.5)); // ratio 0.1 < 0.25
        assert_eq!(implicit_value(50.0, 100.0, 0.25), None); // in between
        assert_eq!(implicit_value(5.0, 0.0, 0.25), None); // no estimate
    }

    #[test]
    fn delta_decay_and_sigmoid_exact() {
        assert!((pattern_delta(1.0, 0.4) - 0.04).abs() < 1e-12);
        assert!((pattern_delta(-1.0, 0.9) + 0.09).abs() < 1e-12);
        assert!((decay_factor(0) - 1.0).abs() < 1e-12);
        assert!((decay_factor(29) - 1.0).abs() < 1e-12);
        assert!((decay_factor(30) - 0.9).abs() < 1e-12);
        assert!((decay_factor(65) - 0.81).abs() < 1e-12);
        assert!((relevance_score(0.0) - 0.5).abs() < 1e-12);
        // sigmoid(5*0.2)=sigmoid(1)=0.7310585786
        assert!((relevance_score(0.2) - 0.7310585786300049).abs() < 1e-9);
    }
}
