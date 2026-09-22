//! Persistence on `SQLCipher`: migrations, full-text search, export and import.
//!
//! Design: `docs/design/08-storage.md` and `docs/design/01-data-model.md`.
//!
//! The store is a single encrypted `SQLite` file holding the entities of the
//! data model. It knows nothing about connectors, models, or policy. Callers
//! hand it a 32-byte key; deriving that key from the master key in the
//! keychain is the daemon's job.
//!
//! Item bodies live in the database; raw records and attachments live beside
//! it as encrypted, content-addressed files (see [`FileStore`]).
//!
//! Vector search (`sqlite-vec`) arrives with milestone M2; phase one is
//! full-text only.

#![forbid(unsafe_code)]

mod annotation;
mod blob;
mod cursor;
mod error;
mod export;
mod files;
mod item;
mod migrate;
mod person;
mod raw;
mod store;
mod thread;
mod time;
mod vector;

pub use cursor::StoredCursor;
pub use error::{Error, Result};
pub use export::ExportSummary;
pub use files::FileStore;
pub use genatrix_keys::DbKey;
pub use item::{ItemQuery, ItemVersion};
pub use store::Store;
pub use vector::EMBEDDING_DIMS;

/// Current schema version. Bump with every new migration file.
pub const SCHEMA_VERSION: i64 = migrate::SCHEMA_VERSION;
