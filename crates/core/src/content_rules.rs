//! Bounded, declarative content manipulation. Rules are data, never server-side code.

use lol_html::{element, rewrite_str, text, RewriteStrSettings};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_REWRITE_RULES: usize = 20;
pub const MAX_ENTRY_FILTERS: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RewriteRule {
    pub name: String,
    pub selector: String,
    pub operation: String,
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub replacement: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntryFilter {
    pub name: String,
    pub target: String,
    pub mode: String,
    pub pattern: String,
    #[serde(default)]
    pub case_insensitive: bool,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

fn enabled() -> bool {
    true
}

fn bounded_regex(pattern: &str, case_insensitive: bool) -> Result<Regex, String> {
    if pattern.is_empty() || pattern.chars().count() > 512 {
        return Err("Regex patterns must contain 1 to 512 characters.".into());
    }
    RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .size_limit(1_048_576)
        .dfa_size_limit(2_097_152)
        .nest_limit(64)
        .build()
        .map_err(|error| format!("Invalid bounded regular expression: {error}"))
}

pub fn rewrite_rules_from_json(value: Option<&Value>) -> Vec<RewriteRule> {
    value
        .and_then(|value| value.get("rewrite_rules"))
        .cloned()
        .and_then(|value| serde_json::from_value::<Vec<RewriteRule>>(value).ok())
        .unwrap_or_default()
        .into_iter()
        .take(MAX_REWRITE_RULES)
        .collect()
}

pub fn entry_filters_from_json(value: Option<&Value>) -> Vec<EntryFilter> {
    value
        .and_then(|value| value.get("entry_filters"))
        .cloned()
        .and_then(|value| serde_json::from_value::<Vec<EntryFilter>>(value).ok())
        .unwrap_or_default()
        .into_iter()
        .take(MAX_ENTRY_FILTERS)
        .collect()
}

pub fn validate_rewrite_rules(rules: &[RewriteRule]) -> Result<(), String> {
    if rules.len() > MAX_REWRITE_RULES {
        return Err(format!("Use at most {MAX_REWRITE_RULES} rewrite rules."));
    }
    for rule in rules {
        if rule.name.trim().is_empty() || rule.name.chars().count() > 80 {
            return Err("Every rewrite rule needs a 1–80 character name.".into());
        }
        if rule.selector.trim().is_empty() || rule.selector.chars().count() > 512 {
            return Err("Rewrite selectors must contain 1 to 512 characters.".into());
        }
        if !matches!(rule.operation.as_str(), "remove" | "replace_text") {
            return Err("Rewrite operations must be remove or replace_text.".into());
        }
        if rule.operation == "replace_text" {
            bounded_regex(&rule.pattern, false)?;
            if rule.replacement.chars().count() > 2_000 {
                return Err("Rewrite replacements may contain at most 2,000 characters.".into());
            }
        }
    }
    Ok(())
}

pub fn validate_entry_filters(filters: &[EntryFilter]) -> Result<(), String> {
    if filters.len() > MAX_ENTRY_FILTERS {
        return Err(format!("Use at most {MAX_ENTRY_FILTERS} entry filters."));
    }
    for filter in filters {
        if filter.name.trim().is_empty() || filter.name.chars().count() > 80 {
            return Err("Every entry filter needs a 1–80 character name.".into());
        }
        if !matches!(
            filter.target.as_str(),
            "title" | "content" | "author" | "url"
        ) {
            return Err("Entry-filter targets must be title, content, author, or URL.".into());
        }
        if !matches!(filter.mode.as_str(), "include" | "exclude") {
            return Err("Entry-filter modes must be include or exclude.".into());
        }
        bounded_regex(&filter.pattern, filter.case_insensitive)?;
    }
    Ok(())
}

/// Apply ordered rewrite rules. The caller must sanitize the returned HTML.
pub fn apply_rewrite_rules(html: &str, rules: &[RewriteRule]) -> Result<String, String> {
    validate_rewrite_rules(rules)?;
    let mut output = html.to_string();
    for rule in rules.iter().filter(|rule| rule.enabled) {
        output = if rule.operation == "remove" {
            let handler = element!(rule.selector.as_str(), |element| {
                element.remove();
                Ok(())
            });
            rewrite_str(
                &output,
                RewriteStrSettings::new().append_element_content_handler(handler),
            )
            .map_err(|error| error.to_string())?
        } else {
            let pattern = bounded_regex(&rule.pattern, false)?;
            let replacement = rule.replacement.clone();
            let handler = text!(rule.selector.as_str(), move |chunk| {
                let replaced = pattern
                    .replace_all(chunk.as_str(), replacement.as_str())
                    .into_owned();
                chunk.replace(&replaced, lol_html::html_content::ContentType::Text);
                Ok(())
            });
            rewrite_str(
                &output,
                RewriteStrSettings::new().append_element_content_handler(handler),
            )
            .map_err(|error| error.to_string())?
        };
        if output.len() > 5 * 1_024 * 1_024 {
            return Err("Rewritten content exceeded the 5 MiB safety limit.".into());
        }
    }
    Ok(output)
}

pub fn entry_is_visible(
    title: &str,
    content: &str,
    author: Option<&str>,
    url: &str,
    filters: &[EntryFilter],
) -> Result<bool, String> {
    validate_entry_filters(filters)?;
    let mut has_include = false;
    let mut include_match = false;
    for filter in filters.iter().filter(|filter| filter.enabled) {
        let candidate = match filter.target.as_str() {
            "title" => title,
            "content" => content,
            "author" => author.unwrap_or_default(),
            "url" => url,
            _ => continue,
        };
        let matched = bounded_regex(&filter.pattern, filter.case_insensitive)?.is_match(candidate);
        if filter.mode == "exclude" && matched {
            return Ok(false);
        }
        if filter.mode == "include" {
            has_include = true;
            include_match |= matched;
        }
    }
    Ok(!has_include || include_match)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_rewrites_remove_and_replace_then_require_resanitization() {
        let rules = vec![
            RewriteRule {
                name: "remove promo".into(),
                selector: ".promo".into(),
                operation: "remove".into(),
                pattern: String::new(),
                replacement: String::new(),
                enabled: true,
            },
            RewriteRule {
                name: "expand".into(),
                selector: "p".into(),
                operation: "replace_text".into(),
                pattern: "JS".into(),
                replacement: "JavaScript".into(),
                enabled: true,
            },
        ];
        let output =
            apply_rewrite_rules("<p>JS guide</p><aside class=promo>ad</aside>", &rules).unwrap();
        assert!(output.contains("JavaScript guide"));
        assert!(!output.contains("promo"));
    }

    #[test]
    fn include_and_exclude_filters_are_composed() {
        let filters = vec![
            EntryFilter {
                name: "Rust".into(),
                target: "title".into(),
                mode: "include".into(),
                pattern: "rust".into(),
                case_insensitive: true,
                enabled: true,
            },
            EntryFilter {
                name: "Ads".into(),
                target: "content".into(),
                mode: "exclude".into(),
                pattern: "sponsor".into(),
                case_insensitive: true,
                enabled: true,
            },
        ];
        assert!(entry_is_visible(
            "Rust release",
            "Details",
            None,
            "https://example.test",
            &filters
        )
        .unwrap());
        assert!(!entry_is_visible(
            "Rust release",
            "Sponsored",
            None,
            "https://example.test",
            &filters
        )
        .unwrap());
        assert!(!entry_is_visible(
            "Go release",
            "Details",
            None,
            "https://example.test",
            &filters
        )
        .unwrap());
    }
}
