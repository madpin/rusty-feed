use std::{cell::Cell, collections::HashSet};

use ammonia::{Builder, Url, UrlRelative};
use htmd::options::{BrStyle, BulletListMarker, Options as MarkdownOptions};
use htmd::HtmlToMarkdown;
use lol_html::html_content::ContentType;
use lol_html::{element, rewrite_str, RewriteStrSettings};
use pulldown_cmark::{html, Options, Parser};
use scraper::Html;

use crate::tracking_pixel::TrackingPixelPolicy;
use crate::tracking_url::TrackingUrlPolicy;

/// Version of the shared public-page extraction and cleanup representation.
/// Incrementing this makes older copies eligible for one shared refresh.
pub const ARTICLE_PAGE_PROCESSING_VERSION: i32 = 4;

/// Sanitized shared content plus privacy cleanup metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SanitizedArticleHtml {
    pub html: String,
    pub blocked_tracking_pixels: u32,
}

/// Normalize feed-provided text into the sanitized semantic HTML stored by Rusty Feed.
///
/// Feed formats distinguish HTML, Markdown, and plain text. Treating all three as
/// HTML loses line breaks and can interpret literal angle brackets as elements.
pub fn normalize_article_content(content: &str, content_type: &str, base_url: &str) -> String {
    normalize_article_content_with_policy(
        content,
        content_type,
        base_url,
        &TrackingUrlPolicy::default(),
    )
}

/// Normalize feed content while applying the instance's URL privacy policy.
pub fn normalize_article_content_with_policy(
    content: &str,
    content_type: &str,
    base_url: &str,
    policy: &TrackingUrlPolicy,
) -> String {
    normalize_article_content_with_policies(
        content,
        content_type,
        base_url,
        policy,
        &TrackingPixelPolicy::default(),
    )
    .html
}

/// Normalize feed content with the shared URL and tracking-image policies.
pub fn normalize_article_content_with_policies(
    content: &str,
    content_type: &str,
    base_url: &str,
    url_policy: &TrackingUrlPolicy,
    pixel_policy: &TrackingPixelPolicy,
) -> SanitizedArticleHtml {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let html = match media_type.as_str() {
        "text/html" | "application/xhtml+xml" => content.to_string(),
        "text/markdown" | "text/x-markdown" => markdown_to_html(content),
        _ => plain_text_to_html(content),
    };
    sanitize_article_html_with_policies(&html, base_url, url_policy, pixel_policy)
}

/// Convert publisher HTML to clean, portable Markdown for AI and text analysis.
///
/// Sanitization and URL resolution happen first so active content never reaches
/// downstream consumers and relative links remain meaningful outside the page.
pub fn article_html_to_markdown(html: &str, base_url: &str) -> String {
    let sanitized = sanitize_article_html(html, base_url);
    let converter = HtmlToMarkdown::builder()
        .options(MarkdownOptions {
            br_style: BrStyle::Backslash,
            bullet_list_marker: BulletListMarker::Dash,
            ul_bullet_spacing: 1,
            ol_number_spacing: 1,
            ..MarkdownOptions::default()
        })
        .build();
    converter
        .convert(&sanitized)
        .map(|markdown| normalize_markdown(&markdown))
        .unwrap_or_else(|_| fallback_text(&sanitized))
}

/// Count normalized plain-text characters in a sanitized article body.
pub fn article_plain_text_len(html: &str) -> usize {
    let document = Html::parse_fragment(html);
    let mut count = 0usize;
    let mut has_word = false;
    for word in document
        .root_element()
        .text()
        .flat_map(str::split_whitespace)
    {
        if has_word {
            count = count.saturating_add(1);
        }
        count = count.saturating_add(word.chars().count());
        has_word = true;
    }
    count
}

/// Apply a personal navigation target to already-sanitized article links.
///
/// The shared body stays unchanged. Opener and referrer isolation are
/// reasserted in both modes so choosing the current tab never weakens privacy.
pub fn apply_external_link_navigation(html: &str, new_tab: bool) -> String {
    rewrite_str(
        html,
        RewriteStrSettings::new().append_element_content_handler(element!(
            "a[href]",
            move |element| {
                if new_tab {
                    element.set_attribute("target", "_blank")?;
                } else {
                    element.remove_attribute("target");
                }
                element.set_attribute("rel", "noopener noreferrer")?;
                element.set_attribute("referrerpolicy", "no-referrer")?;
                Ok(())
            }
        )),
    )
    .unwrap_or_else(|_| html.to_string())
}

pub fn sanitize_article_html(html: &str, base_url: &str) -> String {
    sanitize_article_html_with_policy(html, base_url, &TrackingUrlPolicy::default())
}

pub fn sanitize_article_preview_html(html: &str, base_url: &str) -> String {
    let sanitized = sanitize_article_html(html, base_url);
    rewrite_str(
        &sanitized,
        RewriteStrSettings::new().append_element_content_handler(element!(
            "img, picture, source, audio, video",
            |element| {
                element.remove();
                Ok(())
            }
        )),
    )
    .unwrap_or_else(|_| fallback_text(&sanitized))
}

/// Sanitize article HTML and canonicalize every surviving HTTP(S) link.
pub fn sanitize_article_html_with_policy(
    html: &str,
    base_url: &str,
    policy: &TrackingUrlPolicy,
) -> String {
    sanitize_article_html_with_policies(html, base_url, policy, &TrackingPixelPolicy::default())
        .html
}

/// Upper bound on stored/rendered article body length. A large-but-under-fetch-
/// cap body is a shared cost (it bloats the DB and every render), so cap it once
/// at the sanitize choke point with a visible notice (FND-194). The input is
/// truncated at a char boundary and then sanitized, so the parser re-balances
/// any tag cut mid-way.
pub const ARTICLE_MAX_STORED_CHARS: usize = 400_000;

/// Sanitize article HTML and report images removed by the shared privacy policy.
pub fn sanitize_article_html_with_policies(
    html: &str,
    base_url: &str,
    url_policy: &TrackingUrlPolicy,
    pixel_policy: &TrackingPixelPolicy,
) -> SanitizedArticleHtml {
    let capped: std::borrow::Cow<'_, str> = if html.chars().count() > ARTICLE_MAX_STORED_CHARS {
        let mut truncated: String = html.chars().take(ARTICLE_MAX_STORED_CHARS).collect();
        truncated.push_str("<p><em>Content truncated — open the original.</em></p>");
        std::borrow::Cow::Owned(truncated)
    } else {
        std::borrow::Cow::Borrowed(html)
    };
    let html = capped.as_ref();
    let (prepared, blocked_tracking_pixels) =
        prepare_media(html, base_url, pixel_policy).unwrap_or_else(|_| (html.to_string(), 0));
    let relative = Url::parse(base_url)
        .map(UrlRelative::RewriteWithBase)
        .unwrap_or(UrlRelative::Deny);
    let sanitized = Builder::default()
        .url_schemes(
            ["http", "https", "mailto"]
                .into_iter()
                .collect::<HashSet<_>>(),
        )
        .add_tags(&["audio", "picture", "source", "tfoot", "video"])
        .add_tag_attributes("audio", &["controls", "preload"])
        .add_tag_attributes("details", &["open"])
        .add_tag_attributes(
            "img",
            &["decoding", "loading", "referrerpolicy", "sizes", "srcset"],
        )
        .add_tag_attributes("li", &["value"])
        .add_tag_attributes("ol", &["reversed", "type"])
        .add_tag_attributes("source", &["media", "sizes", "src", "srcset", "type"])
        .add_tag_attributes("time", &["datetime"])
        .add_tag_attributes(
            "video",
            &["controls", "height", "poster", "preload", "width"],
        )
        .url_relative(relative)
        .clean(&prepared)
        .to_string();
    let sanitized = crate::image_dedup::dedupe_images(&sanitized);
    SanitizedArticleHtml {
        html: harden_content(&sanitized, url_policy).unwrap_or(sanitized),
        blocked_tracking_pixels,
    }
}

fn markdown_to_html(markdown: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let parser = Parser::new_ext(markdown, options);
    let mut output = String::new();
    html::push_html(&mut output, parser);
    output
}

/// Render generated Markdown through the same HTML sanitizer used for stored
/// content, while preserving root-relative application links.
pub fn markdown_to_sanitized_html(markdown: &str) -> String {
    let html = markdown_to_html(markdown);
    let sanitized = Builder::default()
        .url_schemes(
            ["http", "https", "mailto"]
                .into_iter()
                .collect::<HashSet<_>>(),
        )
        .url_relative(UrlRelative::PassThrough)
        .clean(&html)
        .to_string();
    harden_content(&sanitized, &TrackingUrlPolicy::default()).unwrap_or(sanitized)
}

fn plain_text_to_html(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut output = String::new();
    let mut paragraph = Vec::new();
    for line in normalized.lines() {
        if line.trim().is_empty() {
            push_plain_paragraph(&mut output, &paragraph);
            paragraph.clear();
        } else {
            paragraph.push(line);
        }
    }
    push_plain_paragraph(&mut output, &paragraph);
    output
}

fn push_plain_paragraph(output: &mut String, lines: &[&str]) {
    if lines.is_empty() {
        return;
    }
    output.push_str("<p>");
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            output.push_str("<br>");
        }
        escape_html(output, line);
    }
    output.push_str("</p>");
}

fn escape_html(output: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(ch),
        }
    }
}

fn prepare_media(
    html: &str,
    base_url: &str,
    policy: &TrackingPixelPolicy,
) -> Result<(String, u32), lol_html::errors::RewritingError> {
    let base = Url::parse(base_url).ok();
    let image_base = base.clone();
    let policy = policy.clone();
    let youtube_policy = crate::youtube::YouTubePolicy::default();
    let blocked = Cell::new(0_u32);
    let output = rewrite_str(
        html,
        RewriteStrSettings::new()
            .append_element_content_handler(element!("img", |element| {
                let current = element.get_attribute("src").unwrap_or_default();
                let mut restored_lazy_source = false;
                if current.trim().is_empty() || current.starts_with("data:") {
                    for attribute in [
                        "data-src",
                        "data-lazy-src",
                        "data-original",
                        "data-original-src",
                        "data-url",
                        "data-hi-res-src",
                        "data-actualsrc",
                        "data-echo",
                        "data-image-src",
                    ] {
                        if let Some(candidate) = element.get_attribute(attribute) {
                            if !candidate.trim().is_empty() {
                                element.set_attribute("src", candidate.trim())?;
                                restored_lazy_source = true;
                                break;
                            }
                        }
                    }
                }
                // Recover a lazily-declared srcset into a sanitized srcset when the
                // element has none (FND: content-cleaning §6.3).
                let has_srcset = element
                    .get_attribute("srcset")
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false);
                if !has_srcset {
                    for attribute in ["data-srcset", "data-lazy-srcset", "data-original-set"] {
                        if let Some(candidate) = element.get_attribute(attribute) {
                            if let Some(safe) = sanitize_srcset(&candidate, image_base.as_ref()) {
                                element.set_attribute("srcset", &safe)?;
                            }
                            break;
                        }
                    }
                }
                if is_tracking_pixel(element, restored_lazy_source, image_base.as_ref(), &policy) {
                    blocked.set(blocked.get().saturating_add(1));
                    element.remove();
                }
                Ok(())
            }))
            .append_element_content_handler(element!(
                "img[srcset], source[srcset]",
                move |element| {
                    if let Some(srcset) = element.get_attribute("srcset") {
                        if let Some(safe) = sanitize_srcset(&srcset, base.as_ref()) {
                            element.set_attribute("srcset", &safe)?;
                        } else {
                            element.remove_attribute("srcset");
                        }
                    }
                    Ok(())
                }
            ))
            .append_element_content_handler(element!("iframe[src]", move |element| {
                let Some(source) = element.get_attribute("src") else {
                    return Ok(());
                };
                let Some(video) = youtube_policy.video_from_url(&source) else {
                    return Ok(());
                };
                let url = html_escape::encode_double_quoted_attribute(&video.watch_url);
                element.replace(
                    &format!("<p><a href=\"{url}\">Watch video on YouTube</a></p>"),
                    ContentType::Html,
                );
                Ok(())
            })),
    )?;
    Ok((output, blocked.get()))
}

fn is_tracking_pixel(
    element: &lol_html::html_content::Element<'_, '_>,
    restored_lazy_source: bool,
    base_url: Option<&Url>,
    policy: &TrackingPixelPolicy,
) -> bool {
    if !restored_lazy_source && (has_tiny_dimensions(element) || has_hidden_style(element)) {
        return true;
    }
    let Some(source) = element
        .get_attribute("src")
        .filter(|source| !source.trim().is_empty())
    else {
        return false;
    };
    let resolved = Url::parse(source.trim())
        .ok()
        .or_else(|| base_url.and_then(|base| base.join(source.trim()).ok()));
    resolved.is_some_and(|url| policy.blocks_url(url.as_str()))
}

fn has_tiny_dimensions(element: &lol_html::html_content::Element<'_, '_>) -> bool {
    let dimension = |name| {
        element
            .get_attribute(name)
            .and_then(|value| value.trim().parse::<u32>().ok())
    };
    matches!(
        (dimension("width"), dimension("height")),
        (Some(0..=2), Some(0..=2))
    )
}

fn has_hidden_style(element: &lol_html::html_content::Element<'_, '_>) -> bool {
    if element.get_attribute("hidden").is_some() {
        return true;
    }
    let Some(style) = element.get_attribute("style") else {
        return false;
    };
    let declarations = style
        .split(';')
        .filter_map(|declaration| declaration.split_once(':'))
        .map(|(name, value)| {
            let value = value.trim().to_ascii_lowercase();
            let value = value
                .strip_suffix("!important")
                .unwrap_or(&value)
                .trim()
                .to_string();
            (name.trim().to_ascii_lowercase(), value)
        })
        .collect::<Vec<_>>();
    let css_value = |name: &str| {
        declarations
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    };
    matches!(css_value("display"), Some("none"))
        || matches!(css_value("visibility"), Some("hidden" | "collapse"))
        || css_value("opacity").is_some_and(|value| {
            value
                .trim_end_matches('%')
                .parse::<f32>()
                .is_ok_and(|opacity| opacity <= 0.0)
        })
        || matches!(
            (
                css_pixels(css_value("width")),
                css_pixels(css_value("height"))
            ),
            (Some(0.0..=2.0), Some(0.0..=2.0))
        )
}

fn css_pixels(value: Option<&str>) -> Option<f32> {
    let value = value?.trim();
    let value = value.strip_suffix("px").unwrap_or(value);
    value.parse::<f32>().ok()
}

fn sanitize_srcset(srcset: &str, base: Option<&Url>) -> Option<String> {
    let mut safe = Vec::new();
    for candidate in srcset.split(',') {
        let mut parts = candidate.split_whitespace();
        let Some(raw_url) = parts.next() else {
            continue;
        };
        let descriptor = parts.next();
        if parts.next().is_some() {
            continue;
        }
        let url = match Url::parse(raw_url) {
            Ok(url) => url,
            Err(_) => {
                let Some(url) = base.and_then(|base| base.join(raw_url).ok()) else {
                    continue;
                };
                url
            }
        };
        if !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        if descriptor.is_some_and(|value| !valid_srcset_descriptor(value)) {
            continue;
        }
        let mut value = url.to_string();
        if let Some(descriptor) = descriptor {
            value.push(' ');
            value.push_str(descriptor);
        }
        safe.push(value);
    }
    (!safe.is_empty()).then(|| safe.join(", "))
}

fn valid_srcset_descriptor(value: &str) -> bool {
    value
        .strip_suffix('w')
        .is_some_and(|number| number.parse::<u32>().is_ok_and(|number| number > 0))
        || value
            .strip_suffix('x')
            .is_some_and(|number| number.parse::<f32>().is_ok_and(|number| number > 0.0))
}

fn harden_content(
    html: &str,
    policy: &TrackingUrlPolicy,
) -> Result<String, lol_html::errors::RewritingError> {
    rewrite_str(
        html,
        RewriteStrSettings::new()
            .append_element_content_handler(element!("a[href]", |element| {
                if let Some(href) = element.get_attribute("href") {
                    let href = html_escape::decode_html_entities(&href);
                    element.set_attribute("href", &policy.clean(&href))?;
                }
                element.set_attribute("target", "_blank")?;
                element.set_attribute("rel", "noopener noreferrer")?;
                element.set_attribute("referrerpolicy", "no-referrer")?;
                Ok(())
            }))
            .append_element_content_handler(element!("img[src]", |element| {
                element.set_attribute("loading", "lazy")?;
                element.set_attribute("decoding", "async")?;
                element.set_attribute("referrerpolicy", "no-referrer")?;
                Ok(())
            })),
    )
}

fn fallback_text(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            value if !in_tag => text.push(value),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_markdown(markdown: &str) -> String {
    let normalized = markdown
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\u{a0}', " ");
    let mut output = String::with_capacity(normalized.len());
    let mut blank = false;
    let mut fence: Option<(char, usize)> = None;

    for line in normalized.lines() {
        let trimmed = line.trim_end();
        let marker = markdown_fence(trimmed);
        let was_in_fence = fence.is_some();
        let closes_fence = fence.is_some_and(|(ch, minimum)| {
            marker.is_some_and(|(candidate, count, suffix)| {
                candidate == ch && count >= minimum && suffix.trim().is_empty()
            })
        });
        let opens_fence = if was_in_fence {
            None
        } else {
            marker.map(|(ch, count, _)| (ch, count))
        };

        if !was_in_fence && opens_fence.is_none() && trimmed.is_empty() {
            if blank || output.is_empty() {
                continue;
            }
            blank = true;
            output.push('\n');
            continue;
        }

        blank = false;
        output.push_str(if was_in_fence || opens_fence.is_some() {
            line
        } else {
            trimmed
        });
        output.push('\n');
        if closes_fence {
            fence = None;
        } else if let Some(opening) = opens_fence {
            fence = Some(opening);
        }
    }

    output.trim().to_string()
}

fn markdown_fence(line: &str) -> Option<(char, usize, &str)> {
    let line = line.trim_start();
    let ch = line.chars().next()?;
    if !matches!(ch, '`' | '~') {
        return None;
    }
    let count = line
        .chars()
        .take_while(|candidate| *candidate == ch)
        .count();
    (count >= 3).then(|| (ch, count, &line[count..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_only_largest_duplicate_image() {
        let html = r#"<figure><img src="https://x.com/u/p-300x200.jpg" width="300"></figure><noscript><img src="https://x.com/u/p-1024x683.jpg"></noscript><p>Text</p>"#;
        let content = sanitize_article_html(html, "https://example.com/a");
        assert_eq!(content.matches("<img").count(), 1);
        assert!(content.contains("p-1024x683.jpg"));
        assert!(!content.contains("<figure></figure>"));
        assert!(content.contains("Text"));
    }

    #[test]
    fn preserves_safe_rich_content_and_absolutizes_relative_urls() {
        let html = r#"<figure><img src="/cover.png" alt="Cover"><video controls poster="poster.jpg"><source src="movie.mp4" type="video/mp4"></video><table><tfoot><tr><td>Cell</td></tr></tfoot></table></figure>"#;
        let content = sanitize_article_html(html, "https://example.com/articles/one");
        assert!(content.contains("https://example.com/cover.png"));
        assert!(content.contains("https://example.com/articles/poster.jpg"));
        assert!(content.contains("https://example.com/articles/movie.mp4"));
        assert!(content.contains("loading=\"lazy\""));
        assert!(content.contains("<video controls"));
        assert!(content.contains("<tfoot>"));
    }

    #[test]
    fn preview_html_keeps_structure_without_remote_media() {
        let content = sanitize_article_preview_html(
            r#"<h2>Guide</h2><p>Read <strong>carefully</strong> and <a href="/more">continue</a>.</p><picture><source srcset="/small.jpg"><img src="/large.jpg"></picture><audio src="/clip.mp3"></audio><script>bad()</script>"#,
            "https://example.com/articles/one",
        );
        assert!(content.contains("<h2>Guide</h2>"));
        assert!(content.contains("<strong>carefully</strong>"));
        assert!(content.contains("href=\"https://example.com/more\""));
        assert!(!content.contains("small.jpg"));
        assert!(!content.contains("large.jpg"));
        assert!(!content.contains("clip.mp3"));
        assert!(!content.contains("bad()"));
    }

    #[test]
    fn caps_oversized_bodies_with_a_notice() {
        let huge = format!("<p>{}</p>", "word ".repeat(ARTICLE_MAX_STORED_CHARS));
        let out = sanitize_article_html(&huge, "https://example.com");
        assert!(
            out.chars().count() < ARTICLE_MAX_STORED_CHARS + 200,
            "oversized body must be capped, got {} chars",
            out.chars().count()
        );
        assert!(out.contains("Content truncated"));
    }

    #[test]
    fn recovers_lazy_srcset_from_data_attributes() {
        let out = sanitize_article_html(
            r#"<img src="data:image/gif;base64,R0lG" data-srcset="/small.webp 480w, /big.webp 1024w">"#,
            "https://example.com",
        );
        assert!(
            out.contains("srcset"),
            "lazy data-srcset should be recovered"
        );
        assert!(out.contains("/small.webp"));
    }

    #[test]
    fn restores_lazy_images_and_sanitizes_responsive_sources() {
        let content = sanitize_article_html(
            r#"<picture><source srcset="/small.webp 480w, javascript:bad 2x"><img src="data:image/gif;base64,R0lG" data-src="/photo.jpg"></picture>"#,
            "https://example.com/posts/one",
        );
        assert!(content.contains("src=\"https://example.com/photo.jpg\""));
        assert!(content.contains("https://example.com/small.webp 480w"));
        assert!(!content.contains("javascript:"));
        assert!(!content.contains("data:image"));
    }

    #[test]
    fn applies_tracking_policy_to_surviving_content_links() {
        let policy = TrackingUrlPolicy::try_new(true, "campaign_*", "keep.example.com")
            .expect("valid policy");
        let content = sanitize_article_html_with_policy(
            r#"<a href="https://news.example.com/a?id=1&amp;utm_source=rss&amp;campaign_id=2">Clean</a><a href="https://keep.example.com/a?utm_source=rss">Keep</a>"#,
            "https://news.example.com/",
            &policy,
        );
        assert!(
            content.contains("href=\"https://news.example.com/a?id=1\""),
            "{content}"
        );
        assert!(content.contains("href=\"https://keep.example.com/a?utm_source=rss\""));
    }

    #[test]
    fn normalizes_plain_text_without_interpreting_it_as_html() {
        let content = normalize_article_content(
            "First <literal> & safe\nsecond line\n\nNew paragraph",
            "text/plain",
            "https://example.com/article",
        );
        assert!(content.contains("First &lt;literal&gt; &amp; safe<br>second line"));
        assert!(content.contains("</p><p>New paragraph</p>"));
    }

    #[test]
    fn normalizes_markdown_extensions_then_sanitizes_raw_html() {
        let content = normalize_article_content(
            "## Heading\n\n- one\n- two\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n<script>bad()</script>",
            "text/markdown",
            "https://example.com/article",
        );
        assert!(content.contains("<h2>Heading</h2>"));
        assert!(content.contains("<ul>"));
        assert!(content.contains("<table>"));
        assert!(!content.contains("bad()"));
    }

    #[test]
    fn counts_normalized_plain_text_without_markup() {
        assert_eq!(
            article_plain_text_len("<h2>Hello</h2><p>wide&nbsp;world</p>"),
            16
        );
        assert_eq!(article_plain_text_len("<p>   </p>"), 0);
    }

    #[test]
    fn converts_sanitized_html_to_structured_markdown() {
        let markdown = article_html_to_markdown(
            r#"<h2>Guide</h2><p>Use <strong>care</strong> and <a href="/docs">docs</a>.</p><pre><code>let x = 1;</code></pre><script>bad()</script>"#,
            "https://example.com/posts/one",
        );
        assert!(markdown.contains("## Guide"));
        assert!(markdown.contains("**care**"));
        assert!(markdown.contains("[docs](https://example.com/docs)"));
        assert!(markdown.contains("let x = 1;"));
        assert!(!markdown.contains("bad()"));
        assert!(!markdown.contains('<'));
    }

    #[test]
    fn produces_compact_commonmark_for_rich_articles() {
        let markdown = article_html_to_markdown(
            r#"<h2>Practical guide</h2><p>First&nbsp;paragraph.<br>Second line.</p>
               <ul><li>One</li><li>Two<ul><li>Nested</li></ul></li></ul>
               <table><thead><tr><th>Name</th><th>Value</th></tr></thead>
               <tbody><tr><td>Alpha</td><td>Beta</td></tr></tbody></table>
               <blockquote><p>Useful context.</p></blockquote>"#,
            "https://example.com/guide",
        );
        assert!(markdown.contains("## Practical guide"));
        assert!(markdown.contains("First paragraph.\\\nSecond line."));
        assert!(markdown.contains("- One"));
        assert!(markdown.contains("  - Nested"));
        assert!(markdown.contains("| Name"));
        assert!(markdown.contains("> Useful context."));
        assert!(!markdown.contains("\n\n\n"));
        assert!(!markdown.contains('\u{a0}'));
    }

    #[test]
    fn removes_explicit_tracking_pixels_but_keeps_lazy_images() {
        let content = sanitize_article_html(
            r#"<img src="/pixel.gif" width="1" height="1" alt="">
               <img src="data:image/gif;base64,R0lG" data-src="/photo.jpg" width="1" height="1" alt="Photo">"#,
            "https://example.com/article",
        );
        assert!(!content.contains("pixel.gif"));
        assert!(content.contains("https://example.com/photo.jpg"));
    }

    #[test]
    fn reports_hidden_known_and_custom_tracking_images_without_remote_probes() {
        let pixel_policy = TrackingPixelPolicy::try_new(
            crate::tracking_pixel::TrackingPixelStrictness::Balanced,
            "metrics.example.com",
            "images.metrics.example.com",
        )
        .expect("valid policy");
        let report = sanitize_article_html_with_policies(
            r#"<img src="/hidden.gif" style="display: none">
               <img src="https://pixel.wp.com/open.gif">
               <img src="https://metrics.example.com/count">
               <img src="https://images.metrics.example.com/photo.jpg">"#,
            "https://news.example.com/article",
            &TrackingUrlPolicy::default(),
            &pixel_policy,
        );
        assert_eq!(report.blocked_tracking_pixels, 3);
        assert!(!report.html.contains("hidden.gif"));
        assert!(!report.html.contains("pixel.wp.com"));
        assert!(!report.html.contains("metrics.example.com/count"));
        assert!(report.html.contains("images.metrics.example.com/photo.jpg"));
    }

    #[test]
    fn markdown_cleanup_does_not_reflow_fenced_code() {
        let markdown = normalize_markdown(
            "\n\n```text\nfirst  \n\n```not-a-closing-fence\n\nlast\n```\n\n\nAfter\n",
        );
        assert!(markdown.contains("first  \n\n```not-a-closing-fence\n\nlast"));
        assert!(markdown.ends_with("```\n\nAfter"));
    }

    #[test]
    fn preserves_valid_youtube_iframe_identity_as_a_safe_link() {
        let content = sanitize_article_html(
            r#"<figure class="wp-block-embed is-provider-youtube"><iframe title="You Can See Everything | Official Teaser HD | A24" src="https://www.youtube.com/embed/GGJSRFWALTI?feature=oembed" allowfullscreen></iframe></figure>"#,
            "https://techcrunch.com/2026/09/07/example/",
        );
        assert!(!content.contains("<iframe"));
        assert!(content.contains("https://www.youtube.com/watch?v=GGJSRFWALTI"));
        assert!(content.contains("Watch video on YouTube"));
    }

    #[test]
    fn removes_active_and_non_http_content() {
        let content = sanitize_article_html(
            r#"<script>alert(1)</script><iframe src="https://bad.example"></iframe><img src="javascript:bad"><a href="ftp://bad.example">bad</a><a href="https://safe.example">safe</a>"#,
            "https://example.com/article",
        );
        assert!(!content.contains("alert"));
        assert!(!content.contains("iframe"));
        assert!(!content.contains("javascript:"));
        assert!(!content.contains("ftp:"));
        assert!(content.contains("https://safe.example"));
        assert!(content.contains("target=\"_blank\""));
        assert!(content.contains("rel=\"noopener noreferrer\""));
        assert!(content.contains("referrerpolicy=\"no-referrer\""));
    }

    #[test]
    fn replaces_publisher_link_policy_with_the_mandatory_policy() {
        let content = sanitize_article_html(
            r#"<a href="https://safe.example" target="_self" rel="opener" referrerpolicy="origin">safe</a>"#,
            "https://example.com/article",
        );
        assert_eq!(content.matches("target=\"_blank\"").count(), 1);
        assert_eq!(content.matches("rel=\"noopener noreferrer\"").count(), 1);
        assert_eq!(content.matches("referrerpolicy=\"no-referrer\"").count(), 1);
        assert!(!content.contains("opener\""));
        assert!(!content.contains("referrerpolicy=\"origin\""));
    }

    #[test]
    fn personal_link_navigation_never_weakens_privacy() {
        let html = r#"<p><a href="https://example.com" target="_blank">Example</a></p>"#;
        let same_tab = apply_external_link_navigation(html, false);
        assert!(!same_tab.contains("target=\"_blank\""));
        assert!(same_tab.contains("rel=\"noopener noreferrer\""));
        assert!(same_tab.contains("referrerpolicy=\"no-referrer\""));

        let new_tab = apply_external_link_navigation(&same_tab, true);
        assert!(new_tab.contains("target=\"_blank\""));
        assert!(new_tab.contains("rel=\"noopener noreferrer\""));
        assert!(new_tab.contains("referrerpolicy=\"no-referrer\""));
    }
}
