//! Deterministic removal of known analytics parameters from public web URLs.

use std::collections::HashSet;

use url::Url;

const DEFAULT_EXACT_PARAMETERS: &[&str] = &[
    "fbclid",
    "gclid",
    "dclid",
    "msclkid",
    "mc_cid",
    "mc_eid",
    "igshid",
    "twclid",
    "ttclid",
    "li_fat_id",
    "vero_conv",
    "vero_id",
    "oly_anon_id",
    "oly_enc_id",
    "_hsenc",
    "_hsmi",
    "rb_clickid",
    "s_cid",
];

const DEFAULT_PREFIXES: &[&str] = &["utm_", "pk_", "piwik_", "mtm_", "matomo_", "hsa_"];

/// Instance-wide, immutable URL-cleaning policy.
///
/// Extra parameter entries are comma-separated exact names or trailing-wildcard
/// prefixes (`campaign_id,ref_*`). Domain exceptions match the named host and
/// its subdomains. The policy never mutates non-HTTP(S) URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackingUrlPolicy {
    enabled: bool,
    exact_parameters: HashSet<String>,
    parameter_prefixes: Vec<String>,
    domain_exceptions: Vec<String>,
}

impl Default for TrackingUrlPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            exact_parameters: DEFAULT_EXACT_PARAMETERS
                .iter()
                .map(|value| value.to_string())
                .collect(),
            parameter_prefixes: DEFAULT_PREFIXES
                .iter()
                .map(|value| value.to_string())
                .collect(),
            domain_exceptions: Vec::new(),
        }
    }
}

impl TrackingUrlPolicy {
    /// Whether cleanup is active for this instance.
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Number of exact and prefix parameter rules in the effective policy.
    pub fn parameter_rule_count(&self) -> usize {
        self.exact_parameters.len() + self.parameter_prefixes.len()
    }

    /// Number of host/subdomain exceptions in the effective policy.
    pub fn domain_exception_count(&self) -> usize {
        self.domain_exceptions.len()
    }

    /// Build and validate a policy from environment-friendly comma-separated values.
    pub fn try_new(
        enabled: bool,
        extra_parameters: &str,
        domain_exceptions: &str,
    ) -> Result<Self, String> {
        let defaults = Self::default();
        let mut exact_parameters = defaults.exact_parameters;
        let mut parameter_prefixes = defaults.parameter_prefixes;

        let extras = comma_values(extra_parameters, 100, "tracking parameters")?;
        for value in extras {
            let (name, prefix) = value
                .strip_suffix('*')
                .map_or((value.as_str(), false), |name| (name, true));
            if name.is_empty()
                || name.len() > 64
                || !name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || "_-.".contains(character))
            {
                return Err(format!("invalid tracking parameter pattern: {value}"));
            }
            let name = name.to_ascii_lowercase();
            if prefix {
                parameter_prefixes.push(name);
            } else {
                exact_parameters.insert(name);
            }
        }
        parameter_prefixes.sort();
        parameter_prefixes.dedup();

        let mut exceptions = comma_values(domain_exceptions, 100, "domain exceptions")?;
        for domain in &mut exceptions {
            *domain = domain.trim_end_matches('.').to_ascii_lowercase();
            if domain.is_empty()
                || domain.len() > 253
                || domain.starts_with('.')
                || domain.ends_with('.')
                || !domain.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '.')
                })
            {
                return Err(format!("invalid tracking URL domain exception: {domain}"));
            }
        }
        exceptions.sort();
        exceptions.dedup();

        Ok(Self {
            enabled,
            exact_parameters,
            parameter_prefixes,
            domain_exceptions: exceptions,
        })
    }

    /// Return the configured canonical form, or the input unchanged when the
    /// URL/scheme is outside this policy.
    pub fn clean(&self, value: &str) -> String {
        if !self.enabled {
            return value.to_string();
        }
        let Ok(mut url) = Url::parse(value) else {
            return value.to_string();
        };
        if !matches!(url.scheme(), "http" | "https") {
            return value.to_string();
        }
        let Some(host) = url
            .host_str()
            .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
        else {
            return value.to_string();
        };
        if self.domain_exceptions.iter().any(|domain| {
            host == *domain
                || host
                    .strip_suffix(domain)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        }) {
            return value.to_string();
        }

        let retained = url
            .query_pairs()
            .filter(|(name, _)| !self.removes(name))
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        if retained.len() == url.query_pairs().count() {
            return value.to_string();
        }
        url.query_pairs_mut().clear().extend_pairs(retained);
        if url.query() == Some("") {
            url.set_query(None);
        }
        url.to_string()
    }

    fn removes(&self, name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        self.exact_parameters.contains(&name)
            || self
                .parameter_prefixes
                .iter()
                .any(|prefix| name.starts_with(prefix))
    }
}

fn comma_values(value: &str, maximum: usize, label: &str) -> Result<Vec<String>, String> {
    let values = value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if values.len() > maximum {
        return Err(format!("{label} supports at most {maximum} entries"));
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_defaults_case_insensitively_and_preserves_functional_values() {
        let policy = TrackingUrlPolicy::default();
        assert_eq!(
            policy.clean("https://example.com/read?id=42&UTM_Source=news&fbclid=x#part"),
            "https://example.com/read?id=42#part"
        );
    }

    #[test]
    fn supports_extra_exact_prefix_and_domain_exception_rules() {
        let policy = TrackingUrlPolicy::try_new(true, "campaign,ref_*", "keep.example.com")
            .expect("valid policy");
        assert_eq!(
            policy.clean("https://news.example.com/a?campaign=x&ref_src=y&id=1"),
            "https://news.example.com/a?id=1"
        );
        assert_eq!(
            policy.clean("https://sub.keep.example.com/a?utm_source=x"),
            "https://sub.keep.example.com/a?utm_source=x"
        );
    }

    #[test]
    fn disabled_policy_and_non_http_urls_are_unchanged() {
        let disabled = TrackingUrlPolicy::try_new(false, "", "").expect("valid policy");
        assert_eq!(
            disabled.clean("https://example.com/?utm_source=x"),
            "https://example.com/?utm_source=x"
        );
        assert_eq!(
            TrackingUrlPolicy::default().clean("mailto:reader@example.com?utm_source=x"),
            "mailto:reader@example.com?utm_source=x"
        );
    }

    #[test]
    fn rejects_unbounded_or_ambiguous_patterns() {
        assert!(TrackingUrlPolicy::try_new(true, "*", "").is_err());
        assert!(TrackingUrlPolicy::try_new(true, "a?", "").is_err());
        assert!(TrackingUrlPolicy::try_new(true, "", "https://example.com").is_err());
    }
}
