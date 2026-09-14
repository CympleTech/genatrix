//! Error type.

/// Store errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database rejected the key or is not a Genatrix database.
    #[error("database could not be opened with the given key")]
    WrongKey,
    /// The file's schema is newer than this build understands.
    #[error("database schema version {found} is newer than supported {supported}")]
    SchemaTooNew {
        /// Version found in the file.
        found: i64,
        /// Version this build supports.
        supported: i64,
    },
    /// A row referenced something that does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// A stored value could not be parsed back into its type.
    #[error("corrupt value in {table}.{column}: {detail}")]
    Corrupt {
        /// Table.
        table: &'static str,
        /// Column.
        column: &'static str,
        /// What went wrong.
        detail: String,
    },
    /// Underlying database error.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// JSON (de)serialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// File system error during export or import.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias.
pub type Result<T> = std::result::Result<T, Error>;

#[allow(
    clippy::needless_pass_by_value,
    reason = "callers hand over owned error values"
)]
pub(crate) fn corrupt(table: &'static str, column: &'static str, detail: impl ToString) -> Error {
    Error::Corrupt {
        table,
        column,
        detail: detail.to_string(),
    }
}
