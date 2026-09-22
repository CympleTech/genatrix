//! Annotations.

use genatrix_model::{Annotation, AnnotationId, AnnotationKind, ItemId};
use rusqlite::{OptionalExtension, Row, params};

use crate::error::{Result, corrupt};
use crate::store::Store;
use crate::time::{utc_from_col, utc_to_col};

const T: &str = "annotation";

fn kind_discriminator(k: &AnnotationKind) -> &'static str {
    match k {
        AnnotationKind::Summary { .. } => "summary",
        AnnotationKind::Embedding { .. } => "embedding",
        AnnotationKind::Sensitivity { .. } => "sensitivity",
        AnnotationKind::Label { .. } => "label",
        AnnotationKind::Suggestion { .. } => "suggestion",
    }
}

fn row_to_annotation(r: &Row<'_>) -> Result<Annotation> {
    let id: String = r.get("id")?;
    let item_id: String = r.get("item_id")?;
    let producer: String = r.get("producer")?;
    let created_at: String = r.get("created_at")?;
    let value: String = r.get("value")?;
    let superseded_by: Option<String> = r.get("superseded_by")?;
    Ok(Annotation {
        id: id.parse().map_err(|e| corrupt(T, "id", e))?,
        item_id: item_id.parse().map_err(|e| corrupt(T, "item_id", e))?,
        producer: serde_json::from_str(&producer).map_err(|e| corrupt(T, "producer", e))?,
        created_at: utc_from_col(T, "created_at", &created_at)?,
        kind: serde_json::from_str(&value).map_err(|e| corrupt(T, "value", e))?,
        superseded_by: superseded_by
            .map(|s| s.parse().map_err(|e| corrupt(T, "superseded_by", e)))
            .transpose()?,
    })
}

impl Store {
    /// Insert an annotation.
    pub fn insert_annotation(&self, a: &Annotation) -> Result<()> {
        self.conn().execute(
            "INSERT INTO annotation (id, item_id, producer, created_at, kind, value, superseded_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                a.id.to_string(),
                a.item_id.to_string(),
                serde_json::to_string(&a.producer)?,
                utc_to_col(a.created_at),
                kind_discriminator(&a.kind),
                serde_json::to_string(&a.kind)?,
                a.superseded_by.map(|s| s.to_string()),
            ],
        )?;
        Ok(())
    }

    /// Mark `old` as replaced by `new`.
    pub fn supersede_annotation(&self, old: AnnotationId, new: AnnotationId) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE annotation SET superseded_by = ?2 WHERE id = ?1 AND superseded_by IS NULL",
            params![old.to_string(), new.to_string()],
        )?;
        if n == 0 {
            return Err(crate::Error::NotFound(format!("annotation {old}")));
        }
        Ok(())
    }

    /// Fetch one annotation.
    pub fn get_annotation(&self, id: AnnotationId) -> Result<Option<Annotation>> {
        self.conn()
            .query_row(
                "SELECT * FROM annotation WHERE id = ?1",
                [id.to_string()],
                |r| Ok(row_to_annotation(r)),
            )
            .optional()?
            .transpose()
    }

    /// Annotations of an item, oldest first, including superseded ones.
    pub fn annotations_of(&self, item: ItemId) -> Result<Vec<Annotation>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM annotation WHERE item_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map([item.to_string()], |r| Ok(row_to_annotation(r)))?;
        rows.map(|r| r?).collect()
    }
    /// Current items whose text is at least `min_chars` characters and that
    /// have no summary yet, newest first.
    pub fn items_without_summary(
        &self,
        min_chars: usize,
        limit: u32,
    ) -> Result<Vec<genatrix_model::Item>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT i.* FROM item i
             WHERE i.tombstoned = 0 AND length(i.text) >= ?1
               AND NOT EXISTS (SELECT 1 FROM item n WHERE n.supersedes = i.id)
               AND NOT EXISTS (SELECT 1 FROM annotation a WHERE a.item_id = i.id
                               AND a.kind = 'summary' AND a.superseded_by IS NULL)
             ORDER BY i.occurred_ms DESC LIMIT ?2",
        )?;
        let mut rows = stmt.query(params![i64::try_from(min_chars).unwrap_or(i64::MAX), limit])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(crate::item::row_to_item(r)?);
        }
        Ok(out)
    }

    /// How many items carry at least one annotation of a kind
    /// (`summary`, `embedding`, `sensitivity`, ...).
    pub fn count_annotated_items(&self, kind: &str) -> Result<u64> {
        let n: i64 = self.conn().query_row(
            "SELECT count(DISTINCT item_id) FROM annotation WHERE kind = ?1 AND superseded_by IS NULL",
            params![kind],
            |r| r.get(0),
        )?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// All annotations. Used by export.
    pub fn all_annotations(&self) -> Result<Vec<Annotation>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM annotation ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok(row_to_annotation(r)))?;
        rows.map(|r| r?).collect()
    }
}
