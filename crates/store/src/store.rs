//! The store handle.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OpenFlags};

use crate::error::{Error, Result};
use crate::migrate;
use genatrix_keys::DbKey;

/// An open, unlocked Genatrix database.
///
/// Single connection behind a mutex: the design is single-user,
/// single-device, single-writer. Methods take `&self`.
pub struct Store {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Store")
    }
}

impl Store {
    /// Open or create the database at `path`, unlock it with `key`, and
    /// migrate it to the current schema.
    pub fn open(path: impl AsRef<Path>, key: &DbKey) -> Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        genatrix_vec::register();
        let conn = Connection::open_with_flags(path, flags)?;
        Self::init(conn, key)
    }

    /// An in-memory database, for tests and for import dry runs.
    pub fn open_in_memory(key: &DbKey) -> Result<Self> {
        genatrix_vec::register();
        Self::init(Connection::open_in_memory()?, key)
    }

    fn init(mut conn: Connection, key: &DbKey) -> Result<Self> {
        // Already registered before `conn` was opened, by `open` and
        // `open_in_memory`; harmless to say again.
        genatrix_vec::register();
        // The key pragma must be the first statement on the connection.
        conn.execute_batch(&format!("PRAGMA key = {};", key.pragma_literal()))?;
        // Touching the schema is how SQLCipher reports a wrong key.
        if conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| {
                r.get::<_, i64>(0)
            })
            .is_err()
        {
            return Err(Error::WrongKey);
        }
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;
             -- Sixty-four megabytes of pages, against a default of two. The
             -- file is hundreds of megabytes and every page read out of it
             -- is a page decrypted, so a scan that spills the cache pays for
             -- the decryption again on the next question. The interface asks
             -- the same few questions over and over.
             PRAGMA cache_size = -65536;
             -- Sorting and grouping happen in memory rather than in a file
             -- beside the database, which would be plaintext on disk.
             PRAGMA temp_store = MEMORY;",
        )?;
        migrate::run(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Schema version of the open file.
    pub fn schema_version(&self) -> Result<i64> {
        migrate::current_version(&self.conn())
    }

    /// Force the write-ahead log into the main file. Called before snapshots
    /// so a backup never captures a torn state (design 08).
    pub fn checkpoint(&self) -> Result<()> {
        self.conn()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        // A poisoned lock means a panic mid-statement on another thread.
        // SQLite's own transaction semantics keep the file consistent, so
        // continuing is safe.
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Run `f` inside one transaction.
    pub(crate) fn tx<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut guard = self.conn();
        let tx = guard.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(b: u8) -> DbKey {
        DbKey::from_bytes([b; 32])
    }

    #[test]
    fn creates_migrates_and_reopens_with_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        {
            let s = Store::open(&path, &key(1)).unwrap();
            assert_eq!(s.schema_version().unwrap(), crate::SCHEMA_VERSION);
        }
        let s = Store::open(&path, &key(1)).unwrap();
        assert_eq!(s.schema_version().unwrap(), crate::SCHEMA_VERSION);
    }

    #[test]
    fn wrong_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        drop(Store::open(&path, &key(1)).unwrap());
        let err = Store::open(&path, &key(2)).expect_err("must fail");
        assert!(matches!(err, Error::WrongKey), "{err:?}");
    }

    #[test]
    fn file_is_not_plaintext_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        let s = Store::open(&path, &key(1)).unwrap();
        s.checkpoint().unwrap();
        drop(s);
        let head = std::fs::read(&path).unwrap();
        assert!(
            !head.starts_with(b"SQLite format 3"),
            "database is not encrypted"
        );
    }

    #[test]
    fn fts5_trigram_is_available() {
        let s = Store::open_in_memory(&key(1)).unwrap();
        let n: i64 = s
            .conn()
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'item_fts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
}
