use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::ACCEPT;
use rustss_db::{feed_icons, Db};
use uuid::Uuid;

use crate::SafeFetchClient;

const MAX_ICON_BYTES: usize = 512 * 1024;

pub fn resolve_icon_url(value: &str, feed_url: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let mut url = url::Url::parse(feed_url).ok()?.join(value).ok()?;
    url.set_fragment(None);
    Some(url.to_string())
}

pub fn favicon_url(site_url: Option<&str>, feed_url: &str) -> Option<String> {
    let base = site_url.unwrap_or(feed_url);
    url::Url::parse(base)
        .ok()?
        .join("/favicon.ico")
        .ok()
        .map(|url| url.to_string())
}

pub async fn try_refresh_feed_icon(
    db: &Db,
    client: &SafeFetchClient,
    feed_id: Uuid,
    source_url: &str,
) -> rustss_db::Result<()> {
    if !feed_icons::should_attempt(db.pool(), feed_id, source_url).await? {
        return Ok(());
    }
    match tokio::time::timeout(Duration::from_secs(5), download_icon(client, source_url)).await {
        Ok(Ok((content_type, content))) => {
            feed_icons::record_success(db.pool(), feed_id, source_url, content_type, &content).await
        }
        Ok(Err(_)) => {
            feed_icons::record_failure(db.pool(), feed_id, source_url, "icon download failed").await
        }
        Err(_) => {
            feed_icons::record_failure(db.pool(), feed_id, source_url, "icon download timed out")
                .await
        }
    }
}

async fn download_icon(
    client: &SafeFetchClient,
    source_url: &str,
) -> std::result::Result<(&'static str, Vec<u8>), String> {
    client
        .validate_url(source_url)
        .map_err(|error| error.to_string())?;
    let response = client
        .client()
        .get(source_url)
        .header(
            ACCEPT,
            "image/png,image/jpeg,image/gif,image/webp,image/x-icon,image/vnd.microsoft.icon",
        )
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ICON_BYTES as u64)
    {
        return Err("icon response exceeded the size limit".to_string());
    }
    let content = read_icon(response).await?;
    let content_type =
        image_type(&content).ok_or_else(|| "unsupported image format".to_string())?;
    Ok((content_type, content))
}

async fn read_icon(response: reqwest::Response) -> std::result::Result<Vec<u8>, String> {
    let mut stream = response.bytes_stream();
    let mut content = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        if content.len() + chunk.len() > MAX_ICON_BYTES {
            return Err("icon response exceeded the size limit".to_string());
        }
        content.extend_from_slice(&chunk);
    }
    Ok(content)
}

fn image_type(content: &[u8]) -> Option<&'static str> {
    if content.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if content.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if content.starts_with(b"GIF87a") || content.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if content.len() >= 12 && &content[..4] == b"RIFF" && &content[8..12] == b"WEBP" {
        Some("image/webp")
    } else if content.starts_with(&[0, 0, 1, 0]) {
        Some("image/x-icon")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_declared_and_fallback_icon_urls() {
        assert_eq!(
            resolve_icon_url("images/icon.png", "https://example.com/feed.xml"),
            Some("https://example.com/images/icon.png".to_string())
        );
        assert_eq!(
            favicon_url(
                Some("https://example.com/news"),
                "https://feed.example.com/rss"
            ),
            Some("https://example.com/favicon.ico".to_string())
        );
    }

    #[test]
    fn accepts_only_raster_icon_signatures() {
        assert_eq!(image_type(b"\x89PNG\r\n\x1a\nbody"), Some("image/png"));
        assert_eq!(image_type(b"GIF89abody"), Some("image/gif"));
        assert_eq!(image_type(b"<svg></svg>"), None);
        assert_eq!(image_type(b"<html></html>"), None);
    }
}
