//! Removing everything one connector brought in, so it can be fetched again
//! under today's rules. The user asks for this by name; nothing calls it on
//! its own (design 01: nothing of the user's is deleted without their word).
//!
//! What goes: the connector's items (every version), their annotations,
//! chunks and vectors, their raw records, its threads, the attachments only
//! its items used, promises that rest only on its items, and its sync
//! cursors except the ones named to keep (a session, so no new sign-in is
//! needed). What stays: people and their handles, so the same people come
//! back as the same people with the roles and notes the user gave them;
//! digests and actions, which keep their words and lose a source link.

use std::collections::HashSet;

use genatrix_model::{Connector, ContentHash};
use rusqlite::params;

use crate::error::Result;
use crate::store::Store;

/// What a purge removed, or would remove.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Purged {
    /// Items, every version.
    pub items: usize,
    /// Annotations on them.
    pub annotations: usize,
    /// Search chunks of them.
    pub chunks: usize,
    /// Raw records.
    pub raws: usize,
    /// Threads.
    pub threads: usize,
    /// Promises whose evidence was only these items.
    pub commitments: usize,
    /// Attachment rows no other item uses.
    pub blobs: usize,
    /// Sync cursors.
    pub cursors: usize,
    /// Of the annotations, the ones the user made: levels they set.
    pub yours: usize,
    /// Of the promises going, the ones the user confirmed or rejected.
    pub judged_promises: usize,
    /// Raw record files to delete from disk once the transaction is in.
    pub raw_files: Vec<ContentHash>,
    /// Attachment files to delete from disk.
    pub blob_files: Vec<ContentHash>,
}

impl Store {
    /// Remove what `connector` brought in. With `dry_run`, count only, and
    /// quickly: nothing is deleted and nothing is locked for writing.
    pub fn purge_connector(
        &self,
        connector: Connector,
        keep_cursors: &[&str],
        dry_run: bool,
    ) -> Result<Purged> {
        if dry_run {
            self.purge_plan(connector, keep_cursors)
        } else {
            self.purge_now(connector, keep_cursors)
        }
    }

    fn purge_plan(&self, connector: Connector, keep_cursors: &[&str]) -> Result<Purged> {
        let c = connector.as_str();
        let conn = self.conn();
        let count = |sql: &str| -> Result<usize> {
            let n: i64 = conn.query_row(sql, [c], |r| r.get(0))?;
            Ok(usize::try_from(n).unwrap_or(0))
        };
        let mut out = Purged {
            items: count("SELECT count(*) FROM item WHERE connector = ?1")?,
            annotations: count(
                "SELECT count(*) FROM annotation WHERE item_id IN (SELECT id FROM item WHERE connector = ?1)",
            )?,
            chunks: count(
                "SELECT count(*) FROM chunk WHERE item_id IN (SELECT id FROM item WHERE connector = ?1)",
            )?,
            raws: count("SELECT count(*) FROM raw WHERE connector = ?1")?,
            threads: count("SELECT count(*) FROM thread WHERE connector = ?1")?,
            yours: count(
                "SELECT count(*) FROM annotation WHERE producer LIKE '%\"by\":\"user\"%'
                   AND item_id IN (SELECT id FROM item WHERE connector = ?1)",
            )?,
            ..Purged::default()
        };
        let ids = item_ids(&conn, c)?;
        let going: Vec<String> = promises(&conn)?
            .into_iter()
            .filter(|(_, list)| !list.is_empty() && list.iter().all(|e| ids.contains(e)))
            .map(|(id, _)| id)
            .collect();
        out.commitments = going.len();
        out.judged_promises = {
            let mut stmt = conn.prepare("SELECT standing FROM commitment WHERE id = ?1")?;
            going
                .iter()
                .filter(|id| {
                    stmt.query_row([id], |r| r.get::<_, String>(0))
                        .is_ok_and(|s| s != "inferred")
                })
                .count()
        };
        out.blobs = only_ours(&conn, c)?.len();
        out.cursors = cursors_scopes(&conn, c)?
            .iter()
            .filter(|s| !keep_cursors.contains(&s.as_str()))
            .count();
        Ok(out)
    }

    fn purge_now(&self, connector: Connector, keep_cursors: &[&str]) -> Result<Purged> {
        let c = connector.as_str();
        let mut guard = self.conn();
        let tx = guard.transaction()?;
        let ids = item_ids(&tx, c)?;
        let mut out = remove_items(&tx, &ids)?;
        // Raw records the connector fetched that never became items.
        let orphans: Vec<String> = {
            let mut stmt = tx.prepare("SELECT DISTINCT hash FROM raw WHERE connector = ?1")?;
            let rows = stmt.query_map([c], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        out.raws += tx.execute("DELETE FROM raw WHERE connector = ?1", [c])?;
        for hash in orphans {
            let still: i64 =
                tx.query_row("SELECT count(*) FROM raw WHERE hash = ?1", [&hash], |r| {
                    r.get(0)
                })?;
            if still == 0
                && let Ok(h) = hash.parse::<ContentHash>()
                && !out.raw_files.contains(&h)
            {
                out.raw_files.push(h);
            }
        }
        out.threads = tx.execute("DELETE FROM thread WHERE connector = ?1", [c])?;
        for scope in cursors_scopes(&tx, c)? {
            if !keep_cursors.contains(&scope.as_str()) {
                out.cursors += tx.execute(
                    "DELETE FROM sync_cursor WHERE connector = ?1 AND scope = ?2",
                    params![c, scope],
                )?;
            }
        }
        tx.commit()?;
        Ok(out)
    }

    /// Remove these items, every version of each, with everything that
    /// hangs on them (design 01, "垃圾"). Raw and attachment files to delete
    /// from disk come back in the result.
    pub fn remove_items(&self, ids: &HashSet<String>) -> Result<Purged> {
        let mut guard = self.conn();
        let tx = guard.transaction()?;
        let out = remove_items(&tx, ids)?;
        tx.commit()?;
        Ok(out)
    }

    /// How many items one connector brought in, all versions.
    pub fn count_items_from(&self, connector: Connector) -> Result<u64> {
        let n: i64 = self.conn().query_row(
            "SELECT count(*) FROM item WHERE connector = ?1",
            [connector.as_str()],
            |r| r.get(0),
        )?;
        Ok(u64::try_from(n).unwrap_or(0))
    }
}

fn item_ids(conn: &rusqlite::Connection, c: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT id FROM item WHERE connector = ?1")?;
    let rows = stmt.query_map([c], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

fn promises(conn: &rusqlite::Connection) -> Result<Vec<(String, Vec<String>)>> {
    let mut stmt = conn.prepare("SELECT id, evidence FROM commitment")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, evidence) = row?;
        out.push((id, serde_json::from_str(&evidence).unwrap_or_default()));
    }
    Ok(out)
}

/// Attachments this connector's items use and no other item does.
fn only_ours(conn: &rusqlite::Connection, c: &str) -> Result<Vec<String>> {
    let set = |sql: &str| -> Result<HashSet<String>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([c], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    };
    let elsewhere =
        set("SELECT DISTINCT j.value FROM item i, json_each(i.blobs) j WHERE i.connector <> ?1")?;
    let ours =
        set("SELECT DISTINCT j.value FROM item i, json_each(i.blobs) j WHERE i.connector = ?1")?;
    Ok(ours.difference(&elsewhere).cloned().collect())
}

fn cursors_scopes(conn: &rusqlite::Connection, c: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT scope FROM sync_cursor WHERE connector = ?1")?;
    let rows = stmt.query_map([c], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Above this many items the full-text index is rebuilt once rather than
/// told about each deletion.
const REBUILD_ABOVE: usize = 2000;

/// The deletions behind a purge and behind marking something as junk: the
/// items, their annotations, chunks and vectors, their raw records, the
/// attachments only they used, and promises resting only on them.
#[allow(
    clippy::too_many_lines,
    reason = "one transaction, table by table, in the order the keys allow"
)]
fn remove_items(tx: &rusqlite::Transaction<'_>, ids: &HashSet<String>) -> Result<Purged> {
    let mut out = Purged::default();
    if ids.is_empty() {
        return Ok(out);
    }
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS doomed (id TEXT PRIMARY KEY);
         DELETE FROM doomed;",
    )?;
    {
        let mut put = tx.prepare("INSERT OR IGNORE INTO doomed (id) VALUES (?1)")?;
        for id in ids {
            put.execute([id])?;
        }
    }

    // Promises resting only on these items go; those that also rest on
    // something else lose these items from their evidence.
    for (id, list) in promises(tx)? {
        let left: Vec<&String> = list.iter().filter(|e| !ids.contains(*e)).collect();
        if left.len() == list.len() {
            continue;
        }
        if left.is_empty() {
            tx.execute("DELETE FROM commitment WHERE id = ?1", [&id])?;
            out.commitments += 1;
        } else {
            tx.execute(
                "UPDATE commitment SET evidence = ?2 WHERE id = ?1",
                params![id, serde_json::to_string(&left)?],
            )?;
        }
    }

    // Attachments only these items use.
    let set = |sql: &str| -> Result<HashSet<String>> {
        let mut stmt = tx.prepare(sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    };
    let ours = set(
        "SELECT DISTINCT j.value FROM item i, json_each(i.blobs) j WHERE i.id IN (SELECT id FROM doomed)",
    )?;
    let elsewhere = set(
        "SELECT DISTINCT j.value FROM item i, json_each(i.blobs) j WHERE i.id NOT IN (SELECT id FROM doomed)",
    )?;
    for hash in ours.difference(&elsewhere) {
        out.blobs += tx.execute("DELETE FROM blob WHERE hash = ?1", [hash])?;
        tx.execute("DELETE FROM blob_text WHERE hash = ?1", [hash])?;
        if let Ok(h) = hash.parse::<ContentHash>() {
            out.blob_files.push(h);
        }
    }

    // Vectors one by one: the vector table answers a lookup by row quickly
    // and a set of rows slowly.
    let chunks: Vec<i64> = {
        let mut stmt =
            tx.prepare("SELECT id FROM chunk WHERE item_id IN (SELECT id FROM doomed)")?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    {
        let mut del = tx.prepare("DELETE FROM chunk_vec WHERE rowid = ?1")?;
        for id in &chunks {
            del.execute([id])?;
        }
    }
    out.chunks = tx.execute(
        "DELETE FROM chunk WHERE item_id IN (SELECT id FROM doomed)",
        [],
    )?;
    out.annotations = tx.execute(
        "DELETE FROM annotation WHERE item_id IN (SELECT id FROM doomed)",
        [],
    )?;

    // The raw records behind these items, before the items go.
    let raws: Vec<(String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT r.id, r.hash FROM raw r JOIN item i ON i.raw_id = r.id
             WHERE i.id IN (SELECT id FROM doomed)",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<_, _>>()?
    };

    // Versions point at the versions they replace; loosen that first so the
    // rows can go in any order. A version outside the set that replaced one
    // inside it keeps standing on its own.
    tx.execute(
        "UPDATE item SET supersedes = NULL
         WHERE id IN (SELECT id FROM doomed) OR supersedes IN (SELECT id FROM doomed)",
        [],
    )?;
    if ids.len() > REBUILD_ABOVE {
        // The full-text index is told about each deletion by a trigger, one
        // row at a time; for many rows it is far quicker to let them go
        // unannounced and rebuild the index from what remains. The trigger
        // is put back in the same transaction.
        tx.execute_batch("DROP TRIGGER item_fts_ad;")?;
        out.items = tx.execute("DELETE FROM item WHERE id IN (SELECT id FROM doomed)", [])?;
        tx.execute_batch(
            "INSERT INTO item_fts(item_fts) VALUES ('rebuild');
             CREATE TRIGGER item_fts_ad AFTER DELETE ON item BEGIN
                 INSERT INTO item_fts(item_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
             END;",
        )?;
    } else {
        out.items = tx.execute("DELETE FROM item WHERE id IN (SELECT id FROM doomed)", [])?;
    }

    for (raw, hash) in raws {
        out.raws += tx.execute("DELETE FROM raw WHERE id = ?1", [&raw])?;
        let still: i64 =
            tx.query_row("SELECT count(*) FROM raw WHERE hash = ?1", [&hash], |r| {
                r.get(0)
            })?;
        if still == 0
            && let Ok(h) = hash.parse::<ContentHash>()
            && !out.raw_files.contains(&h)
        {
            out.raw_files.push(h);
        }
    }
    tx.execute("DELETE FROM doomed", [])?;
    Ok(out)
}
