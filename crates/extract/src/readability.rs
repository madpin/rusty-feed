//! HTML → article-content extractors.
//!
//! [`ReadabilityExtractor`] uses a Mozilla-Readability-compatible parser to
//! remove publisher chrome and recover semantic article structure. It honors a
//! per-feed CSS selector and retains the former density heuristic as a fallback
//! for malformed or unusual documents. [`RawExtractor`] is the final,
//! low-priority fallback. All output is sanitized.

use dom_smoothie::{Config, Readability};
use lol_html::{element, rewrite_str, RewriteStrSettings};
use scraper::{Html, Selector};

use rusty_feed_core::{
    html::{sanitize_article_html, sanitize_article_html_with_policies},
    tracking_pixel::TrackingPixelPolicy,
    tracking_url::TrackingUrlPolicy,
};

/// Extracted article content (sanitized HTML).
#[derive(Debug, Clone)]
pub struct Extracted {
    pub content: String,
    pub audio: Vec<rusty_feed_core::article_audio::ArticleAudio>,
    pub excerpt: Option<String>,
    pub title: Option<String>,
    pub extractor: &'static str,
    pub verified_html: bool,
    pub blocked_tracking_pixels: u32,
}

/// A content extractor over already-fetched HTML (fetching is centralized in the
/// SSRF-safe client, so extractors stay pure/testable).
pub trait Extractor: Send + Sync {
    fn name(&self) -> &'static str;
    /// Return content if this extractor found something usable.
    fn extract(
        &self,
        html: &str,
        selector: Option<&str>,
        base_url: &str,
        tracking_pixel_policy: &TrackingPixelPolicy,
    ) -> Option<Extracted>;
}

const MIN_CONTENT_CHARS: usize = 200;
const MAX_DOCUMENT_ELEMENTS: usize = 50_000;
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

    fn extract(
        &self,
        html: &str,
        selector: Option<&str>,
        base_url: &str,
        tracking_pixel_policy: &TrackingPixelPolicy,
    ) -> Option<Extracted> {
        let original_doc = Html::parse_document(html);
        let audio = crate::audio::detect_article_audio(&original_doc, base_url);
        let prepared_html = crate::audio::without_narration_players(html, &original_doc);
        let html = prepared_html.as_str();
        let doc = Html::parse_document(html);
        let document_title = document_title(&doc);
        let custom = selector.and_then(|value| select_custom_content(&doc, value));
        let has_custom = custom.is_some();
        let mut extracted = custom
            .map(|raw| (raw, document_title.clone()))
            .or_else(|| extract_readability(html, base_url))
            .or_else(|| pick_main_content(&doc).map(|raw| (raw, document_title.clone())))?;
        if !has_custom {
            if let Some(whole) = split_body_html(&doc) {
                // Replace only when it recovers more text than the generic pass kept.
                if text_len(&remove_publisher_boilerplate(&whole))
                    > text_len(&remove_publisher_boilerplate(&extracted.0))
                {
                    extracted.0 = whole;
                }
            }
        }

        let title = extracted.1.or(document_title);
        let cleaned = remove_publisher_boilerplate(&extracted.0);
        let report = sanitize_article_html_with_policies(
            &cleaned,
            base_url,
            &TrackingUrlPolicy::default(),
            tracking_pixel_policy,
        );
        let content = report.html;
        let content = remove_duplicate_title_heading(&content, title.as_deref());
        if text_len(&content) < MIN_CONTENT_CHARS {
            return None;
        }
        let excerpt = excerpt_from(&content);
        Some(Extracted {
            audio,
            content,
            excerpt,
            title,
            extractor: "readability",
            verified_html: false,
            blocked_tracking_pixels: report.blocked_tracking_pixels,
        })
    }
}

fn extract_readability(html: &str, base_url: &str) -> Option<(String, Option<String>)> {
    let html = remove_publisher_boilerplate(html);
    let config = Config {
        keep_classes: false,
        max_elements_to_parse: MAX_DOCUMENT_ELEMENTS,
        char_threshold: MIN_CONTENT_CHARS,
        ..Config::default()
    };
    let mut parser = Readability::new(html, Some(base_url), Some(config)).ok()?;
    let article = parser.parse().ok()?;
    let content = article.content.to_string();
    let title = story_title(&article.title, &content);
    Some((content, title))
}

/// Elements that hold one slice of a publisher's article body when the page
/// builds a story from several grid cells (Condé Nast: WIRED, The New Yorker,
/// Vogue, GQ): the text lives in `.body__container` blocks, and gallery
/// guides add one `.gallery__slides__slide` per product.
const SPLIT_BODY_SELECTOR: &str = ".body__container, .gallery__slides__slide";

/// Join the body slices of the `<article>` that holds the most of them, in
/// document order. A generic Readability pass sees these cells as separate
/// candidates and keeps only one, so on WIRED it returned 20-30% of a story.
/// Returns `None` unless one `<article>` holds at least two slices, so a lone
/// teaser block or a block outside the article never triggers it.
fn split_body_html(doc: &Html) -> Option<String> {
    let blocks = Selector::parse(SPLIT_BODY_SELECTOR).ok()?;
    let article = Selector::parse("article").ok()?;
    // Slices grouped by the `<article>` that owns them (the article's position
    // in document order identifies it).
    let mut groups: Vec<(usize, Vec<scraper::ElementRef<'_>>)> = Vec::new();
    for element in doc.select(&blocks) {
        // Skip a slice nested inside another slice: its parent already holds it.
        if element
            .ancestors()
            .filter_map(scraper::ElementRef::wrap)
            .any(|ancestor| blocks.matches(&ancestor))
        {
            continue;
        }
        let Some(owner) = element
            .ancestors()
            .filter_map(scraper::ElementRef::wrap)
            .find(|ancestor| article.matches(ancestor))
        else {
            continue;
        };
        let Some(index) = doc
            .select(&article)
            .position(|candidate| candidate == owner)
        else {
            continue;
        };
        match groups.iter_mut().find(|(known, _)| *known == index) {
            Some((_, slices)) => slices.push(element),
            None => groups.push((index, vec![element])),
        }
    }
    let (_, slices) = groups
        .into_iter()
        .filter(|(_, slices)| slices.len() >= 2)
        .max_by_key(|(_, slices)| slices.iter().map(content_score).sum::<usize>())?;
    Some(
        slices
            .iter()
            .map(|slice| slice.html())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn select_custom_content(doc: &Html, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    let fragments = doc
        .select(&selector)
        .map(|element| element.inner_html())
        .collect::<Vec<_>>();
    (!fragments.is_empty()).then(|| fragments.join("\n"))
}

/// Fallback: sanitize the whole `<body>` (used only if it has substantial text).
pub struct RawExtractor;

impl Extractor for RawExtractor {
    fn name(&self) -> &'static str {
        "raw"
    }

    fn extract(
        &self,
        html: &str,
        _selector: Option<&str>,
        base_url: &str,
        tracking_pixel_policy: &TrackingPixelPolicy,
    ) -> Option<Extracted> {
        let original_doc = Html::parse_document(html);
        let audio = crate::audio::detect_article_audio(&original_doc, base_url);
        let prepared_html = crate::audio::without_narration_players(html, &original_doc);
        let doc = Html::parse_document(&prepared_html);
        let body_sel = Selector::parse("body").ok()?;
        let body = doc.select(&body_sel).next()?.inner_html();
        let cleaned = remove_publisher_boilerplate(&body);
        let report = sanitize_article_html_with_policies(
            &cleaned,
            base_url,
            &TrackingUrlPolicy::default(),
            tracking_pixel_policy,
        );
        let content = report.html;
        if text_len(&content) < 400 {
            return None;
        }
        Some(Extracted {
            audio,
            excerpt: excerpt_from(&content),
            title: document_title(&doc),
            content,
            extractor: "raw",
            verified_html: false,
            blocked_tracking_pixels: report.blocked_tracking_pixels,
        })
    }
}

/// Preserve a small but readable body when browser rendering is unavailable.
///
/// This is deliberately not part of the normal extractor chain: its relaxed
/// threshold would otherwise accept publisher chrome as article content. It is
/// only used after the local extractors and configured Chromium renderer have
/// both failed, so a short public note remains readable during an optional
/// dependency outage.
pub(crate) fn short_outage_fallback(
    html: &str,
    base_url: &str,
    tracking_pixel_policy: &TrackingPixelPolicy,
) -> Option<Extracted> {
    let original_doc = Html::parse_document(html);
    let audio = crate::audio::detect_article_audio(&original_doc, base_url);
    let prepared_html = crate::audio::without_narration_players(html, &original_doc);
    let doc = Html::parse_document(&prepared_html);
    let body_selector = Selector::parse("body").ok()?;
    let body = doc.select(&body_selector).next()?.inner_html();
    let cleaned = remove_publisher_boilerplate(&body);
    let report = sanitize_article_html_with_policies(
        &cleaned,
        base_url,
        &TrackingUrlPolicy::default(),
        tracking_pixel_policy,
    );
    let content = report.html;
    if text_len(&content) < 20 {
        return None;
    }
    Some(Extracted {
        audio,
        excerpt: excerpt_from(&content),
        title: document_title(&doc),
        content,
        extractor: "outage-fallback",
        verified_html: false,
        blocked_tracking_pixels: report.blocked_tracking_pixels,
    })
}

fn document_title(doc: &Html) -> Option<String> {
    let sel = Selector::parse("title").ok()?;
    let text: String = doc.select(&sel).next()?.text().collect();
    normalized_inline(&text)
}

fn normalized_inline(value: &str) -> Option<String> {
    let trimmed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!trimmed.is_empty()).then_some(trimmed)
}

fn story_title(document_title: &str, html: &str) -> Option<String> {
    let title = normalized_inline(document_title)?;
    let fragment = Html::parse_fragment(html);
    let heading = Selector::parse("h1, h2")
        .ok()
        .and_then(|selector| fragment.select(&selector).next())
        .and_then(|element| normalized_inline(&element.text().collect::<Vec<_>>().join(" ")));
    if let Some(heading) = heading.filter(|heading| titles_equivalent(&title, heading)) {
        return Some(heading);
    }
    [" | ", " :: ", " · "]
        .into_iter()
        .find_map(|separator| {
            title
                .split_once(separator)
                .map(|(story, _)| story.to_string())
        })
        .and_then(|story| normalized_inline(&story))
        .or(Some(title))
}

fn remove_duplicate_title_heading(html: &str, title: Option<&str>) -> String {
    let Some(title) = title else {
        return html.to_string();
    };
    let fragment = Html::parse_fragment(html);
    let Ok(headings) = Selector::parse("h1, h2") else {
        return html.to_string();
    };
    let Some(first_heading) = fragment.select(&headings).next() else {
        return html.to_string();
    };
    let heading = first_heading.text().collect::<Vec<_>>().join(" ");
    if !titles_equivalent(title, &heading) {
        return html.to_string();
    }

    let mut first = true;
    rewrite_str(
        html,
        RewriteStrSettings::new().append_element_content_handler(element!(
            "h1, h2",
            move |element| {
                if first {
                    element.remove();
                    first = false;
                }
                Ok(())
            }
        )),
    )
    .unwrap_or_else(|_| html.to_string())
}

fn titles_equivalent(document_title: &str, heading: &str) -> bool {
    let heading = comparable_title(heading);
    if heading.is_empty() {
        return false;
    }
    title_variants(document_title)
        .into_iter()
        .any(|candidate| comparable_title(candidate) == heading)
}

fn title_variants(title: &str) -> Vec<&str> {
    let mut variants = vec![title];
    for separator in [" | ", " — ", " – ", " - ", " :: ", " · "] {
        if let Some((article_title, _)) = title.split_once(separator) {
            variants.push(article_title);
        }
    }
    variants
}

fn comparable_title(title: &str) -> String {
    title
        .chars()
        .flat_map(char::to_lowercase)
        .map(|ch| if ch.is_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn remove_publisher_boilerplate(html: &str) -> String {
    rewrite_str(
        html,
        RewriteStrSettings::new()
            .append_element_content_handler(element!(
                "form, nav, button, [role=navigation], [role=dialog]",
                |element| {
                    element.remove();
                    Ok(())
                }
            ))
            .append_element_content_handler(element!("[class], [id]", |element| {
                let tag = element.tag_name();
                let role = element.get_attribute("role");
                let content_block = is_text_content_block(&tag, role.as_deref());
                if [element.get_attribute("class"), element.get_attribute("id")]
                    .into_iter()
                    .flatten()
                    .any(|value| boilerplate_label(&value, content_block))
                {
                    element.remove();
                }
                Ok(())
            })),
    )
    .unwrap_or_else(|_| html.to_string())
}

/// Whether an element is itself a piece of article text (paragraph, heading,
/// list, quotation...) rather than a wrapper widget.
fn is_text_content_block(tag: &str, role: Option<&str>) -> bool {
    matches!(
        tag,
        "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "li" | "blockquote"
    ) || role.is_some_and(|role| role.eq_ignore_ascii_case("heading"))
}

/// Whether a `class`/`id` value marks publisher chrome. `content_block` says
/// the element is itself article text: Condé Nast sites (WIRED, The New Yorker,
/// Vogue...) put a bare `class="paywall"` on every body paragraph and heading
/// to say the text sits behind the meter, so that word alone must not delete
/// text. Gate widgets (`paywall-modal`, `paywall-bar`, `paywall-message`) and
/// non-text wrappers are still removed.
fn boilerplate_label(value: &str, content_block: bool) -> bool {
    value
        .split_whitespace()
        .filter(|class| !(content_block && class.eq_ignore_ascii_case("paywall")))
        .flat_map(|class| class.split(|ch: char| !ch.is_alphanumeric()))
        .any(|token| {
            matches!(
                token.to_ascii_lowercase().as_str(),
                "share"
                    | "sharing"
                    | "newsletter"
                    | "subscribe"
                    | "subscription"
                    | "related"
                    | "recommended"
                    | "recommendations"
                    | "outbrain"
                    | "taboola"
                    // Cookie/consent + comment widgets.
                    | "cookie"
                    | "cookies"
                    | "consent"
                    | "gdpr"
                    | "comments"
                    | "comment"
                    | "disqus"
                    | "livefyre"
                    // Author bios, paywall gates, social, promos.
                    | "author-bio"
                    | "authorbio"
                    | "byline"
                    | "paywall"
                    | "subscriber"
                    | "socials"
                    | "promo"
                    | "promotion"
                    | "advertisement"
                    | "sponsored"
                    | "breadcrumb"
                    | "breadcrumbs"
            )
        })
}

/// Choose the element with the most paragraph text (a decent proxy for the
/// article body) among common containers.
fn pick_main_content(doc: &Html) -> Option<String> {
    // Prefer semantic containers when they carry enough text. Publishers often
    // render several teaser <article> elements before the real story, so score
    // every match instead of accepting the first one in document order.
    for sel in CANDIDATE_SELECTORS {
        if let Ok(selector) = Selector::parse(sel) {
            if let Some((_, html)) = doc
                .select(&selector)
                .map(|element| {
                    let html = element.inner_html();
                    (content_score(&element), html)
                })
                .filter(|(_, html)| text_len(html) >= MIN_CONTENT_CHARS)
                .max_by_key(|(score, _)| *score)
            {
                return Some(html);
            }
        }
    }

    // Fallback: densest paragraph container.
    let containers = Selector::parse("article, main, section, div").ok()?;
    let paragraphs = Selector::parse("p").ok()?;
    let mut best: Option<(usize, String)> = None;
    for el in doc.select(&containers) {
        let paragraph_text = el
            .select(&paragraphs)
            .map(|paragraph| paragraph.text().collect::<String>())
            .map(|text| text.chars().filter(|ch| !ch.is_whitespace()).count())
            .sum::<usize>();
        let score = paragraph_text.saturating_mul(2) + content_score(&el);
        if score > best.as_ref().map(|(score, _)| *score).unwrap_or(0) {
            best = Some((score, el.inner_html()));
        }
    }
    best.filter(|(_, html)| text_len(html) >= MIN_CONTENT_CHARS)
        .map(|(_, h)| h)
}

fn content_score(element: &scraper::ElementRef<'_>) -> usize {
    let text = element
        .text()
        .collect::<String>()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .count();
    let link_text = Selector::parse("a")
        .ok()
        .map(|selector| {
            element
                .select(&selector)
                .map(|link| link.text().collect::<String>())
                .map(|text| text.chars().filter(|ch| !ch.is_whitespace()).count())
                .sum::<usize>()
        })
        .unwrap_or_default();
    text.saturating_sub(link_text)
}

/// Sanitize untrusted HTML (strips scripts/styles/handlers; keeps article tags).
pub fn sanitize(html: &str) -> String {
    sanitize_article_html(html, "https://rusty-feed.invalid/")
}

fn text_len(html: &str) -> usize {
    visible_text(html)
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .count()
}

fn excerpt_from(html: &str) -> Option<String> {
    let text = visible_text(html)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
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

fn visible_text(html: &str) -> String {
    Html::parse_fragment(html)
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel_policy() -> TrackingPixelPolicy {
        TrackingPixelPolicy::default()
    }

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
    fn narration_is_shared_metadata_even_outside_the_selected_body() {
        let html = format!(
            "{}{}",
            include_str!("../tests/fixtures/google_tts.html"),
            PAGE
        );
        for extractor in [&ReadabilityExtractor as &dyn Extractor, &RawExtractor] {
            let out = extractor
                .extract(
                    &html,
                    Some("article"),
                    "https://example.com/article",
                    &pixel_policy(),
                )
                .unwrap();
            assert_eq!(out.audio.len(), 2);
            assert_eq!(out.audio[0].voice_name, "Umbriel");
            assert!(!out.content.contains("<audio"));
            assert!(!out.content.contains("storage.googleapis.com"));
            assert!(out.content.contains("first substantial paragraph"));
        }
    }

    #[test]
    fn readability_extracts_article_and_sanitizes() {
        let out = ReadabilityExtractor
            .extract(PAGE, None, "https://example.com/article", &pixel_policy())
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
            .extract(
                html,
                Some("#target"),
                "https://example.com/article",
                &pixel_policy(),
            )
            .unwrap();
        assert!(out.content.contains("targeted content"));
    }

    #[test]
    fn chooses_the_substantial_article_instead_of_the_first_teaser() {
        let html = format!(
            r#"<body>
              <article><p>{}</p></article>
              <article><h1>Actual story</h1><p>{}</p><p>{}</p></article>
            </body>"#,
            "Teaser words and links. ".repeat(12),
            "The complete story has substantially more useful prose &amp; context. ".repeat(12),
            "Its second section makes this the clear extraction candidate. ".repeat(10),
        );
        let out = ReadabilityExtractor
            .extract(&html, None, "https://example.com/story", &pixel_policy())
            .unwrap();
        assert!(out.content.contains("Actual story"));
        assert!(out.content.contains("second section"));
        assert!(!out.content.contains("Teaser words"));
        assert!(out.excerpt.unwrap().contains("prose & context"));
    }

    #[test]
    fn removes_publisher_chrome_and_duplicate_story_heading() {
        let prose = "The reporting in this paragraph contains the complete story, useful context, and enough detail for a reader to understand the subject without publisher navigation. ".repeat(4);
        let html = format!(
            r#"<html><head><title>A clear story | Daily Example</title></head><body>
              <header><nav>Sections Subscribe Sign in</nav></header>
              <main><article>
                <div class="share-tools">Share on every social network</div>
                <h1>A clear story</h1>
                <p>{prose}</p>
                <h2>What happens next</h2>
                <p>{prose}</p>
                <blockquote><p>An important quotation.</p></blockquote>
                <ul><li>First consequence</li><li>Second consequence</li></ul>
                <form class="newsletter"><label>Get our newsletter<input></label></form>
                <aside class="related">Related stories you may have missed</aside>
              </article></main>
              <footer>Terms Privacy Cookies</footer>
            </body></html>"#
        );
        let out = ReadabilityExtractor
            .extract(
                &html,
                None,
                "https://example.com/news/story",
                &pixel_policy(),
            )
            .unwrap();

        assert_eq!(out.title.as_deref(), Some("A clear story"));
        assert!(!out.content.contains("<h1"));
        assert!(!out.content.contains("Subscribe Sign in"));
        assert!(!out.content.contains("Share on every social network"));
        assert!(!out.content.contains("Get our newsletter"));
        assert!(!out.content.contains("Related stories"));
        assert!(out.content.contains("What happens next"));
        assert!(out.content.contains("An important quotation"));
        assert!(out.content.contains("First consequence"));

        let markdown = rusty_feed_core::html::article_html_to_markdown(
            &out.content,
            "https://example.com/news/story",
        );
        assert!(markdown.contains("## What happens next"));
        assert!(markdown.contains("> An important quotation."));
        assert!(markdown.contains("- First consequence"));
    }

    #[test]
    fn selector_override_keeps_all_matching_sections() {
        let paragraph = "A selected section contains sufficient useful article text for the configured publisher rule to preserve it during extraction. ".repeat(3);
        let html = format!(
            r#"<body><section class="chapter"><p>{paragraph}</p></section>
                <section class="chapter"><h2>Part two</h2><p>{paragraph}</p></section></body>"#
        );
        let out = ReadabilityExtractor
            .extract(
                &html,
                Some(".chapter"),
                "https://example.com/story",
                &pixel_policy(),
            )
            .unwrap();
        assert_eq!(out.content.matches(&paragraph).count(), 2);
        assert!(out.content.contains("Part two"));
    }

    #[test]
    fn returns_none_when_too_short() {
        let html = "<body><p>tiny</p></body>";
        assert!(ReadabilityExtractor
            .extract(html, None, "https://example.com/article", &pixel_policy())
            .is_none());
    }

    #[test]
    fn keeps_body_text_a_publisher_tags_paywall() {
        let prose = "The reporting in this paragraph is the actual story and must survive extraction even though the publisher marks every body block with a paywall class. ".repeat(3);
        let html = format!(
            r#"<html><head><title>A metered story | Daily Example</title></head><body>
              <article>
                <h1>A metered story</h1>
                <div class="body__inner-container">
                  <p class="paywall">{prose}</p>
                  <h2 class="paywall">Second section</h2>
                  <div role="heading" aria-level="4" class="paywall heading-h4"><strong>Phone model: Example</strong></div>
                  <p class="paywall">{prose}</p>
                  <ul class="paywall"><li>First kept item</li><li>Second kept item</li></ul>
                </div>
                <div class="PaywallBarWrapper-gFBZnd paywall-bar"><p>Power up with unlimited access. Subscribe Today.</p></div>
                <div class="paywall-modal paywall-modal--paywall"><p>Sign in to keep reading this story.</p></div>
              </article></body></html>"#
        );
        let out = ReadabilityExtractor
            .extract(&html, None, "https://example.com/story", &pixel_policy())
            .unwrap();
        assert_eq!(out.content.matches(prose.trim()).count(), 2);
        assert!(out.content.contains("Second section"));
        assert!(out.content.contains("Phone model: Example"));
        assert!(out.content.contains("First kept item"));
        assert!(!out.content.contains("Subscribe Today"));
        assert!(!out.content.contains("Sign in to keep reading"));
    }

    #[test]
    fn paywall_gates_stay_removed_even_when_they_wrap_a_paragraph() {
        let prose = "A complete paragraph of real reporting that is long enough to count as the article body for the extractor, repeated to clear the minimum. ".repeat(4);
        let html = format!(
            r#"<body><article><p>{prose}</p>
              <p class="paywall-message">Subscribe to read the rest of this article.</p>
              <section class="paywall"><p>Already a subscriber? Sign in.</p></section>
            </article></body>"#
        );
        let out = ReadabilityExtractor
            .extract(&html, None, "https://example.com/story", &pixel_policy())
            .unwrap();
        assert!(out
            .content
            .contains("A complete paragraph of real reporting"));
        assert!(!out.content.contains("Subscribe to read the rest"));
        assert!(!out.content.contains("Already a subscriber"));
    }

    /// One shaped like a Condé Nast review: the body is split into several
    /// `body__inner-container` chunks, each in its own grid cell, beside a rating
    /// card and a link-heavy recirculation block. Readability alone keeps just
    /// one chunk.
    fn chunked_review_html() -> String {
        let chunk = |marker: &str| {
            let sentence = format!(
                "{marker} describes one measured aspect of the product in full sentences, with the figures and the context a reader needs to follow the argument. "
            );
            format!(
                r#"<div class="GridWrapper-uulHE grid"><div class="GridItem-bufwYC"><div class="BodyWrapper-cuqCAc body body__container article__body" id="BodyWrapper"><div class="body__inner-container"><p>{}</p><p>{}</p></div></div></div></div>"#,
                sentence.repeat(3),
                sentence.repeat(3)
            )
        };
        format!(
            r#"<html><head><title>Example Purifier Review | WIRED</title></head><body>
              <header><nav>Sections Sign in</nav></header>
              <article data-testid="ReviewPageArticle" class="story">
                <div class="ContentHeaderContainer"><h1>Example Purifier Review</h1><a href="/author/a">Author One</a></div>
                <div class="ArticlePageContentBackGround">
                  <div class="GridWrapper-uulHE"><div class="GridItem-bufwYC"><div class="ProsConsWrapper">
                    <p>WIRED: Quiet. Looks good.</p><p>TIRED: No display. No remote.</p></div></div></div>
                  <div class="ArticlePageChunks-lfyGNk">{}{}{}{}</div>
                </div>
              </article>
              <div class="body__container"><p>{}</p></div>
              <footer>Terms Privacy</footer>
            </body></html>"#,
            chunk("FIRSTCHUNK"),
            chunk("SECONDCHUNK"),
            chunk("THIRDCHUNK"),
            chunk("FOURTHCHUNK"),
            "Teaser outside the article about something else entirely, long enough to count as text on its own for any scoring heuristic. ".repeat(3)
        )
    }

    #[test]
    fn keeps_every_body_chunk_of_a_chunked_article() {
        let out = ReadabilityExtractor
            .extract(
                &chunked_review_html(),
                None,
                "https://example.com/review/example",
                &pixel_policy(),
            )
            .unwrap();
        for marker in ["FIRSTCHUNK", "SECONDCHUNK", "THIRDCHUNK", "FOURTHCHUNK"] {
            assert!(out.content.contains(marker), "{marker} was dropped");
        }
        assert!(!out.content.contains("Teaser outside the article"));
        assert!(!out.content.contains("Sections Sign in"));
        assert_eq!(out.title.as_deref(), Some("Example Purifier Review"));
        assert!(!out.content.contains("<h1"));
    }

    #[test]
    fn a_configured_selector_wins_over_the_chunk_rule() {
        let out = ReadabilityExtractor
            .extract(
                &chunked_review_html(),
                Some(".ArticlePageChunks-lfyGNk > .GridWrapper-uulHE:first-child"),
                "https://example.com/review/example",
                &pixel_policy(),
            )
            .unwrap();
        // The feed owner's selector decides: the chunk rule must not widen it.
        assert!(out.content.contains("FIRSTCHUNK"));
        assert!(!out.content.contains("SECONDCHUNK"));
    }

    #[test]
    fn split_body_needs_two_slices_inside_one_article() {
        let slice = |text: &str| format!(r#"<div class="body__container"><p>{text}</p></div>"#);
        let doc = |html: &str| Html::parse_document(html);

        // Two slices in one article, in document order.
        let joined = split_body_html(&doc(&format!(
            "<body><article>{}{}</article></body>",
            slice("first"),
            slice("second")
        )))
        .unwrap();
        assert!(joined.find("first").unwrap() < joined.find("second").unwrap());

        // A single slice, slices outside any article, and slices spread over
        // different articles never qualify.
        assert!(split_body_html(&doc(&format!(
            "<body><article>{}</article></body>",
            slice("only")
        )))
        .is_none());
        assert!(split_body_html(&doc(&format!(
            "<body><main>{}{}</main></body>",
            slice("a"),
            slice("b")
        )))
        .is_none());
        assert!(split_body_html(&doc(&format!(
            "<body><article>{}</article><article>{}</article></body>",
            slice("a"),
            slice("b")
        )))
        .is_none());

        // A slice nested inside another slice is counted once.
        let nested = split_body_html(&doc(
            r#"<body><article><div class="body__container"><div class="body__container"><p>inner</p></div></div>
               <div class="body__container"><p>next</p></div></article></body>"#,
        ))
        .unwrap();
        assert_eq!(nested.matches("inner").count(), 1);
    }

    #[test]
    fn gallery_slides_join_the_surrounding_text() {
        let prose = "A buying guide paragraph with enough prose to matter, naming the model and the reason it earned its place in the list. ".repeat(5);
        let html = format!(
            r#"<html><head><title>The Best Example Laptops | WIRED</title></head><body>
              <article>
                <div class="GalleryPageTextBlock"><div class="body__container"><p>INTROTEXT {prose}</p></div></div>
                <ul>
                  <li class="gallery__slides__slide"><div><h2>Best Overall</h2><p>SLIDEONE {prose}</p></div></li>
                  <li class="gallery__slides__slide"><div><h2>Best Cheap</h2><p>SLIDETWO {prose}</p></div></li>
                </ul>
              </article></body></html>"#
        );
        let doc = Html::parse_document(&html);
        let joined = split_body_html(&doc).unwrap();
        for marker in ["INTROTEXT", "SLIDEONE", "SLIDETWO"] {
            assert!(joined.contains(marker), "{marker} missing");
        }
    }

    #[test]
    fn removes_buttons_from_extracted_content() {
        let prose = "A product guide paragraph with enough prose to be the article body, including the specification a reader compares across models. ".repeat(5);
        let html = format!(
            r#"<body><article><p>{prose}</p>
              <button type="button"><span>Save to wishlist</span></button>
              <div class="toc"><button role="button"><span>AccordionItemContainerButton</span></button><ul><li>Why you might not upgrade</li></ul></div>
              <p>{prose}</p></article></body>"#
        );
        let out = ReadabilityExtractor
            .extract(
                &html,
                Some("article"),
                "https://example.com/guide",
                &pixel_policy(),
            )
            .unwrap();
        assert!(out.content.contains("Why you might not upgrade"));
        assert!(!out.content.contains("Save to wishlist"));
        assert!(!out.content.contains("AccordionItemContainerButton"));
    }
}
