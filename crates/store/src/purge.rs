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

    #[allow(
        clippy::too_many_lines,
        reason = "one transaction, table by table, in the order the keys allow"
    )]
    fn purge_now(&self, connector: Connector, keep_cursors: &[&str]) -> Result<Purged> {
        let c = connector.as_str();
        let mut guard = self.conn();
        let tx = guard.transaction()?;
        let mut out = Purged::default();
        let ids = item_ids(&tx, c)?;

        // Promises resting only on these items go; those that also rest on
        // something else lose these items from their evidence.
        for (id, list) in promises(&tx)? {
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

        for hash in only_ours(&tx, c)? {
            out.blobs += tx.execute("DELETE FROM blob WHERE hash = ?1", [&hash])?;
            if let Ok(h) = hash.parse::<ContentHash>() {
                out.blob_files.push(h);
            }
        }

        // Vectors one by one: the vector table answers a lookup by row
        // quickly and a set of rows slowly.
        let chunks: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT ch.id FROM chunk ch JOIN item i ON i.id = ch.item_id WHERE i.connector = ?1",
            )?;
            let rows = stmt.query_map([c], |r| r.get::<_, i64>(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        {
            let mut del = tx.prepare("DELETE FROM chunk_vec WHERE rowid = ?1")?;
            for id in &chunks {
                del.execute([id])?;
            }
        }
        out.chunks = tx.execute(
            "DELETE FROM chunk WHERE item_id IN (SELECT id FROM item WHERE connector = ?1)",
            [c],
        )?;
        out.annotations = tx.execute(
            "DELETE FROM annotation WHERE item_id IN (SELECT id FROM item WHERE connector = ?1)",
            [c],
        )?;
        // Versions point at the versions they replace; loosen that first so
        // the rows can go in any order.
        tx.execute(
            "UPDATE item SET supersedes = NULL WHERE connector = ?1",
            [c],
        )?;
        // The full-text index is told about each deletion by a trigger, one
        // row at a time. For a whole connector it is far quicker to let the
        // rows go unannounced and rebuild the index from what remains; the
        // trigger is put back in the same transaction.
        tx.execute_batch("DROP TRIGGER item_fts_ad;")?;
        out.items = tx.execute("DELETE FROM item WHERE connector = ?1", [c])?;
        tx.execute_batch(
            "INSERT INTO item_fts(item_fts) VALUES ('rebuild');
             CREATE TRIGGER item_fts_ad AFTER DELETE ON item BEGIN
                 INSERT INTO item_fts(item_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
             END;",
        )?;

        let raw_hashes: Vec<String> = {
            let mut stmt = tx.prepare("SELECT DISTINCT hash FROM raw WHERE connector = ?1")?;
            let rows = stmt.query_map([c], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        out.raws = tx.execute("DELETE FROM raw WHERE connector = ?1", [c])?;
        for hash in raw_hashes {
            let still: i64 =
                tx.query_row("SELECT count(*) FROM raw WHERE hash = ?1", [&hash], |r| {
                    r.get(0)
                })?;
            if still == 0
                && let Ok(h) = hash.parse::<ContentHash>()
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
