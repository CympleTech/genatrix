//! Error type.

/// Ledger errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database rejected the key or is not a ledger.
    #[error("ledger could not be opened with the given key")]
    WrongKey,
    /// The file's schema is newer than this build understands.
    #[error("ledger schema version {found} is newer than supported {supported}")]
    SchemaTooNew {
        /// Version found.
        found: i64,
        /// Version supported.
        supported: i64,
    },
    /// The chain does not verify.
    #[error("ledger chain broken at seq {seq}: {reason}")]
    ChainBroken {
        /// First entry that fails.
        seq: i64,
        /// Why.
        reason: String,
    },
    /// The head file disagrees with the database.
    #[error("ledger head mismatch: {0}")]
    HeadMismatch(String),
    /// A stored value could not be parsed.
    #[error("corrupt ledger value in {column}: {detail}")]
    Corrupt {
        /// Column.
        column: &'static str,
        /// Detail.
        detail: String,
    },
    /// Underlying database error.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// JSON error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// File system error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias.
pub type Result<T> = std::result::Result<T, Error>;
