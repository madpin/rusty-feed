//! Shared publisher-provided narration metadata (no audio bytes or user state).

use serde::{Deserialize, Serialize};
use url::Url;

/// Maximum number of narration voices retained per shared article.
pub const MAX_ARTICLE_AUDIO_VOICES: usize = 8;

/// One publisher-provided recording of an article.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArticleAudio {
    pub voice_name: String,
    pub url: String,
    pub mime_type: String,
}

impl ArticleAudio {
    /// Validate inert metadata. Network/DNS safety is enforced by SafeFetchClient
    /// when the reader explicitly requests playback through the media proxy.
    pub fn new(name: &str, source: &str, mime_type: &str, base_url: &str) -> Option<Self> {
        if source.trim().is_empty() || source.len() > 8192 || mime_type.len() > 128 {
            return None;
        }
        let url = Url::parse(base_url).ok()?.join(source.trim()).ok()?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        let mime_type = mime_type.split(';').next()?.trim().to_ascii_lowercase();
        let subtype = mime_type.strip_prefix("audio/")?;
        if subtype.is_empty()
            || !subtype
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&byte))
        {
            return None;
        }
        let voice_name: String = name
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .filter(|character| !character.is_control())
            .take(80)
            .collect();
        Some(Self {
            voice_name: if voice_name.is_empty() {
                "Default voice".to_string()
            } else {
                voice_name
            },
            url: url.into(),
            mime_type,
        })
    }
}
