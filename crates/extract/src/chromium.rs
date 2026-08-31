//! Bounded rendering through an existing remote Chrome DevTools endpoint.
//! Chrome receives only HTML already fetched through `SafeFetchClient`, under
//! a restrictive injected CSP; it never navigates to a publisher URL and
//! therefore cannot bypass the application's SSRF boundary.

use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::Browser;
use futures_util::StreamExt;
use tokio::sync::Semaphore;

#[derive(Debug, Clone)]
pub struct ChromiumConfig {
    pub endpoint: String,
    pub max_concurrency: usize,
    pub render_wait: Duration,
    pub timeout: Duration,
}

#[derive(Clone)]
pub struct ChromiumRenderer {
    config: ChromiumConfig,
    capacity: Arc<Semaphore>,
}

impl ChromiumRenderer {
    pub fn new(config: ChromiumConfig) -> Self {
        let max_concurrency = config.max_concurrency.clamp(1, 8);
        Self {
            config,
            capacity: Arc::new(Semaphore::new(max_concurrency)),
        }
    }

    pub async fn render(&self, html: &str, base_url: &str) -> Result<String, String> {
        let _permit = self
            .capacity
            .acquire()
            .await
            .map_err(|_| "Chromium renderer is shutting down".to_string())?;
        tokio::time::timeout(self.config.timeout, self.render_inner(html, base_url))
            .await
            .map_err(|_| "Chromium rendering timed out".to_string())?
    }

    async fn render_inner(&self, html: &str, base_url: &str) -> Result<String, String> {
        let (browser, mut handler) = Browser::connect(self.config.endpoint.clone())
            .await
            .map_err(|error| format!("Chromium connection failed: {error}"))?;
        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|error| format!("Chromium page creation failed: {error}"))?;
        let document = isolated_document(html, base_url);
        let result = async {
            page.set_content(document)
                .await
                .map_err(|error| format!("Chromium document load failed: {error}"))?;
            tokio::time::sleep(self.config.render_wait).await;
            page.content()
                .await
                .map_err(|error| format!("Chromium DOM capture failed: {error}"))
        }
        .await;
        let _ = page.close().await;
        handler_task.abort();
        result
    }
}

fn isolated_document(html: &str, base_url: &str) -> String {
    let base = html_escape::encode_double_quoted_attribute(base_url);
    let boundary = format!(
        "<base href=\"{base}\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; media-src 'none'; font-src 'none'; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'\">"
    );
    if let Some(head) = html.to_ascii_lowercase().find("<head") {
        if let Some(close) = html[head..].find('>') {
            let offset = head + close + 1;
            let mut output = String::with_capacity(html.len() + boundary.len());
            output.push_str(&html[..offset]);
            output.push_str(&boundary);
            output.push_str(&html[offset..]);
            return output;
        }
    }
    format!("<!doctype html><html><head>{boundary}</head><body>{html}</body></html>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_document_blocks_every_external_channel() {
        let document = isolated_document(
            "<script>document.body.textContent='rendered'</script>",
            "https://publisher.example/story",
        );
        assert!(document.contains("connect-src 'none'"));
        assert!(document.contains("frame-src 'none'"));
        assert!(document.contains("https://publisher.example/story"));
        assert!(!document.contains("unsafe-eval"));
    }
}
