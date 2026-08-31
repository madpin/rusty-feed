//! OPML 2.0 import/export primitives.
//!
//! This module only handles the portable document format. Applying an import to
//! a user's subscriptions remains the web/service layer's responsibility so the
//! shared-feed and personal-state boundary stays explicit.

use std::collections::BTreeMap;

use quick_xml::escape::escape;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use serde::{Deserialize, Serialize};

const MAX_OUTLINE_DEPTH: usize = 32;
const MAX_FOLDER_NAME_CHARS: usize = 80;
const MAX_TITLE_CHARS: usize = 200;
const MAX_URL_CHARS: usize = 8_192;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OpmlError {
    #[error("could not parse OPML: {0}")]
    Invalid(String),

    #[error("OPML contains more than {0} subscriptions")]
    TooManySubscriptions(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpmlSubscription {
    pub title: String,
    pub xml_url: String,
    pub html_url: Option<String>,
    pub category_path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpmlDocument {
    pub title: Option<String>,
    pub subscriptions: Vec<OpmlSubscription>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpmlExportEntry {
    pub title: String,
    pub xml_url: String,
    pub html_url: Option<String>,
    pub category_path: Vec<String>,
}

#[derive(Default)]
struct ExportFolder {
    folders: BTreeMap<String, ExportFolder>,
    feeds: Vec<OpmlExportEntry>,
}

#[derive(Default)]
struct OutlineAttributes {
    title: Option<String>,
    xml_url: Option<String>,
    html_url: Option<String>,
}

/// Parse a bounded OPML document into a flat list with category paths.
pub fn parse_opml(bytes: &[u8], max_subscriptions: usize) -> Result<OpmlDocument, OpmlError> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut category_path = Vec::new();
    let mut outline_stack = Vec::new();
    let mut subscriptions = Vec::new();
    let mut document_title = None;
    let mut reading_head_title = false;

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| OpmlError::Invalid(error.to_string()))?;
        match event {
            Event::Start(event) if event.name().as_ref() == b"outline" => {
                let attributes = outline_attributes(&event, &reader)?;
                let pushed = process_outline(
                    attributes,
                    &mut category_path,
                    &mut subscriptions,
                    max_subscriptions,
                )?;
                outline_stack.push(pushed);
                if outline_stack.len() > MAX_OUTLINE_DEPTH {
                    return Err(OpmlError::Invalid(format!(
                        "folder nesting exceeds {MAX_OUTLINE_DEPTH} levels"
                    )));
                }
            }
            Event::Empty(event) if event.name().as_ref() == b"outline" => {
                let attributes = outline_attributes(&event, &reader)?;
                let pushed = process_outline(
                    attributes,
                    &mut category_path,
                    &mut subscriptions,
                    max_subscriptions,
                )?;
                if pushed {
                    category_path.pop();
                }
            }
            Event::End(event) if event.name().as_ref() == b"outline" => {
                let pushed = outline_stack.pop().ok_or_else(|| {
                    OpmlError::Invalid("encountered an unmatched outline end tag".to_string())
                })?;
                if pushed {
                    category_path.pop();
                }
            }
            Event::Start(event) if event.name().as_ref() == b"title" => {
                reading_head_title = document_title.is_none();
            }
            Event::Text(text) if reading_head_title => {
                let value = text
                    .xml10_content()
                    .map_err(|error| OpmlError::Invalid(error.to_string()))?;
                document_title = clean_label(&value, MAX_TITLE_CHARS);
            }
            Event::End(event) if event.name().as_ref() == b"title" => {
                reading_head_title = false;
            }
            Event::DocType(_) => {
                return Err(OpmlError::Invalid(
                    "document type declarations are not allowed".to_string(),
                ));
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    if !outline_stack.is_empty() {
        return Err(OpmlError::Invalid(
            "one or more outline elements were not closed".to_string(),
        ));
    }

    Ok(OpmlDocument {
        title: document_title,
        subscriptions,
    })
}

fn outline_attributes(
    event: &BytesStart<'_>,
    reader: &Reader<&[u8]>,
) -> Result<OutlineAttributes, OpmlError> {
    let mut result = OutlineAttributes::default();
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|error| OpmlError::Invalid(error.to_string()))?;
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|error| OpmlError::Invalid(error.to_string()))?;
        match attribute.key.as_ref() {
            b"text" if result.title.is_none() => result.title = Some(value.into_owned()),
            b"title" if result.title.is_none() => result.title = Some(value.into_owned()),
            b"xmlUrl" | b"xmlurl" if result.xml_url.is_none() => {
                result.xml_url = Some(value.into_owned());
            }
            b"htmlUrl" | b"htmlurl" if result.html_url.is_none() => {
                result.html_url = Some(value.into_owned());
            }
            _ => {}
        }
    }
    Ok(result)
}

fn process_outline(
    attributes: OutlineAttributes,
    category_path: &mut Vec<String>,
    subscriptions: &mut Vec<OpmlSubscription>,
    max_subscriptions: usize,
) -> Result<bool, OpmlError> {
    if let Some(xml_url) = attributes
        .xml_url
        .as_deref()
        .and_then(|value| clean_url(value, MAX_URL_CHARS))
    {
        if subscriptions.len() >= max_subscriptions {
            return Err(OpmlError::TooManySubscriptions(max_subscriptions));
        }
        let title = attributes
            .title
            .as_deref()
            .and_then(|value| clean_label(value, MAX_TITLE_CHARS))
            .unwrap_or_else(|| xml_url.clone());
        subscriptions.push(OpmlSubscription {
            title,
            xml_url,
            html_url: attributes
                .html_url
                .as_deref()
                .and_then(|value| clean_url(value, MAX_URL_CHARS)),
            category_path: category_path.clone(),
        });
        return Ok(false);
    }

    if let Some(folder) = attributes
        .title
        .as_deref()
        .and_then(|value| clean_label(value, MAX_FOLDER_NAME_CHARS))
    {
        category_path.push(folder);
        return Ok(true);
    }
    Ok(false)
}

fn clean_label(value: &str, max_chars: usize) -> Option<String> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!value.is_empty()
        && value.chars().count() <= max_chars
        && !value.chars().any(char::is_control))
    .then_some(value)
}

fn clean_url(value: &str, max_chars: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.chars().count() <= max_chars
        && !value.chars().any(char::is_control))
    .then(|| value.to_string())
}

/// Render a deterministic OPML 2.0 export grouped by personal category path.
pub fn export_opml(title: &str, entries: impl IntoIterator<Item = OpmlExportEntry>) -> String {
    let mut root = ExportFolder::default();
    for entry in entries {
        let mut folder = &mut root;
        for segment in &entry.category_path {
            folder = folder.folders.entry(segment.clone()).or_default();
        }
        folder.feeds.push(entry);
    }
    sort_export_tree(&mut root);

    let mut output = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    output.push_str("<opml version=\"2.0\">\n  <head><title>");
    output.push_str(&escape(title));
    output.push_str("</title></head>\n  <body>\n");
    render_export_folder(&root, 2, &mut output);
    output.push_str("  </body>\n</opml>\n");
    output
}

fn sort_export_tree(folder: &mut ExportFolder) {
    folder.feeds.sort_by(|left, right| {
        left.title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then_with(|| left.xml_url.cmp(&right.xml_url))
    });
    for child in folder.folders.values_mut() {
        sort_export_tree(child);
    }
}

fn render_export_folder(folder: &ExportFolder, depth: usize, output: &mut String) {
    let indent = "  ".repeat(depth);
    for feed in &folder.feeds {
        output.push_str(&indent);
        output.push_str("<outline type=\"rss\" text=\"");
        output.push_str(&escape(&feed.title));
        output.push_str("\" title=\"");
        output.push_str(&escape(&feed.title));
        output.push_str("\" xmlUrl=\"");
        output.push_str(&escape(&feed.xml_url));
        output.push('"');
        if let Some(html_url) = &feed.html_url {
            output.push_str(" htmlUrl=\"");
            output.push_str(&escape(html_url));
            output.push('"');
        }
        output.push_str(" />\n");
    }
    for (name, child) in &folder.folders {
        output.push_str(&indent);
        output.push_str("<outline text=\"");
        output.push_str(&escape(name));
        output.push_str("\" title=\"");
        output.push_str(&escape(name));
        output.push_str("\">\n");
        render_export_folder(child, depth + 1, output);
        output.push_str(&indent);
        output.push_str("</outline>\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_folders_and_multiple_feed_shapes() {
        let document = parse_opml(
            br#"<?xml version="1.0"?>
              <opml version="2.0">
                <head><title>My feeds</title></head>
                <body>
                  <outline text="Technology">
                    <outline title="Rust &amp; Friends" type="rss"
                      xmlUrl="https://example.com/feed.xml"
                      htmlUrl="https://example.com/" />
                  </outline>
                  <outline text="Unfiled" xmlurl="https://other.example/feed" />
                </body>
              </opml>"#,
            100,
        )
        .unwrap();

        assert_eq!(document.title.as_deref(), Some("My feeds"));
        assert_eq!(document.subscriptions.len(), 2);
        assert_eq!(document.subscriptions[0].title, "Rust & Friends");
        assert_eq!(document.subscriptions[0].category_path, vec!["Technology"]);
        assert!(document.subscriptions[1].category_path.is_empty());
    }

    #[test]
    fn rejects_doctypes_and_excessive_subscription_counts() {
        assert!(matches!(
            parse_opml(b"<!DOCTYPE opml><opml><body /></opml>", 10),
            Err(OpmlError::Invalid(_))
        ));
        assert_eq!(
            parse_opml(
                br#"<opml><body><outline xmlUrl="https://one.example"/><outline xmlUrl="https://two.example"/></body></opml>"#,
                1,
            ),
            Err(OpmlError::TooManySubscriptions(1))
        );
    }

    #[test]
    fn exports_a_deterministic_round_trip_with_escaped_values() {
        let output = export_opml(
            "A & B",
            [OpmlExportEntry {
                title: "Rust <News>".to_string(),
                xml_url: "https://example.com/feed?a=1&b=2".to_string(),
                html_url: Some("https://example.com/".to_string()),
                category_path: vec!["Tech & Science".to_string()],
            }],
        );
        assert!(output.contains("<title>A &amp; B</title>"));
        assert!(output.contains("text=\"Tech &amp; Science\""));
        assert!(output.contains("title=\"Rust &lt;News&gt;\""));

        let parsed = parse_opml(output.as_bytes(), 10).unwrap();
        assert_eq!(parsed.subscriptions.len(), 1);
        assert_eq!(parsed.subscriptions[0].title, "Rust <News>");
        assert_eq!(
            parsed.subscriptions[0].category_path,
            vec!["Tech & Science"]
        );
    }
}
