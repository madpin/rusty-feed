//! Product-vocabulary guard (see `docs/GLOSSARY.md`). No database needed.
//!
//! Scans every Askama template for internal jargon in the text a person can
//! actually read: element text plus `title`, `aria-label`, `placeholder` and
//! `alt` attributes. Template expressions (`{{ }}`, `{% %}`), comments,
//! `<script>`/`<style>` bodies, class names, `href`s, `data-*` attributes, URLs
//! and ALL_CAPS environment-variable names are ignored, so renaming a CSS class
//! or route can never trip it.

use std::fs;
use std::path::{Path, PathBuf};

/// Internal terms that must not appear in visible text, with the plain word to
/// use instead. Matching is case-insensitive and starts at a word boundary, so
/// `jobs` matches `job` but `planet` does not match `lane`.
const DENY: &[(&str, &str)] = &[
    ("lane", "priority"),
    ("outbox", "pending deliveries"),
    ("heartbeat", "check-in"),
    ("acquisition", "feed access"),
    ("agent", "task / background run"),
    ("job", "task"),
    ("dead-letter", "failed"),
    ("dead letter", "failed"),
    ("embedding", "search index"),
    ("vector", "search index"),
    ("pgvector", "search index"),
    ("hnsw", "search index"),
    ("skip locked", "(implementation detail)"),
    ("advisory lock", "(implementation detail)"),
    ("ssrf", "safe fetching / private-network protection"),
    ("onnx", "on-device model"),
    ("singleton", "(implementation detail)"),
    ("cluster", "story"),
    ("digest", "briefing"),
    ("enrich", "process"),
    ("magazine", "Discover"),
    ("glance", "seen"),
    ("worker", "background worker (and explain it)"),
];

/// Pages where a technical term is deliberate: the page explains the term, or
/// it is the public architecture section for self-hosters evaluating the stack.
/// Every entry must still be needed; a stale entry fails the test.
const ALLOW: &[(&str, &str, &str)] = &[
    (
        "admin_logs.html",
        "worker",
        "intro defines 'worker'; the table lists worker processes",
    ),
    (
        "admin_dashboard.html",
        "worker",
        "tooltip defines 'workers'; the tile counts them",
    ),
    (
        "admin_dashboard.html",
        "embedding",
        "storage tooltip explains embeddings as the search index",
    ),
    (
        "admin_dashboard.html",
        "pgvector",
        "storage tooltip: index technology the admin may tune",
    ),
    (
        "admin_dashboard.html",
        "hnsw",
        "storage tooltip: index technology the admin may tune",
    ),
    (
        "admin_dashboard.html",
        "agent",
        "'User agent' is the HTTP header; the label explains it",
    ),
    (
        "admin_costs.html",
        "embedding",
        "intro explains that embedding rows build the search index",
    ),
    ("landing.html", "job", "public architecture section"),
    ("landing.html", "vector", "public architecture section"),
    ("landing.html", "pgvector", "public architecture section"),
    ("landing.html", "embedding", "public architecture section"),
    ("landing.html", "skip locked", "public architecture section"),
    ("landing.html", "ssrf", "public architecture section"),
    ("landing.html", "worker", "public architecture section"),
    ("landing.html", "heartbeat", "public architecture section"),
    ("landing.html", "outbox", "public architecture section"),
];

const VISIBLE_ATTRIBUTES: &[&str] = &["title", "aria-label", "placeholder", "alt"];

fn templates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("templates")
}

/// Remove every `open ... close` span (template expressions and comments).
fn strip_delimited(input: &str, open: &str, close: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find(open) {
        output.push_str(&rest[..start]);
        let body = &rest[start + open.len()..];
        match body.find(close) {
            Some(end) => rest = &body[end + close.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    output.push_str(rest);
    output
}

/// Collect the human-visible attribute values of one start tag (`inner`
/// excludes `<` and `>`).
fn visible_attributes(inner: &str, out: &mut String) {
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'='
            && bytes[i] != b'/'
        {
            i += 1;
        }
        let name = inner[name_start..i].to_ascii_lowercase();
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        let value = if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
            let quote = bytes[i];
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] != quote {
                i += 1;
            }
            let value = &inner[start..i];
            i = (i + 1).min(bytes.len());
            value
        } else {
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            &inner[start..i]
        };
        if VISIBLE_ATTRIBUTES.contains(&name.as_str()) {
            out.push(' ');
            out.push_str(value);
            out.push(' ');
        }
    }
}

/// Blocks whose bodies are CSS class names, not text.
const CLASS_BLOCKS: &[&str] = &["body_class", "layout_class"];

/// The text a reader can see in one template, as one normalized lowercase line.
fn visible_text(source: &str) -> String {
    let mut source = source.to_string();
    for block in CLASS_BLOCKS {
        source = strip_delimited(&source, &format!("{{% block {block} %}}"), "{% endblock %}");
    }
    let source = strip_delimited(&source, "{#", "#}");
    let source = strip_delimited(&source, "{{", "}}");
    let source = strip_delimited(&source, "{%", "%}");
    let source = strip_delimited(&source, "<!--", "-->");
    let mut text = String::new();
    let mut rest = source.as_str();
    while let Some(start) = rest.find('<') {
        text.push_str(&rest[..start]);
        text.push(' ');
        let after = &rest[start + 1..];
        let starts_tag = after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '/');
        if !starts_tag {
            text.push('<');
            rest = after;
            continue;
        }
        let mut quote: Option<char> = None;
        let mut end = after.len();
        for (index, character) in after.char_indices() {
            match (quote, character) {
                (None, '"' | '\'') => quote = Some(character),
                (Some(q), c) if c == q => quote = None,
                (None, '>') => {
                    end = index;
                    break;
                }
                _ => {}
            }
        }
        let inner = &after[..end];
        let is_end_tag = inner.starts_with('/');
        let name = inner
            .trim_start_matches('/')
            .split(|c: char| c.is_ascii_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        rest = &after[(end + 1).min(after.len())..];
        if (name == "script" || name == "style") && !is_end_tag {
            let close = format!("</{name}");
            rest = match rest.to_ascii_lowercase().find(&close) {
                Some(index) => &rest[index..],
                None => "",
            };
            continue;
        }
        if !is_end_tag {
            visible_attributes(inner, &mut text);
        }
    }
    text.push_str(rest);
    normalize(&text)
}

/// Lowercase and drop tokens that are URLs, paths, or `ENV_VAR=value` names.
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .filter(|token| {
            let bare = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
            let env_name = bare.contains('_')
                && bare
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            !(token.contains("://") || token.starts_with('/') || token.contains('=') || env_name)
        })
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Terms from `DENY` found in `text`, matched from a word boundary.
fn denied_terms(text: &str) -> Vec<&'static str> {
    DENY.iter()
        .filter(|(term, _)| {
            text.match_indices(term).any(|(index, _)| {
                text[..index]
                    .chars()
                    .next_back()
                    .is_none_or(|before| !before.is_alphanumeric())
            })
        })
        .map(|(term, _)| *term)
        .collect()
}

fn templates() -> Vec<(String, String)> {
    let mut files: Vec<_> = fs::read_dir(templates_dir())
        .expect("templates directory")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "html"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let source = fs::read_to_string(&path).expect("readable template");
            (name, source)
        })
        .collect()
}

#[test]
fn templates_do_not_show_internal_jargon() {
    let mut violations = Vec::new();
    let mut used = vec![false; ALLOW.len()];
    for (name, source) in templates() {
        for term in denied_terms(&visible_text(&source)) {
            match ALLOW
                .iter()
                .position(|(file, allowed, _)| *file == name && *allowed == term)
            {
                Some(index) => used[index] = true,
                None => {
                    let hint = DENY.iter().find(|(t, _)| *t == term).unwrap().1;
                    violations.push(format!("{name}: \"{term}\" (say: {hint})"));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "internal terms in visible template text; reword them or, for a page that \
         explains the term, add an ALLOW entry:\n{}",
        violations.join("\n")
    );
    let stale: Vec<_> = ALLOW
        .iter()
        .zip(&used)
        .filter(|(_, used)| !**used)
        .map(|((file, term, _), _)| format!("{file}: {term}"))
        .collect();
    assert!(stale.is_empty(), "stale ALLOW entries: {stale:?}");
}

#[test]
fn scanner_reads_visible_text_only() {
    let html = r#"
        {# the job lane #}
        <!-- heartbeat -->
        {% block body_class %}page-digests job{% endblock %}
        <script>var job = "worker";</script>
        <style>.lane { color: red }</style>
        <a class="admin-acquisition-link job-lane" href="/admin/acquisition"
           data-embedding="x" id="digest-nav" name="agent">{{ job.lane }} Overview</a>
        {% if job.lane == "embedding" %}<p>Plain text</p>{% endif %}
        <p>Set EMBEDDINGS_ENABLED=false or visit https://example.com/vector then /admin/outbox.</p>
        <input type="text" value="worker" placeholder="Search" />
    "#;
    assert!(denied_terms(&visible_text(html)).is_empty());

    let leaky = r#"
        <h2>Dead-letter jobs</h2>
        <button title="Last heartbeat from the agent" aria-label='Outbox lane'>Go</button>
        <img alt="embedding map" src="/vector.png" />
        <p>The skip
           locked queue</p>
    "#;
    let found = denied_terms(&visible_text(leaky));
    for term in [
        "dead-letter",
        "job",
        "heartbeat",
        "agent",
        "outbox",
        "lane",
        "embedding",
        "skip locked",
    ] {
        assert!(found.contains(&term), "expected {term} in {found:?}");
    }
    assert!(!found.contains(&"vector"), "src URLs are not visible text");
    assert!(denied_terms(&visible_text("<p>planet release language</p>")).is_empty());
}

#[test]
fn navigation_labels_come_from_the_message_catalog() {
    for (file, class_marker) in [
        ("app_base.html", "topbar-link"),
        ("app_base.html", "account-link"),
        ("admin_nav.html", "admin-"),
    ] {
        let source = fs::read_to_string(templates_dir().join(file)).unwrap();
        let links: Vec<_> = source
            .lines()
            .filter(|line| line.contains("<a ") && line.contains(class_marker))
            .collect();
        assert!(!links.is_empty(), "{file} has no {class_marker} links");
        for line in links {
            assert!(
                line.contains("ui.text("),
                "{file}: hardcoded English in navigation link: {line}"
            );
        }
    }
}

#[test]
fn templates_link_to_briefing_not_the_old_digests_address() {
    for (name, source) in templates() {
        assert!(
            !source.contains("href=\"/digests") && !source.contains("action=\"/digests"),
            "{name} still points at the old /digests address"
        );
    }
}
