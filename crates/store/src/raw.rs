//! Raw records.

use genatrix_model::{Connector, ContentHash, Raw, RawId, Source};
use rusqlite::{OptionalExtension, Row, params};

use crate::error::{Result, corrupt};
use crate::store::Store;
use crate::time::{utc_from_col, utc_to_col};

const T: &str = "raw";

pub(crate) fn connector_from_col(table: &'static str, s: &str) -> Result<Connector> {
    match s {
        "imap" => Ok(Connector::Imap),
        "telegram" => Ok(Connector::Telegram),
        other => Err(corrupt(table, "connector", other)),
    }
}

fn row_to_raw(r: &Row<'_>) -> Result<Raw> {
    let id: String = r.get("id")?;
    let connector: String = r.get("connector")?;
    let fetched_at: String = r.get("fetched_at")?;
    let hash: String = r.get("hash")?;
    Ok(Raw {
        id: id.parse().map_err(|e| corrupt(T, "id", e))?,
        source: Source::new(
            connector_from_col(T, &connector)?,
            r.get::<_, String>("account")?,
            r.get::<_, String>("external_id")?,
        ),
        fetched_at: utc_from_col(T, "fetched_at", &fetched_at)?,
        content_type: r.get("content_type")?,
        hash: hash.parse().map_err(|e| corrupt(T, "hash", e))?,
        size: r
            .get::<_, i64>("size")?
            .try_into()
            .map_err(|e| corrupt(T, "size", e))?,
    })
}

impl Store {
    /// Insert a raw record. Returns `false` when the same source object with
    /// the same content hash is already present, which is how re-fetching is
    /// made idempotent.
    pub fn insert_raw(&self, raw: &Raw) -> Result<bool> {
        let n = self.conn().execute(
            "INSERT OR IGNORE INTO raw
                (id, connector, account, external_id, fetched_at, content_type, hash, size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                raw.id.to_string(),
                raw.source.connector.as_str(),
                raw.source.account,
                raw.source.external_id,
                utc_to_col(raw.fetched_at),
                raw.content_type,
                raw.hash.to_string(),
                i64::try_from(raw.size).unwrap_or(i64::MAX),
            ],
        )?;
        Ok(n == 1)
    }

    /// Fetch a raw record by id.
    pub fn get_raw(&self, id: RawId) -> Result<Option<Raw>> {
        self.conn()
            .query_row("SELECT * FROM raw WHERE id = ?1", [id.to_string()], |r| {
                Ok(row_to_raw(r))
            })
            .optional()?
            .transpose()
    }

    /// Whether a raw record with this source and content already exists.
    pub fn has_raw(&self, source: &Source, hash: &ContentHash) -> Result<bool> {
        let n: i64 = self.conn().query_row(
            "SELECT count(*) FROM raw
             WHERE connector = ?1 AND account = ?2 AND external_id = ?3 AND hash = ?4",
            params![
                source.connector.as_str(),
                source.account,
                source.external_id,
                hash.to_string()
            ],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// All raw records, oldest first. Used by export.
    pub fn all_raw(&self) -> Result<Vec<Raw>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM raw ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok(row_to_raw(r)))?;
        rows.map(|r| r?).collect()
    }
}
