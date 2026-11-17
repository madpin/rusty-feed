//! Personal reading presentation and shared story-briefing text.
//!
//! Autolinking and ADHD emphasis run at render time on already-sanitized HTML.
//! They never change the shared stored article. Story clustering uses
//! [`STORY_CLUSTER_MIN_SIMILARITY`] so the same event from different sources can
//! share one briefing.

use url::Url;

/// Cosine similarity required before two embedded articles join one story.
///
/// Saved-search matches start at 0.6 and high-confidence notifications at 0.85
/// (`docs/ARCHITECTURE.md` §11). Story grouping sits just under the notification
/// bar so only near-duplicate coverage collapses.
pub const STORY_CLUSTER_MIN_SIMILARITY: f64 = 0.82;

/// Whether `similarity` (0–1, from `1 - cosine_distance/2`) is the same story.
pub fn joins_story_cluster(similarity: f64) -> bool {
    similarity.is_finite() && similarity >= STORY_CLUSTER_MIN_SIMILARITY
}

/// One source line in a personal daily briefing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefingSource {
    pub title: String,
    pub href: String,
    pub feed_name: String,
}

/// A shared story reduced to the sources one reader can actually open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefingStory {
    pub headline: String,
    pub summary: String,
    pub sources: Vec<BriefingSource>,
}

/// Render a plain-text daily briefing. Returns `(title, body)`.
pub fn format_daily_digest(date_label: &str, stories: &[BriefingStory]) -> (String, String) {
    let title = if stories.len() == 1 {
        format!("Briefing for {date_label}: 1 story")
    } else {
        format!("Briefing for {date_label}: {} stories", stories.len())
    };
    let mut body = String::new();
    for (index, story) in stories.iter().enumerate() {
        if index > 0 {
            body.push_str("\n\n");
        }
        body.push_str(&story.headline);
        body.push('\n');
        if !story.summary.trim().is_empty() {
            body.push_str(story.summary.trim());
            body.push('\n');
        }
        for source in &story.sources {
            body.push_str("- ");
            body.push_str(source.title.trim());
            if !source.feed_name.trim().is_empty() {
                body.push_str(" (");
                body.push_str(source.feed_name.trim());
                body.push(')');
            }
            if !source.href.is_empty() {
                body.push(' ');
                body.push_str(&source.href);
            }
            body.push('\n');
        }
    }
    (title, body.trim_end().to_string())
}

/// Prefix same-app article paths so a briefing email can be opened off-site.
///
/// Stored briefings keep root-relative paths. Only a `http` or `https` base is
/// applied, and text that is not a Rusty Feed article path is left unchanged.
pub fn absolutize_app_paths(body: &str, base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    let absolute = base.starts_with("http://") || base.starts_with("https://");
    if base.is_empty() || !absolute {
        return body.to_string();
    }
    body.replace(" /articles/", &format!(" {base}/articles/"))
        .replace("](/articles/", &format!("]({base}/articles/"))
}

/// A deterministic briefing used when no LLM is configured. Shared, so every
/// subscriber of the story sees the same sentence.
pub fn fallback_story_summary(titles: &[String]) -> String {
    let shown: Vec<&str> = titles
        .iter()
        .map(|title| title.trim())
        .filter(|title| !title.is_empty())
        .take(6)
        .collect();
    if shown.is_empty() {
        return "Several sources covered this story.".to_string();
    }
    format!("Covered by {} sources: {}.", shown.len(), shown.join("; "))
}

/// Turn bare `http`/`https` URLs in text nodes into safe anchors.
///
/// Existing links, code, and preformatted text are left alone. Credentials in
/// the URL are rejected. Trailing punctuation stays outside the link.
pub fn autolink_bare_urls(html: &str) -> String {
    rewrite_text_nodes(html, true, autolink_text)
}

/// Bold the leading half of longer words so a reader can track lines.
///
/// Applied only to text outside links, code, and preformatted blocks.
pub fn apply_adhd_reading(html: &str) -> String {
    rewrite_text_nodes(html, true, emphasize_words)
}

/// Autolink, then optionally apply ADHD emphasis. Callers that add `target`
/// should run link navigation between the two steps.
pub fn present_article_html(html: &str, adhd: bool) -> String {
    let linked = autolink_bare_urls(html);
    if adhd {
        apply_adhd_reading(&linked)
    } else {
        linked
    }
}

fn autolink_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let Some(start) = find_url_start(rest) else {
            out.push_str(rest);
            break;
        };
        if start > 0 {
            let previous = rest[..start].chars().next_back();
            if previous.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '/' || ch == '@') {
                out.push_str(&rest[..start]);
                let Some(ch) = rest[start..].chars().next() else {
                    break;
                };
                out.push(ch);
                rest = &rest[start + ch.len_utf8()..];
                continue;
            }
            out.push_str(&rest[..start]);
        }
        let candidate = &rest[start..];
        let (raw_len, url_len) = bare_url_span(candidate);
        let raw = &candidate[..url_len];
        if let Some(url) = accept_http_url(raw) {
            let href = html_escape::encode_double_quoted_attribute(&url);
            let label = html_escape::encode_text(&url);
            out.push_str("<a href=\"");
            out.push_str(&href);
            out.push_str("\" rel=\"noopener noreferrer\" referrerpolicy=\"no-referrer\">");
            out.push_str(&label);
            out.push_str("</a>");
            out.push_str(&candidate[url_len..raw_len]);
            rest = &candidate[raw_len..];
        } else {
            let skip = raw_len.max(1);
            out.push_str(&candidate[..skip]);
            rest = &candidate[skip..];
        }
    }
    out
}

fn find_url_start(text: &str) -> Option<usize> {
    let https = text.find("https://");
    let http = text.find("http://");
    match (https, http) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// `(span including trimmed punctuation, url length)`.
fn bare_url_span(text: &str) -> (usize, usize) {
    let mut end = 0;
    for ch in text.chars() {
        if ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\'' | '`' | '|' | '\\') {
            break;
        }
        end += ch.len_utf8();
        if end >= 2048 {
            break;
        }
    }
    let mut url_end = end;
    let bytes = text.as_bytes();
    while url_end > 0
        && matches!(
            bytes[url_end - 1],
            b'.' | b',' | b';' | b':' | b'!' | b'?' | b')' | b']'
        )
    {
        url_end -= 1;
    }
    (end, url_end)
}

fn accept_http_url(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }
    if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    Some(url.to_string())
}

fn emphasize_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        let letters = word.chars().count();
        let lead = if letters >= 4 {
            letters.div_ceil(2).clamp(2, 5)
        } else {
            0
        };
        if lead == 0 || lead >= letters {
            out.push_str(word);
        } else {
            let split_at = word
                .char_indices()
                .nth(lead)
                .map(|(index, _)| index)
                .unwrap_or(word.len());
            out.push_str("<b class=\"adhd-lead\">");
            out.push_str(&word[..split_at]);
            out.push_str("</b>");
            out.push_str(&word[split_at..]);
        }
        word.clear();
    };
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            word.push(ch);
        } else {
            flush(&mut word, &mut out);
            out.push(ch);
        }
    }
    flush(&mut word, &mut out);
    out
}

fn rewrite_text_nodes(html: &str, skip_anchors: bool, transform: fn(&str) -> String) -> String {
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let mut index = 0;
    let mut skip_depth: u32 = 0;
    while index < bytes.len() {
        if bytes[index] == b'<' {
            let start = index;
            if html[index..].starts_with("<!--") {
                if let Some(relative) = html[index..].find("-->") {
                    index += relative + 3;
                } else {
                    index = html.len();
                }
                out.push_str(&html[start..index]);
                continue;
            }
            index += 1;
            let closing = index < bytes.len() && bytes[index] == b'/';
            if closing {
                index += 1;
            }
            let name_start = index;
            while index < bytes.len() && bytes[index].is_ascii_alphabetic() {
                index += 1;
            }
            let name = html[name_start..index].to_ascii_lowercase();
            let mut self_closing = false;
            let mut quote: Option<u8> = None;
            while index < bytes.len() {
                let byte = bytes[index];
                index += 1;
                if let Some(active) = quote {
                    if byte == active {
                        quote = None;
                    }
                    continue;
                }
                if byte == b'"' || byte == b'\'' {
                    quote = Some(byte);
                    continue;
                }
                if byte == b'/' {
                    self_closing = true;
                }
                if byte == b'>' {
                    break;
                }
            }
            out.push_str(&html[start..index]);
            if is_skipped_tag(&name, skip_anchors) && !self_closing {
                if closing {
                    skip_depth = skip_depth.saturating_sub(1);
                } else {
                    skip_depth = skip_depth.saturating_add(1);
                }
            }
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index] != b'<' {
            index += 1;
        }
        let text = &html[start..index];
        if skip_depth == 0 {
            out.push_str(&transform(text));
        } else {
            out.push_str(text);
        }
    }
    out
}

fn is_skipped_tag(name: &str, skip_anchors: bool) -> bool {
    matches!(name, "code" | "pre" | "script" | "style" | "textarea")
        || (skip_anchors && name == "a")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_threshold_matches_the_documented_constant() {
        assert!(joins_story_cluster(0.82));
        assert!(joins_story_cluster(0.99));
        assert!(!joins_story_cluster(0.819));
        assert!(!joins_story_cluster(f64::NAN));
    }

    #[test]
    fn autolink_wraps_bare_urls_and_keeps_trailing_punctuation() {
        let html = autolink_bare_urls("<p>See https://example.com/a. now</p>");
        assert!(html.contains("href=\"https://example.com/a\""));
        assert!(html.contains(">https://example.com/a</a>."));
        assert!(html.contains("rel=\"noopener noreferrer\""));
    }

    #[test]
    fn autolink_ignores_existing_links_code_and_credentials() {
        let linked = "<a href=\"https://example.com\">https://example.com</a>";
        assert_eq!(autolink_bare_urls(linked), linked);
        let code = "<code>https://example.com/secret</code>";
        assert_eq!(autolink_bare_urls(code), code);
        let credential = "<p>http://user:pass@example.com/x</p>";
        assert_eq!(autolink_bare_urls(credential), credential);
    }

    #[test]
    fn adhd_emphasis_bolds_the_leading_half_outside_links() {
        let html = apply_adhd_reading("<p>reading the <a href=\"/x\">reading</a> notes</p>");
        assert!(html.contains("<b class=\"adhd-lead\">read</b>ing"));
        assert!(html.contains("<a href=\"/x\">reading</a>"));
        assert!(!html.contains("<b class=\"adhd-lead\">th</b>"));
    }

    #[test]
    fn digest_text_lists_only_the_supplied_sources() {
        let (title, body) = format_daily_digest(
            "2026-11-03",
            &[BriefingStory {
                headline: "Harbor closed".into(),
                summary: "Two ports reported the same closure.".into(),
                sources: vec![BriefingSource {
                    title: "Port notice".into(),
                    href: "/articles/1".into(),
                    feed_name: "Gazette".into(),
                }],
            }],
        );
        assert_eq!(title, "Briefing for 2026-11-03: 1 story");
        assert!(body.contains("Harbor closed"));
        assert!(body.contains("Port notice (Gazette) /articles/1"));
    }

    #[test]
    fn briefing_email_uses_the_public_origin_for_article_paths() {
        let body = "Port notice (Gazette) /articles/1\nOpen https://publisher.example/articles/1";
        let mailed = absolutize_app_paths(body, "https://feeds.example/");
        assert!(mailed.contains("https://feeds.example/articles/1"));
        assert!(mailed.contains("https://publisher.example/articles/1"));
        let markdown = absolutize_app_paths("[1](/articles/1)", "https://feeds.example/");
        assert_eq!(markdown, "[1](https://feeds.example/articles/1)");
        assert_eq!(absolutize_app_paths(body, "feeds.example"), body);
    }
}
