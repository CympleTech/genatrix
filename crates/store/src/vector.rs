//! Chunks and their vectors: writing them, and finding the nearest.
//!
//! Design: `docs/design/08-storage.md` (sqlite-vec, one file) and
//! `docs/design/01-data-model.md` (an Embedding annotation carries the
//! vector and the chunk's position; chunking belongs to the producer).

use genatrix_model::{Annotation, AnnotationKind, Item, ItemId, Producer};
use rusqlite::params;

use crate::error::{Result, corrupt};
use crate::item::row_to_item;
use crate::store::Store;

/// The embedder this schema is built for.
pub const EMBEDDING_DIMS: usize = 384;

/// A chunk's vector, as JSON for the extension.
fn vector_json(vector: &[f32]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(vector.len() * 10);
    out.push('[');
    for (i, v) in vector.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "{v}");
    }
    out.push(']');
    out
}

impl Store {
    /// Record one chunk's vector: the annotation, the chunk row, and the
    /// index entry, in one transaction. Replaces an earlier vector for the
    /// same chunk.
    pub fn put_embedding(
        &self,
        item_id: ItemId,
        idx: u32,
        range: (u32, u32),
        vector: &[f32],
        producer: &Producer,
    ) -> Result<()> {
        if vector.len() != EMBEDDING_DIMS {
            return Err(corrupt(
                "chunk_vec",
                "embedding",
                format!("{} dimensions, schema has {EMBEDDING_DIMS}", vector.len()),
            ));
        }
        let annotation = Annotation::new(
            item_id,
            producer.clone(),
            AnnotationKind::Embedding {
                chunk: idx,
                range,
                vector: vector.to_vec(),
            },
        );
        let json = vector_json(vector);
        self.tx(|tx| {
            // The annotation, through the same path every annotation takes.
            tx.execute(
                "INSERT INTO annotation (id, item_id, producer, created_at, kind, value, superseded_by)
                 VALUES (?1, ?2, ?3, ?4, 'embedding', ?5, NULL)",
                params![
                    annotation.id.to_string(),
                    item_id.to_string(),
                    serde_json::to_string(producer)?,
                    crate::time::utc_to_col(annotation.created_at),
                    serde_json::to_string(&annotation.kind)?,
                ],
            )?;
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM chunk WHERE item_id = ?1 AND idx = ?2",
                    params![item_id.to_string(), idx],
                    |r| r.get(0),
                )
                .ok();
            let chunk_id = if let Some(id) = existing {
                tx.execute("DELETE FROM chunk_vec WHERE rowid = ?1", params![id])?;
                tx.execute(
                    "UPDATE chunk SET start = ?2, end_ = ?3 WHERE id = ?1",
                    params![id, range.0, range.1],
                )?;
                id
            } else {
                tx.execute(
                    "INSERT INTO chunk (item_id, idx, start, end_) VALUES (?1, ?2, ?3, ?4)",
                    params![item_id.to_string(), idx, range.0, range.1],
                )?;
                tx.last_insert_rowid()
            };
            tx.execute(
                "INSERT INTO chunk_vec(rowid, embedding) VALUES (?1, ?2)",
                params![chunk_id, json],
            )?;
            Ok(())
        })
    }

    /// The items nearest to a query vector, best chunk per item, nearest
    /// first, with the distance. Current, untombstoned items only.
    pub fn similar_items(&self, query: &[f32], limit: u32) -> Result<Vec<(Item, f32)>> {
        if query.len() != EMBEDDING_DIMS {
            return Err(corrupt(
                "chunk_vec",
                "query",
                format!("{} dimensions, schema has {EMBEDDING_DIMS}", query.len()),
            ));
        }
        let json = vector_json(query);
        let conn = self.conn();
        // Ask for more chunks than items wanted: several chunks of one item
        // may be near.
        let k = i64::from(limit.max(1)) * 4;
        let mut stmt = conn.prepare(
            "SELECT c.item_id, v.distance FROM chunk_vec v
             JOIN chunk c ON c.id = v.rowid
             WHERE v.embedding MATCH ?1 AND k = ?2
             ORDER BY v.distance",
        )?;
        let mut seen: Vec<(String, f32)> = Vec::new();
        for row in stmt.query_map(params![json, k], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
        })? {
            let (item_id, distance) = row?;
            if !seen.iter().any(|(id, _)| *id == item_id) {
                #[expect(clippy::cast_possible_truncation, reason = "a distance, for ordering")]
                seen.push((item_id, distance as f32));
            }
        }
        drop(stmt);

        let mut out = Vec::new();
        let mut fetch = conn.prepare(
            "SELECT i.* FROM item i WHERE i.id = ?1 AND i.tombstoned = 0
               AND NOT EXISTS (SELECT 1 FROM item n WHERE n.supersedes = i.id)",
        )?;
        for (item_id, distance) in seen.into_iter().take(limit as usize) {
            let mut rows = fetch.query(params![item_id])?;
            if let Some(r) = rows.next()? {
                out.push((row_to_item(r)?, distance));
            }
        }
        Ok(out)
    }

    /// Current items with text and no vector yet, oldest first so history
    /// fills in evenly. `min_chars` skips items too short to be worth it.
    pub fn items_without_embedding(&self, limit: u32) -> Result<Vec<Item>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT i.* FROM item i
             WHERE i.tombstoned = 0 AND length(i.text) > 0
               AND NOT EXISTS (SELECT 1 FROM item n WHERE n.supersedes = i.id)
               AND NOT EXISTS (SELECT 1 FROM chunk c WHERE c.item_id = i.id)
             ORDER BY i.occurred_ms DESC LIMIT ?1",
        )?;
        let mut rows = stmt.query(params![limit])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(row_to_item(r)?);
        }
        Ok(out)
    }

    /// How many items have at least one vector.
    pub fn count_embedded_items(&self) -> Result<u64> {
        let n: i64 =
            self.conn()
                .query_row("SELECT count(DISTINCT item_id) FROM chunk", [], |r| {
                    r.get(0)
                })?;
        Ok(u64::try_from(n).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vector_json_is_what_the_extension_expects() {
        assert_eq!(vector_json(&[1.0, -0.5, 0.25]), "[1,-0.5,0.25]");
    }
}
