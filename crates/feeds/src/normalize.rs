//! Feed URL normalization, so the same feed maps to one shared row regardless of
//! superficial URL differences.

use crate::error::FeedError;

/// Normalize a feed URL: require http(s), lowercase scheme/host (the `url` crate
/// does this), drop the fragment, and remove the default port.
pub fn normalize_url(input: &str) -> Result<String, FeedError> {
    let trimmed = input.trim();
    let mut url = url::Url::parse(trimmed).map_err(|_| FeedError::InvalidUrl(input.to_string()))?;

    match url.scheme() {
        "http" | "https" => {}
        other => return Err(FeedError::UnsupportedScheme(other.to_string())),
    }
    if url.host_str().is_none() {
        return Err(FeedError::InvalidUrl(input.to_string()));
    }

    url.set_fragment(None);
    let is_default_port = matches!(
        (url.scheme(), url.port()),
        ("http", Some(80)) | ("https", Some(443))
    );
    if is_default_port {
        let _ = url.set_port(None);
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_port_and_fragment() {
        assert_eq!(
            normalize_url("HTTPS://Example.COM:443/Feed.xml#top").unwrap(),
            "https://example.com/Feed.xml"
        );
        assert_eq!(
            normalize_url("  http://example.com:80/rss  ").unwrap(),
            "http://example.com/rss"
        );
    }

    #[test]
    fn rejects_bad_schemes_and_urls() {
        assert!(normalize_url("ftp://example.com").is_err());
        assert!(normalize_url("file:///etc/passwd").is_err());
        assert!(normalize_url("not a url").is_err());
    }
}
