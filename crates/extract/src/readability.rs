//! HTML → article-content extractors.
//!
//! [`ReadabilityExtractor`] is a pragmatic, pure-Rust main-content heuristic
//! (via `scraper`): it honors a per-feed CSS selector, else picks the densest
//! paragraph container. [`RawExtractor`] is a low-priority fallback that keeps
//! the sanitized `<body>`. A headless-Chrome extractor (chromiumoxide) can slot
//! in as another [`Extractor`] later (FND-120). All output is sanitized.

use scraper::{Html, Selector};

use rustss_core::html::sanitize_article_html;

/// Extracted article content (sanitized HTML).
#[derive(Debug, Clone)]
pub struct Extracted {
    pub content: String,
    pub excerpt: Option<String>,
    pub title: Option<String>,
    pub extractor: &'static str,
}

/// A content extractor over already-fetched HTML (fetching is centralized in the
/// SSRF-safe client, so extractors stay pure/testable).
pub trait Extractor: Send + Sync {
    fn name(&self) -> &'static str;
    /// Return content if this extractor found something usable.
    fn extract(&self, html: &str, selector: Option<&str>, base_url: &str) -> Option<Extracted>;
}

const MIN_CONTENT_CHARS: usize = 200;
const CANDIDATE_SELECTORS: &[&str] = &[
    "article",
    "main",
    "[role=main]",
    ".post-content",
    ".entry-content",
    ".article-content",
    ".article-body",
    ".post-body",
    "#content",
    ".content",
];

/// Readability-style main-content extraction.
pub struct ReadabilityExtractor;

impl Extractor for ReadabilityExtractor {
    fn name(&self) -> &'static str {
        "readability"
    }

    fn extract(&self, html: &str, selector: Option<&str>, base_url: &str) -> Option<Extracted> {
        let doc = Html::parse_document(html);
        let title = document_title(&doc);

        let raw = if let Some(sel) = selector {
            Selector::parse(sel)
                .ok()
                .and_then(|s| doc.select(&s).next().map(|e| e.inner_html()))
        } else {
            pick_main_content(&doc)
        }?;

        let content = sanitize_article_html(&raw, base_url);
        if text_len(&content) < MIN_CONTENT_CHARS {
            return None;
        }
        let excerpt = excerpt_from(&content);
        Some(Extracted {
            content,
            excerpt,
            title,
            extractor: "readability",
        })
    }
}

/// Fallback: sanitize the whole `<body>` (used only if it has substantial text).
pub struct RawExtractor;

impl Extractor for RawExtractor {
    fn name(&self) -> &'static str {
        "raw"
    }

    fn extract(&self, html: &str, _selector: Option<&str>, base_url: &str) -> Option<Extracted> {
        let doc = Html::parse_document(html);
        let body_sel = Selector::parse("body").ok()?;
        let body = doc.select(&body_sel).next()?.inner_html();
        let content = sanitize_article_html(&body, base_url);
        if text_len(&content) < 400 {
            return None;
        }
        Some(Extracted {
            excerpt: excerpt_from(&content),
            title: document_title(&doc),
            content,
            extractor: "raw",
        })
    }
}

fn document_title(doc: &Html) -> Option<String> {
    let sel = Selector::parse("title").ok()?;
    let text: String = doc.select(&sel).next()?.text().collect();
    let trimmed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Choose the element with the most paragraph text (a decent proxy for the
/// article body) among common containers.
fn pick_main_content(doc: &Html) -> Option<String> {
    // Prefer semantic containers when they carry enough text.
    for sel in CANDIDATE_SELECTORS {
        if let Ok(selector) = Selector::parse(sel) {
            if let Some(el) = doc.select(&selector).next() {
                let html = el.inner_html();
                if text_len(&html) >= MIN_CONTENT_CHARS {
                    return Some(html);
                }
            }
        }
    }

    // Fallback: densest paragraph container.
    let containers = Selector::parse("article, main, section, div").ok()?;
    let paragraphs = Selector::parse("p").ok()?;
    let mut best: Option<(usize, String)> = None;
    for el in doc.select(&containers) {
        let p_text: usize = el
            .select(&paragraphs)
            .map(|p| p.text().map(|t| t.len()).sum::<usize>())
            .sum();
        if p_text > best.as_ref().map(|(n, _)| *n).unwrap_or(0) {
            best = Some((p_text, el.inner_html()));
        }
    }
    best.filter(|(n, _)| *n >= MIN_CONTENT_CHARS)
        .map(|(_, h)| h)
}

/// Sanitize untrusted HTML (strips scripts/styles/handlers; keeps article tags).
pub fn sanitize(html: &str) -> String {
    sanitize_article_html(html, "https://rustss.invalid/")
}

fn text_len(html: &str) -> usize {
    // Approximate visible-text length (tags stripped).
    let mut len = 0usize;
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag && !c.is_whitespace() => len += 1,
            _ => {}
        }
    }
    len
}

fn excerpt_from(html: &str) -> Option<String> {
    let mut text = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    if text.chars().count() <= 280 {
        Some(text)
    } else {
        let mut t: String = text.chars().take(280).collect();
        t.push('…');
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><head><title>  My  Article </title></head><body>
      <nav><a href="/">Home</a><script>evil()</script></nav>
      <article>
        <h1>Heading</h1>
        <p>This is the first substantial paragraph of the article body. It is deliberately
           written to be long enough to comfortably clear the minimum content threshold that
           the readability heuristic uses when deciding whether a candidate container really
           holds the main article text rather than navigation or boilerplate.</p>
        <p>A second paragraph with <a href="https://ex.com">a link</a> and
           <img src="/x.png" onerror="hack()"> an image, plus enough additional prose to keep
           the extracted body clearly above the threshold in every reasonable case.</p>
        <script>tracker()</script>
      </article>
      <footer>© 2026</footer>
    </body></html>"#;

    #[test]
    fn readability_extracts_article_and_sanitizes() {
        let out = ReadabilityExtractor
            .extract(PAGE, None, "https://example.com/article")
            .unwrap();
        assert_eq!(out.title.as_deref(), Some("My Article"));
        assert!(out.content.contains("first substantial paragraph"));
        assert!(out.content.contains("a link"));
        assert!(!out.content.contains("evil()"));
        assert!(!out.content.contains("onerror"));
        assert!(!out.content.contains("tracker()"));
        assert!(out.excerpt.unwrap().contains("first substantial paragraph"));
    }

    #[test]
    fn rendered_content_rewrites_relative_media_urls_and_keeps_safe_media() {
        let html = r#"<figure><img src="/cover.png" alt="Cover"><video controls poster="poster.jpg"><source src="movie.mp4" type="video/mp4"></video></figure><script>bad()</script>"#;
        let content = sanitize_article_html(html, "https://example.com/articles/one");
        assert!(content.contains("https://example.com/cover.png"));
        assert!(content.contains("https://example.com/articles/poster.jpg"));
        assert!(content.contains("https://example.com/articles/movie.mp4"));
        assert!(content.contains("<video controls"));
        assert!(!content.contains("bad()"));
    }

    #[test]
    fn selector_override_is_respected() {
        let html = r#"<body><div class="x"><p>Short.</p></div>
            <div id="target"><p>The targeted content is definitely long enough to be selected as
            the main content of this particular page by the extractor, because we have written
            a good deal of additional explanatory prose here to push it comfortably past the
            minimum content length that the heuristic requires before accepting a container.</p>
            </div></body>"#;
        let out = ReadabilityExtractor
            .extract(html, Some("#target"), "https://example.com/article")
            .unwrap();
        assert!(out.content.contains("targeted content"));
    }

    #[test]
    fn returns_none_when_too_short() {
        let html = "<body><p>tiny</p></body>";
        assert!(ReadabilityExtractor
            .extract(html, None, "https://example.com/article")
            .is_none());
    }
}
