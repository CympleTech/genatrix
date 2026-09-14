//! The ledger handle: open, append, read, verify.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use genatrix_keys::DbKey;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::head::HeadFile;

/// A SHA-256 digest.
pub type Hash = [u8; 32];

const SCHEMA_VERSION: i64 = 1;
const INIT_SQL: &str = include_str!("../migrations/0001_init.sql");
const GENESIS: Hash = [0u8; 32];

/// Well-known entry kinds. The set is closed by design (02, 03); adding one
/// is a design change first.
pub mod kind {
    /// A request left for a model through the egress gate (or was refused).
    pub const EGRESS: &str = "egress";
    /// The outcome of an egress: sent, unknown, failed.
    pub const EGRESS_RESULT: &str = "egress_result";
    /// A run of a task started.
    pub const RUN: &str = "run";
    /// One step of a run.
    pub const RUN_STEP: &str = "run_step";
    /// A run finished.
    pub const RUN_END: &str = "run_end";
    /// An action was proposed.
    pub const ACTION: &str = "action";
    /// A new version of an action's payload.
    pub const ACTION_VERSION: &str = "action_version";
    /// An action was approved, declined, or expired.
    pub const ACTION_DECISION: &str = "action_decision";
    /// An action was executed, or execution failed or is unknown.
    pub const ACTION_EXECUTION: &str = "action_execution";
}

/// One ledger entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Position in the chain, contiguous from 1.
    pub seq: i64,
    /// When it was written.
    pub at: DateTime<Utc>,
    /// What kind of record.
    pub kind: String,
    /// Id of the thing the record is about.
    pub subject: String,
    /// The record, as canonical JSON.
    pub body: serde_json::Value,
    /// Hash of the previous entry.
    pub prev_hash: Hash,
    /// Hash of this entry.
    pub hash: Hash,
}

impl Entry {
    /// Deserialize the body into a record type.
    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_value(self.body.clone())?)
    }
}

/// Which entries to read.
#[derive(Clone, Debug, Default)]
pub struct EntryFilter {
    /// Only this kind.
    pub kind: Option<String>,
    /// Only this subject.
    pub subject: Option<String>,
    /// Only entries with `seq` greater than this.
    pub after_seq: i64,
    /// Page size; 0 means 1000.
    pub limit: u32,
}

/// Outcome of a chain verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verification {
    /// Entries checked.
    pub entries: i64,
    /// Hash of the last entry, or the genesis hash if empty.
    pub head: Hash,
}

fn at_col(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

/// The hash of an entry. Fields are length-separated by a zero byte so no
/// two different entries can hash the same by shifting bytes between fields.
fn hash_entry(seq: i64, prev: &Hash, at: &str, kind: &str, subject: &str, body: &str) -> Hash {
    let mut h = Sha256::new();
    h.update(seq.to_be_bytes());
    h.update(prev);
    for part in [at, kind, subject, body] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    h.finalize().into()
}

fn blob32(column: &'static str, v: Vec<u8>) -> Result<Hash> {
    v.try_into().map_err(|v: Vec<u8>| Error::Corrupt {
        column,
        detail: format!("expected 32 bytes, got {}", v.len()),
    })
}

fn row_to_entry(r: &Row<'_>) -> Result<Entry> {
    let at: String = r.get("at")?;
    let body: String = r.get("body")?;
    Ok(Entry {
        seq: r.get("seq")?,
        at: DateTime::parse_from_rfc3339(&at)
            .map(|t| t.with_timezone(&Utc))
            .map_err(|e| Error::Corrupt {
                column: "at",
                detail: e.to_string(),
            })?,
        kind: r.get("kind")?,
        subject: r.get("subject")?,
        body: serde_json::from_str(&body)?,
        prev_hash: blob32("prev_hash", r.get("prev_hash")?)?,
        hash: blob32("hash", r.get("hash")?)?,
    })
}

/// An open ledger.
pub struct Ledger {
    conn: Mutex<Connection>,
    head: Option<HeadFile>,
}

impl std::fmt::Debug for Ledger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ledger")
    }
}

impl Ledger {
    /// Open or create the ledger at `path`. A head file is kept beside it.
    pub fn open(path: impl AsRef<Path>, key: &DbKey) -> Result<Self> {
        let path = path.as_ref();
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        let mut ledger = Self::init(conn, key, Some(HeadFile::beside(path)))?;
        ledger.reconcile_head()?;
        Ok(ledger)
    }

    /// An in-memory ledger with no head file, for tests.
    pub fn open_in_memory(key: &DbKey) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, key, None)
    }

    fn init(mut conn: Connection, key: &DbKey, head: Option<HeadFile>) -> Result<Self> {
        conn.execute_batch(&format!("PRAGMA key = {};", key.pragma_literal()))?;
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
             PRAGMA synchronous = FULL;",
        )?;
        let found: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if found > SCHEMA_VERSION {
            return Err(Error::SchemaTooNew {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        if found < 1 {
            let tx = conn.transaction()?;
            tx.execute_batch(INIT_SQL)?;
            tx.pragma_update(None, "user_version", 1)?;
            tx.commit()?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
            head,
        })
    }

    /// On open: if there is no head file yet, write one from the database.
    /// If there is, it must agree with the database, otherwise the ledger
    /// has been rewritten or rolled back behind our back.
    fn reconcile_head(&mut self) -> Result<()> {
        let Some(head) = &self.head else {
            return Ok(());
        };
        let last = self.last()?;
        let (db_seq, db_hash) = last.map_or((0, GENESIS), |e| (e.seq, e.hash));
        match head.read()? {
            None => head.write(db_seq, &db_hash),
            Some((seq, hash)) if seq == db_seq && hash == db_hash => Ok(()),
            Some((seq, _)) => Err(Error::HeadMismatch(format!(
                "head file says seq {seq}, database says seq {db_seq}"
            ))),
        }
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Append one entry. Serial: the mutex is the write queue, so `seq` is
    /// contiguous and the chain never forks.
    pub fn append<T: Serialize + ?Sized>(
        &self,
        kind: &str,
        subject: &str,
        body: &T,
    ) -> Result<Entry> {
        // serde_json's default map is ordered, so this is canonical.
        let body_value = serde_json::to_value(body)?;
        let body_text = serde_json::to_string(&body_value)?;
        let at = Utc::now();
        let at_text = at_col(at);

        let mut guard = self.conn();
        let tx = guard.transaction()?;
        let prev: Option<(i64, Vec<u8>)> = tx
            .query_row(
                "SELECT seq, hash FROM entry ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (seq, prev_hash) = match prev {
            Some((s, h)) => (s + 1, blob32("hash", h)?),
            None => (1, GENESIS),
        };
        let hash = hash_entry(seq, &prev_hash, &at_text, kind, subject, &body_text);
        tx.execute(
            "INSERT INTO entry (seq, at, kind, subject, body, prev_hash, hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                seq,
                at_text,
                kind,
                subject,
                body_text,
                prev_hash.as_slice(),
                hash.as_slice()
            ],
        )?;
        tx.commit()?;
        if let Some(head) = &self.head {
            head.write(seq, &hash)?;
        }
        drop(guard);
        Ok(Entry {
            seq,
            at,
            kind: kind.to_owned(),
            subject: subject.to_owned(),
            body: body_value,
            prev_hash,
            hash,
        })
    }

    /// The most recent entry.
    pub fn last(&self) -> Result<Option<Entry>> {
        self.conn()
            .query_row("SELECT * FROM entry ORDER BY seq DESC LIMIT 1", [], |r| {
                Ok(row_to_entry(r))
            })
            .optional()?
            .transpose()
    }

    /// One entry by sequence number.
    pub fn get(&self, seq: i64) -> Result<Option<Entry>> {
        self.conn()
            .query_row("SELECT * FROM entry WHERE seq = ?1", [seq], |r| {
                Ok(row_to_entry(r))
            })
            .optional()?
            .transpose()
    }

    /// Read entries in sequence order.
    pub fn entries(&self, f: &EntryFilter) -> Result<Vec<Entry>> {
        let limit = if f.limit == 0 { 1000 } else { f.limit };
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT * FROM entry
             WHERE seq > ?1
               AND (?2 IS NULL OR kind = ?2)
               AND (?3 IS NULL OR subject = ?3)
             ORDER BY seq LIMIT ?4",
        )?;
        let rows = stmt.query_map(params![f.after_seq, f.kind, f.subject, limit], |r| {
            Ok(row_to_entry(r))
        })?;
        rows.map(|r| r?).collect()
    }

    /// Number of entries.
    pub fn len(&self) -> Result<i64> {
        Ok(self
            .conn()
            .query_row("SELECT count(*) FROM entry", [], |r| r.get(0))?)
    }

    /// Whether the ledger has no entries.
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Walk the whole chain from the root, recomputing every hash and
    /// checking every link, then compare the end with the head file.
    pub fn verify(&self) -> Result<Verification> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT seq, at, kind, subject, body, prev_hash, hash FROM entry ORDER BY seq",
        )?;
        let mut rows = stmt.query([])?;
        let mut expected_seq = 1i64;
        let mut prev = GENESIS;
        while let Some(r) = rows.next()? {
            let seq: i64 = r.get(0)?;
            let at: String = r.get(1)?;
            let kind: String = r.get(2)?;
            let subject: String = r.get(3)?;
            let body: String = r.get(4)?;
            let prev_hash = blob32("prev_hash", r.get(5)?)?;
            let hash = blob32("hash", r.get(6)?)?;
            if seq != expected_seq {
                return Err(Error::ChainBroken {
                    seq,
                    reason: format!("expected seq {expected_seq}, gap or duplicate"),
                });
            }
            if prev_hash != prev {
                return Err(Error::ChainBroken {
                    seq,
                    reason: "prev_hash does not match the previous entry".into(),
                });
            }
            let recomputed = hash_entry(seq, &prev_hash, &at, &kind, &subject, &body);
            if recomputed != hash {
                return Err(Error::ChainBroken {
                    seq,
                    reason: "stored hash does not match the entry's content".into(),
                });
            }
            prev = hash;
            expected_seq += 1;
        }
        drop(rows);
        drop(stmt);
        drop(conn);
        let entries = expected_seq - 1;
        if let Some(head) = &self.head
            && let Some((seq, hash)) = head.read()?
            && (seq != entries || hash != prev)
        {
            return Err(Error::HeadMismatch(format!(
                "head file says seq {seq}, chain ends at seq {entries}"
            )));
        }
        Ok(Verification {
            entries,
            head: prev,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_separates_fields() {
        let a = hash_entry(1, &GENESIS, "t", "ab", "c", "{}");
        let b = hash_entry(1, &GENESIS, "t", "a", "bc", "{}");
        assert_ne!(a, b);
    }
}
