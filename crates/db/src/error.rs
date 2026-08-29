//! Database error type.

/// Errors returned by the persistence layer.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// A query or connection error from SQLx.
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// A migration failed to apply.
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    /// An expected row was not found where one was required.
    #[error("not found")]
    NotFound,

    /// A database-backed operation rejected a client-safe domain constraint.
    #[error("{0}")]
    Validation(String),
}

/// Convenience alias for repository results.
pub type Result<T> = std::result::Result<T, DbError>;
