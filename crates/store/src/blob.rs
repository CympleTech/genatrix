//! Blob metadata. The bytes live in the file store (design 08); this table
//! only knows what exists.

use genatrix_model::{Blob, ContentHash};
use rusqlite::{OptionalExtension, Row, params};

use crate::error::{Result, corrupt};
use crate::store::Store;

const T: &str = "blob";

fn row_to_blob(r: &Row<'_>) -> Result<Blob> {
    let hash: String = r.get("hash")?;
    Ok(Blob {
        hash: hash.parse().map_err(|e| corrupt(T, "hash", e))?,
        mime: r.get("mime")?,
        size: r
            .get::<_, i64>("size")?
            .try_into()
            .map_err(|e| corrupt(T, "size", e))?,
        name_hint: r.get("name_hint")?,
    })
}

impl Store {
    /// Record a blob. Content-addressed, so inserting the same hash twice is
    /// a no-op; a later name hint fills in a missing one.
    pub fn upsert_blob(&self, blob: &Blob) -> Result<()> {
        self.conn().execute(
            "INSERT INTO blob (hash, mime, size, name_hint) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(hash) DO UPDATE SET name_hint = coalesce(blob.name_hint, excluded.name_hint)",
            params![
                blob.hash.to_string(),
                blob.mime,
                i64::try_from(blob.size).unwrap_or(i64::MAX),
                blob.name_hint
            ],
        )?;
        Ok(())
    }

    /// The text read out of an attachment by this version of the reader:
    /// `Some(Ok(text))`, `Some(Err(why))` if it could not be read, `None` if
    /// this reader has not tried.
    pub fn blob_text(
        &self,
        hash: &ContentHash,
        reader: &str,
    ) -> Result<Option<std::result::Result<String, String>>> {
        let row: Option<(Option<String>, Option<String>)> = self
            .conn()
            .query_row(
                "SELECT text, error FROM blob_text WHERE hash = ?1 AND reader = ?2",
                params![hash.to_string(), reader],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(text, error)| match (text, error) {
            (Some(t), _) => Ok(t),
            (None, e) => Err(e.unwrap_or_default()),
        }))
    }

    /// Keep what the reader made of an attachment.
    pub fn put_blob_text(
        &self,
        hash: &ContentHash,
        reader: &str,
        read: &std::result::Result<String, String>,
    ) -> Result<()> {
        let (text, error) = match read {
            Ok(t) => (Some(t.as_str()), None),
            Err(e) => (None, Some(e.as_str())),
        };
        self.conn().execute(
            "INSERT INTO blob_text (hash, reader, text, error, read_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (hash) DO UPDATE SET reader = excluded.reader, text = excluded.text,
                 error = excluded.error, read_at = excluded.read_at",
            params![
                hash.to_string(),
                reader,
                text,
                error,
                crate::time::utc_to_col(chrono::Utc::now())
            ],
        )?;
        Ok(())
    }

    /// Fetch blob metadata.
    pub fn get_blob(&self, hash: &ContentHash) -> Result<Option<Blob>> {
        self.conn()
            .query_row(
                "SELECT * FROM blob WHERE hash = ?1",
                [hash.to_string()],
                |r| Ok(row_to_blob(r)),
            )
            .optional()?
            .transpose()
    }

    /// All blobs. Used by export.
    pub fn all_blobs(&self) -> Result<Vec<Blob>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM blob ORDER BY hash")?;
        let rows = stmt.query_map([], |r| Ok(row_to_blob(r)))?;
        rows.map(|r| r?).collect()
    }
}
