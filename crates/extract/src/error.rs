//! Extraction errors.

/// Errors from fetching or parsing article content.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("http error: {0}")]
    Http(#[source] reqwest::Error),

    #[error("blocked or invalid URL: {0}")]
    Url(String),

    #[error("response too large")]
    TooLarge,

    #[error("unexpected HTTP status {0}")]
    Status(u16),
}

pub type Result<T> = std::result::Result<T, ExtractError>;
