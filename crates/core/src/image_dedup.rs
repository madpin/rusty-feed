//! Shared, per-article image de-duplication.
//!
//! Some articles repeat one image at several resolutions (WordPress `-300x200`
//! thumbnails, `?w=` CDN variants, `<noscript>` fallbacks, `<picture>` copies).
//! This keeps only the largest variant. It runs once in the shared sanitize
//! step, so the cost is independent of the number of subscribers.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use lol_html::html_content::ContentType;
use lol_html::{element, rewrite_str, RewriteStrSettings};
use regex::Regex;
use url::Url;

const MARKER: &str = "<!--rf-dup-->";
const WRAPPERS: [&str; 5] = ["a", "figure", "p", "div", "span"];
const SIZE_QUERY_KEYS: [&str; 15] = [
    "w", "h", "width", "height", "resize", "fit", "quality", "q", "crop", "size", "ssl", "strip",
    "dpr", "zoom", "auto",
];

static WP_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?)[-_](\d{2,5})x(\d{2,5})(\.[A-Za-z0-9]{2,5})$").unwrap());
static SIZE_SEGMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:(\d{2,5})x\d{2,5}|(?:w|width)[_=-](\d{2,5})|s(\d{2,5})(?:-c)?|(?:[a-z]{1,3}_[^,/]+,?)*w_(\d{2,5})(?:,[a-z]{1,3}_[^,/]+)*)$").unwrap()
});
static WRAPPER_RES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    WRAPPERS
        .iter()
        .map(|tag| {
            let pattern = format!(r"(?is)<{tag}\b[^>]*>((?:\s|<!--rf-dup-->)*)</{tag}>");
            Regex::new(&pattern).unwrap()
        })
        .collect()
});

struct Variant {
    key: String,
    marked: bool,
    width: Option<u32>,
}

fn analyze_url(raw: &str) -> Option<Variant> {
    let url = Url::parse(raw)
        .or_else(|_| Url::parse("http://relative.invalid/").and_then(|base| base.join(raw)))
        .ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let mut marked = false;
    let mut width: Option<u32> = None;
    let mut note = |value: Option<u32>| {
        marked = true;
        width = width.max(value);
    };
    let segments: Vec<&str> = url.path_segments().map(|s| s.collect()).unwrap_or_default();
    let mut path = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        if index + 1 < segments.len() {
            if let Some(caps) = SIZE_SEGMENT.captures(segment) {
                note(
                    (1..=4)
                        .find_map(|i| caps.get(i))
                        .and_then(|m| m.as_str().parse().ok()),
                );
                continue;
            }
            path.push((*segment).to_string());
        } else if let Some(caps) = WP_SUFFIX.captures(segment) {
            note(caps[2].parse().ok());
            path.push(format!("{}{}", &caps[1], &caps[4]));
        } else {
            path.push((*segment).to_string());
        }
    }
    let mut query = Vec::new();
    for (name, value) in url.query_pairs() {
        let lower = name.to_ascii_lowercase();
        if SIZE_QUERY_KEYS.contains(&lower.as_str()) {
            if matches!(lower.as_str(), "w" | "width" | "resize" | "fit" | "size") {
                note(value.split(['x', ',']).next().and_then(|v| v.parse().ok()));
            } else if matches!(lower.as_str(), "h" | "height") {
                note(None);
            }
            continue;
        }
        query.push(format!("{name}={value}"));
    }
    query.sort();
    let key = format!(
        "{}/{}?{}",
        url.host_str().unwrap_or_default().to_ascii_lowercase(),
        path.join("/"),
        query.join("&")
    );
    Some(Variant { key, marked, width })
}

struct ImgInfo {
    src: String,
    key: String,
    rank: u32,
    picture: Option<usize>,
}

fn describe(
    src: &str,
    srcset: Option<&str>,
    width_attr: Option<&str>,
    picture: Option<usize>,
) -> Option<ImgInfo> {
    let primary = analyze_url(src.trim())?;
    let mut size = primary.width.unwrap_or(0);
    size = size.max(
        width_attr
            .and_then(|w| w.trim().parse::<u32>().ok())
            .unwrap_or(0),
    );
    for candidate in srcset.unwrap_or_default().split(',') {
        let mut parts = candidate.split_whitespace();
        let Some(url) = parts.next() else { continue };
        if let Some(w) = parts
            .next()
            .and_then(|d| d.strip_suffix('w'))
            .and_then(|n| n.parse::<u32>().ok())
        {
            size = size.max(w);
        }
        if let Some(width) = analyze_url(url).and_then(|v| v.width) {
            size = size.max(width);
        }
    }
    Some(ImgInfo {
        src: src.trim().to_string(),
        key: primary.key,
        rank: if primary.marked { size } else { u32::MAX },
        picture,
    })
}

/// Drop smaller duplicates of the same image, keeping the largest variant.
/// Returns the input unchanged when nothing is a duplicate.
pub(crate) fn dedupe_images(html: &str) -> String {
    dedupe(html).unwrap_or_else(|| html.to_string())
}

fn dedupe(html: &str) -> Option<String> {
    if html.matches("<img").count() < 2 {
        return None;
    }
    let infos: RefCell<Vec<Option<ImgInfo>>> = RefCell::new(Vec::new());
    let pictures = Cell::new(0_usize);
    rewrite_str(
        html,
        RewriteStrSettings::new()
            .append_element_content_handler(element!("picture", |_| {
                pictures.set(pictures.get() + 1);
                Ok(())
            }))
            .append_element_content_handler(element!("img", |el| {
                let info = el.get_attribute("src").and_then(|src| {
                    describe(
                        &src,
                        el.get_attribute("srcset").as_deref(),
                        el.get_attribute("width").as_deref(),
                        None,
                    )
                });
                infos.borrow_mut().push(info);
                Ok(())
            }))
            .append_element_content_handler(element!("picture img", |_| {
                if let Some(Some(info)) = infos.borrow_mut().last_mut() {
                    info.picture = Some(pictures.get() - 1);
                }
                Ok(())
            })),
    )
    .ok()?;
    let infos = infos.into_inner();
    let mut best: HashMap<&str, u32> = HashMap::new();
    for info in infos.iter().flatten() {
        let entry = best.entry(info.key.as_str()).or_insert(0);
        *entry = (*entry).max(info.rank);
    }
    let mut drop_imgs = HashSet::new();
    let mut drop_pictures = HashSet::new();
    for (index, info) in infos.iter().enumerate() {
        let Some(info) = info else { continue };
        let top = best[info.key.as_str()];
        let beaten = infos
            .iter()
            .flatten()
            .any(|other| other.key == info.key && other.rank == top && other.src != info.src);
        if info.rank < top && beaten {
            drop_imgs.insert(index);
            if let Some(picture) = info.picture {
                drop_pictures.insert(picture);
            }
        }
    }
    if drop_imgs.is_empty() {
        return None;
    }
    let img_index = Cell::new(0_usize);
    let picture_index = Cell::new(0_usize);
    let out = rewrite_str(
        html,
        RewriteStrSettings::new()
            .append_element_content_handler(element!("picture", |el| {
                let index = picture_index.get();
                picture_index.set(index + 1);
                if drop_pictures.contains(&index) {
                    el.replace(MARKER, ContentType::Html);
                }
                Ok(())
            }))
            .append_element_content_handler(element!("img", |el| {
                let index = img_index.get();
                img_index.set(index + 1);
                if drop_imgs.contains(&index) {
                    el.replace(MARKER, ContentType::Html);
                }
                Ok(())
            })),
    )
    .ok()?;
    let mut out = out;
    loop {
        let before = out.clone();
        for re in WRAPPER_RES.iter() {
            out = re
                .replace_all(&out, |caps: &regex::Captures<'_>| {
                    if caps[1].contains(MARKER) {
                        MARKER.to_string()
                    } else {
                        caps[0].to_string()
                    }
                })
                .into_owned();
        }
        if out == before {
            break;
        }
    }
    Some(out.replace(MARKER, ""))
}

#[cfg(test)]
mod tests {
    use super::dedupe_images;

    fn count(html: &str) -> usize {
        html.matches("<img").count()
    }

    #[test]
    fn keeps_largest_wordpress_variant() {
        let html = r#"<p><img src="https://x.com/up/photo-300x200.jpg"></p><p><img src="https://x.com/up/photo-1024x683.jpg"></p><p><img src="https://x.com/up/photo-768x512.jpg"></p>"#;
        let out = dedupe_images(html);
        assert_eq!(count(&out), 1);
        assert!(out.contains("photo-1024x683.jpg"));
        assert!(!out.contains("<p></p>"));
    }

    #[test]
    fn unsuffixed_original_beats_suffixed_variants() {
        let html = r#"<img src="https://x.com/up/photo-300x200.jpg"><img src="https://x.com/up/photo.jpg">"#;
        let out = dedupe_images(html);
        assert_eq!(count(&out), 1);
        assert!(out.contains("up/photo.jpg"));
    }

    #[test]
    fn query_width_variants_collapse() {
        let html = r#"<img src="https://cdn.x.com/a.jpg?w=300"><img src="https://cdn.x.com/a.jpg?w=1200&q=80"><img src="https://cdn.x.com/b.jpg?w=300">"#;
        let out = dedupe_images(html);
        assert_eq!(count(&out), 2);
        assert!(out.contains("w=1200"));
        assert!(out.contains("b.jpg?w=300"));
        assert!(!out.contains("a.jpg?w=300"));
    }

    #[test]
    fn cdn_path_segment_variants_collapse() {
        let html = r#"<img src="https://res.x.com/image/upload/w_300,c_fill/v1/pic.jpg"><img src="https://res.x.com/image/upload/w_1400,c_fill/v1/pic.jpg">"#;
        let out = dedupe_images(html);
        assert_eq!(count(&out), 1);
        assert!(out.contains("w_1400"));
    }

    #[test]
    fn empty_wrappers_are_removed() {
        let html = r#"<figure><a href="https://x.com/p"><img src="https://x.com/u/i-150x150.png"></a></figure><figure><a href="https://x.com/p"><img src="https://x.com/u/i-800x800.png"></a><figcaption>Cap</figcaption></figure>"#;
        let out = dedupe_images(html);
        assert_eq!(count(&out), 1);
        assert_eq!(out.matches("<figure>").count(), 1);
        assert_eq!(out.matches("<a ").count(), 1);
        assert!(out.contains("Cap"));
    }

    #[test]
    fn picture_wrapper_dropped_with_its_image() {
        let html = r#"<picture><source srcset="https://x.com/i-300x200.webp 300w"><img src="https://x.com/i-300x200.jpg"></picture><picture><source srcset="https://x.com/i-900x600.webp 900w"><img src="https://x.com/i-900x600.jpg"></picture>"#;
        let out = dedupe_images(html);
        assert_eq!(count(&out), 1);
        assert_eq!(out.matches("<picture>").count(), 1);
        assert!(out.contains("900x600.jpg"));
        assert!(!out.contains("300x200"));
    }

    #[test]
    fn lazy_placeholder_pair_keeps_larger() {
        let html =
            r#"<img src="https://x.com/l-100x67.jpg" width="100"><img src="https://x.com/l.jpg">"#;
        let out = dedupe_images(html);
        assert_eq!(count(&out), 1);
        assert!(out.contains("l.jpg"));
    }

    #[test]
    fn srcset_width_counts_towards_size() {
        let html = r#"<img src="https://x.com/i-300x200.jpg" srcset="https://x.com/i-300x200.jpg 300w, https://x.com/i-1600x1000.jpg 1600w"><img src="https://x.com/i-768x480.jpg">"#;
        let out = dedupe_images(html);
        assert_eq!(count(&out), 1);
        assert!(out.contains("1600w"));
    }

    #[test]
    fn leaves_distinct_identical_and_equal_size_images_alone() {
        let html = r#"<img src="https://x.com/a-300x200.jpg"><img src="https://x.com/b-300x200.jpg"><img src="https://x.com/c.jpg"><img src="https://x.com/c.jpg"><img src="https://x.com/d.jpg?w=300"><img src="https://x.com/d.jpg?w=300&h=200"><img src="https://y.com/a-900x600.jpg">"#;
        let out = dedupe_images(html);
        assert_eq!(out, html);
    }

    #[test]
    fn single_image_srcset_is_untouched() {
        let html = r#"<img src="https://x.com/i-300x200.jpg" srcset="https://x.com/i-300x200.jpg 300w, https://x.com/i-900x600.jpg 900w">"#;
        assert_eq!(dedupe_images(html), html);
    }
}
