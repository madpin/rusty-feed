//! Discover narration before Readability and sanitization remove player markup.

use rusty_feed_core::article_audio::{ArticleAudio, MAX_ARTICLE_AUDIO_VOICES};
use scraper::{ElementRef, Html, Selector};

/// Remove the original narration players so they cannot duplicate the reader's
/// controls or retain a publisher-controlled preload outside the media proxy.
pub(crate) fn without_narration_players(html: &str, doc: &Html) -> String {
    let Ok(players) = Selector::parse("audio") else {
        return html.to_string();
    };
    let remove: std::collections::HashSet<_> = doc
        .select(&players)
        .enumerate()
        .filter_map(|(index, player)| is_narration(player).then_some(index))
        .collect();
    let mut index = 0;
    lol_html::rewrite_str(
        html,
        lol_html::RewriteStrSettings::new()
            .append_element_content_handler(lol_html::element!("audio", |element| {
                if remove.contains(&index) {
                    element.remove();
                }
                index += 1;
                Ok(())
            }))
            .append_element_content_handler(lol_html::element!("[data-tts-audios]", |element| {
                element.remove();
                Ok(())
            })),
    )
    .unwrap_or_else(|_| html.to_string())
}

fn is_narration(player: ElementRef<'_>) -> bool {
    std::iter::once(player)
        .chain(player.ancestors().filter_map(ElementRef::wrap).take(3))
        .any(|element| {
            element.attr("data-tts-audios").is_some()
                || [
                    element.attr("class"),
                    element.attr("title"),
                    element.attr("aria-label"),
                ]
                .into_iter()
                .flatten()
                .any(tts_hint)
        })
}

/// Detect bounded, validated publisher narration metadata without fetching audio.
pub fn detect_article_audio(doc: &Html, base_url: &str) -> Vec<ArticleAudio> {
    let mut voices = Vec::new();
    let Ok(wrappers) = Selector::parse("[data-tts-audios]") else {
        return voices;
    };
    for wrapper in doc.select(&wrappers).take(64) {
        let Some(json) = wrapper
            .attr("data-tts-audios")
            .filter(|json| json.len() <= 65_536)
        else {
            continue;
        };
        let Ok(serde_json::Value::Array(entries)) = serde_json::from_str(json) else {
            continue;
        };
        for entry in entries.iter().take(64) {
            let voice = (|| {
                ArticleAudio::new(
                    entry
                        .get("voice_name")
                        .and_then(|name| name.as_str())
                        .unwrap_or_default(),
                    entry.get("voice_source")?.as_str()?,
                    entry.get("mimetype")?.as_str()?,
                    base_url,
                )
            })();
            push_voice(&mut voices, voice);
        }
    }
    let (Ok(players), Ok(sources)) = (Selector::parse("audio"), Selector::parse("source")) else {
        return voices;
    };
    for player in doc.select(&players).take(64) {
        if !is_narration(player) {
            continue;
        }
        let name = player
            .attr("title")
            .or(player.attr("aria-label"))
            .unwrap_or("Default voice");
        let voice = std::iter::once(player)
            .chain(player.select(&sources).take(64))
            .find_map(|source| {
                let url = source.attr("src")?;
                let mime = source.attr("type").or_else(|| inferred_mime(url))?;
                ArticleAudio::new(name, url, mime, base_url)
            });
        push_voice(&mut voices, voice);
    }
    voices
}

fn push_voice(voices: &mut Vec<ArticleAudio>, voice: Option<ArticleAudio>) {
    if let Some(voice) = voice {
        if voices.len() < MAX_ARTICLE_AUDIO_VOICES
            && !voices.iter().any(|known| known.url == voice.url)
        {
            voices.push(voice);
        }
    }
}

fn tts_hint(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "tts",
        "text-to-speech",
        "text_to_speech",
        "narration",
        "listen to",
        "read aloud",
    ]
    .iter()
    .any(|hint| value.contains(hint))
}

fn inferred_mime(url: &str) -> Option<&'static str> {
    let path = url.split(['?', '#']).next()?.to_ascii_lowercase();
    match path.rsplit('.').next()? {
        "mp3" => Some("audio/mpeg"),
        "m4a" => Some("audio/mp4"),
        "ogg" | "opus" => Some("audio/ogg"),
        "wav" => Some("audio/wav"),
        "webm" => Some("audio/webm"),
        "aac" => Some("audio/aac"),
        "flac" => Some("audio/flac"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://blog.google/innovation-and-ai/models-and-research/google-deepmind/genie-4-now-available-project-genie/";

    #[test]
    fn google_voices_survive_before_readability() {
        let doc = Html::parse_document(include_str!("../tests/fixtures/google_tts.html"));
        let voices = detect_article_audio(&doc, BASE);
        assert_eq!(voices.len(), 2);
        assert_eq!(voices[0].voice_name, "Umbriel");
        assert_eq!(voices[1].voice_name, "Gacrux");
        assert_eq!(voices[0].mime_type, "audio/mpeg");
        assert!(voices[0].url.starts_with("https://storage.googleapis.com/"));
    }

    #[test]
    fn generic_players_require_a_narration_hint_and_resolve_relative_urls() {
        let doc = Html::parse_document(
            r#"<audio src="/music.mp3"></audio>
            <div class="audio-player-tts"><audio title="Read aloud: Alice">
            <source src="javascript:evil" type="audio/mpeg">
            <source src="/alice.mp3" type="audio/mpeg"><source src="/alice.ogg" type="audio/ogg">
            </audio></div><audio title="Listen to this article" src="/bob.mp3"></audio>"#,
        );
        let voices = detect_article_audio(&doc, BASE);
        assert_eq!(voices.len(), 2);
        assert_eq!(voices[0].url, "https://blog.google/alice.mp3");
        assert_eq!(voices[1].url, "https://blog.google/bob.mp3");
        let stripped = without_narration_players(&doc.html(), &doc);
        assert!(stripped.contains("/music.mp3"));
        assert!(!stripped.contains("/alice.mp3"));
        assert!(!stripped.contains("/bob.mp3"));
    }

    #[test]
    fn malformed_voice_list_still_uses_its_plain_player() {
        let doc = Html::parse_document(
            r#"<div data-tts-audios='broken'><audio>
            <source src="/fallback.mp3" type="audio/mpeg"></audio></div>"#,
        );
        let voices = detect_article_audio(&doc, BASE);
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].url, "https://blog.google/fallback.mp3");
        assert!(!without_narration_players(&doc.html(), &doc).contains("<audio"));
    }

    #[test]
    fn rejects_unsafe_metadata_and_recovers_from_malformed_json() {
        let doc = Html::parse_document(
            r#"<div data-tts-audios='broken'></div>
            <div data-tts-audios='[null,{},
            {"voice_source":"data:audio/mpeg,abc","mimetype":"audio/mpeg"},
            {"voice_source":"ftp://example.com/a","mimetype":"audio/mpeg"},
            {"voice_source":"https://user:pass@example.com/a","mimetype":"audio/mpeg"},
            {"voice_source":"/a","mimetype":"text/html"},
            {"voice_source":"/b","mimetype":"audio/"},
            {"voice_source":"/c","mimetype":"audio/*"},
            {"voice_source":"//cdn.example.com/a","mimetype":"Audio/MPEG; charset=binary"}]'></div>"#,
        );
        let voices = detect_article_audio(&doc, BASE);
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].url, "https://cdn.example.com/a");
        assert_eq!(voices[0].mime_type, "audio/mpeg");
    }

    #[test]
    fn caps_voices_and_metadata_and_deduplicates_urls() {
        let entries: Vec<_> = (0..20).map(|i| serde_json::json!({
            "voice_name": "x".repeat(200), "voice_source": format!("/voice{i}.mp3"), "mimetype": "audio/mpeg"
        })).collect();
        let doc = Html::parse_document(&format!(
            "<div data-tts-audios='{}'></div>",
            serde_json::to_string(&entries).unwrap()
        ));
        let voices = detect_article_audio(&doc, BASE);
        assert_eq!(voices.len(), MAX_ARTICLE_AUDIO_VOICES);
        assert_eq!(voices[0].voice_name.len(), 80);
        let doc = Html::parse_document(
            r#"<audio class="tts" src="/a.mp3"></audio><audio class="tts" src="/a.mp3"></audio>"#,
        );
        assert_eq!(detect_article_audio(&doc, BASE).len(), 1);
        let doc = Html::parse_document(&format!(
            "<div data-tts-audios='{}'></div>",
            " ".repeat(65_537)
        ));
        assert!(detect_article_audio(&doc, BASE).is_empty());
    }
}
