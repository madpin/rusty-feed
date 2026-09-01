//! Charset-correct decoding of fetched HTML/text bytes (FND-193,
//! docs/PUBLIC_LAUNCH_PLAN.md §6.1). Shared by the extractor, Live View, and the
//! YouTube-duration scraper so non-UTF-8 pages (Windows-1252, ISO-8859-1,
//! Shift_JIS, GB2312, …) don't corrupt to replacement characters.

/// Decode fetched page bytes to a `String` using, in priority order: the
/// `Content-Type` charset, a byte-order mark, a `<meta charset>` declaration,
/// then UTF-8.
pub fn decode_html_bytes(bytes: &[u8], content_type: Option<&str>) -> String {
    let encoding = content_type
        .and_then(charset_from_content_type)
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
        .or_else(|| sniff_meta_charset(bytes))
        .unwrap_or(encoding_rs::UTF_8);
    // `decode` honors a leading BOM (overriding the guess) and never fails.
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

/// Extract the `charset` token from a `Content-Type` header value.
fn charset_from_content_type(content_type: &str) -> Option<String> {
    content_type.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix("charset=")
            .or_else(|| part.strip_prefix("charset ="))
            .map(|value| value.trim().trim_matches('"').to_string())
            .filter(|value| !value.is_empty())
    })
}

/// Sniff a `<meta charset>` / `<meta http-equiv="content-type">` declaration
/// from the first 2 KiB of the document (the HTML spec's prescan window).
fn sniff_meta_charset(bytes: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let window = &bytes[..bytes.len().min(2048)];
    let head = String::from_utf8_lossy(window).to_ascii_lowercase();
    if let Some(idx) = head.find("charset=") {
        let rest = &head[idx + "charset=".len()..];
        let label: String = rest
            .trim_start_matches(['"', '\'', ' '])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
            .collect();
        if !label.is_empty() {
            return encoding_rs::Encoding::for_label(label.as_bytes());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::decode_html_bytes;

    #[test]
    fn windows_1252_from_content_type() {
        let bytes = [b'A', 0x92, b'B'];
        let decoded = decode_html_bytes(&bytes, Some("text/html; charset=windows-1252"));
        assert_eq!(decoded, "A\u{2019}B");
    }

    #[test]
    fn shift_jis_from_content_type() {
        let bytes = [0x93, 0xFA, 0x96, 0x7B];
        let decoded = decode_html_bytes(&bytes, Some("text/html; charset=Shift_JIS"));
        assert_eq!(decoded, "日本");
    }

    #[test]
    fn meta_charset_is_sniffed_without_content_type() {
        let mut bytes = b"<html><head><meta charset=\"windows-1252\"></head><body>".to_vec();
        bytes.push(0x92);
        bytes.extend_from_slice(b"</body></html>");
        let decoded = decode_html_bytes(&bytes, None);
        assert!(
            decoded.contains('\u{2019}'),
            "meta charset should be honored"
        );
    }

    #[test]
    fn utf8_is_unchanged() {
        let text = "Café — 日本 — naïve";
        let decoded = decode_html_bytes(text.as_bytes(), Some("text/html; charset=utf-8"));
        assert_eq!(decoded, text);
        assert_eq!(decode_html_bytes(text.as_bytes(), None), text);
    }
}
