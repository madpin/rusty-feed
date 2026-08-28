//! Saved-search scoring (M11): keyword score blended with semantic similarity.
//! Boolean matching lives in [`crate::dsl`]; the semantic term is supplied by
//! the caller (pgvector cosine).

use crate::dsl::Query;

/// Weight of the semantic vs. keyword components (combined = 0.6·sem + 0.4·kw).
pub const SEMANTIC_WEIGHT: f64 = 0.6;
pub const KEYWORD_WEIGHT: f64 = 0.4;

/// Keyword score in [0,1] plus the terms that matched.
#[derive(Debug, Clone)]
pub struct KeywordResult {
    pub score: f64,
    pub matched: Vec<String>,
}

/// Keyword relevance: per positive term, `1 + ln(occurrences)` (×2 if the term
/// appears in the title), each capped and averaged into [0,1].
pub fn keyword_score(query: &Query, title: &str, content: &str) -> KeywordResult {
    let terms = query.positive_terms();
    if terms.is_empty() {
        return KeywordResult {
            score: 0.0,
            matched: Vec::new(),
        };
    }
    let title_l = title.to_ascii_lowercase();
    let content_l = content.to_ascii_lowercase();

    let mut sum = 0.0;
    let mut matched = Vec::new();
    for term in &terms {
        let body_occ = count_occurrences(&content_l, term);
        let title_occ = count_occurrences(&title_l, term);
        if body_occ == 0 && title_occ == 0 {
            continue;
        }
        // Title occurrences count double.
        let effective = body_occ + title_occ * 2;
        let weighted = 1.0 + (effective as f64).ln();
        sum += (weighted / 3.0).min(1.0);
        matched.push(term.clone());
    }
    KeywordResult {
        score: (sum / terms.len() as f64).clamp(0.0, 1.0),
        matched,
    }
}

/// Combine semantic + keyword scores into the final relevance.
pub fn combine(semantic: f64, keyword: f64) -> f64 {
    (SEMANTIC_WEIGHT * semantic.clamp(0.0, 1.0) + KEYWORD_WEIGHT * keyword.clamp(0.0, 1.0))
        .clamp(0.0, 1.0)
}

fn count_occurrences(haystack_lower: &str, term_lower: &str) -> usize {
    if term_lower.is_empty() {
        return 0;
    }
    if term_lower.contains(' ') {
        // Phrase: substring occurrences.
        haystack_lower.matches(term_lower).count()
    } else {
        // Word: exact token matches (avoids "ai" ⊂ "brain").
        haystack_lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| *t == term_lower)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse;

    #[test]
    fn title_presence_boosts_score() {
        let q = parse("rust");
        let in_body = keyword_score(&q, "Some title", "rust appears once in body");
        let in_title = keyword_score(&q, "Rust guide", "unrelated body text here");
        assert!(in_title.score > in_body.score);
        assert!(in_title.matched.contains(&"rust".to_string()));
    }

    #[test]
    fn word_match_is_not_substring() {
        let q = parse("ai");
        let r = keyword_score(&q, "t", "the brain chain contains no standalone token");
        assert_eq!(r.score, 0.0, "'ai' must not match inside 'brain'/'chain'");
    }

    #[test]
    fn combine_weights() {
        assert!((combine(1.0, 0.0) - 0.6).abs() < 1e-9);
        assert!((combine(0.0, 1.0) - 0.4).abs() < 1e-9);
        assert!((combine(1.0, 1.0) - 1.0).abs() < 1e-9);
    }
}
