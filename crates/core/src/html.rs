use std::collections::HashSet;

use ammonia::{Builder, Url, UrlRelative};

pub fn sanitize_article_html(html: &str, base_url: &str) -> String {
    let relative = Url::parse(base_url)
        .map(UrlRelative::RewriteWithBase)
        .unwrap_or(UrlRelative::Deny);
    Builder::default()
        .url_schemes(
            ["http", "https", "mailto"]
                .into_iter()
                .collect::<HashSet<_>>(),
        )
        .add_tags(&["audio", "picture", "source", "video"])
        .add_tag_attributes("audio", &["controls", "preload"])
        .add_tag_attributes("source", &["media", "src", "type"])
        .add_tag_attributes(
            "video",
            &["controls", "height", "poster", "preload", "width"],
        )
        .url_relative(relative)
        .clean(html)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_safe_rich_content_and_absolutizes_relative_urls() {
        let html = r#"<figure><img src="/cover.png" alt="Cover"><video controls poster="poster.jpg"><source src="movie.mp4" type="video/mp4"></video><table><tr><td>Cell</td></tr></table></figure>"#;
        let content = sanitize_article_html(html, "https://example.com/articles/one");
        assert!(content.contains("https://example.com/cover.png"));
        assert!(content.contains("https://example.com/articles/poster.jpg"));
        assert!(content.contains("https://example.com/articles/movie.mp4"));
        assert!(content.contains("<video controls"));
        assert!(content.contains("<table>"));
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
    }
}
