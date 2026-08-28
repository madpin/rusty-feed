//! Feed subsystem errors.

/// Errors from fetching, parsing, or ingesting feeds.
#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    #[error("http error: {0}")]
    Http(#[source] reqwest::Error),

    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    #[error("unsupported URL scheme: {0}")]
    UnsupportedScheme(String),

    #[error("feed response exceeded the size limit")]
    TooLarge,

    #[error("unexpected HTTP status {0}")]
    Status(u16),

    #[error("could not parse feed: {0}")]
    Parse(String),

    #[error(transparent)]
    Db(#[from] rustss_db::DbError),
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, FeedError>;
