//! Commitments and digests.

use chrono::NaiveDate;
use genatrix_model::{
    Commitment, CommitmentId, CommitmentStatus, Digest, ItemId, PersonId, Standing,
};
use rusqlite::{OptionalExtension, Row, params};

use crate::error::{Result, corrupt};
use crate::store::Store;
use crate::time::{opt_utc_from_col, opt_utc_to_col, utc_from_col, utc_to_col};

const T: &str = "commitment";

pub(crate) fn row_to_commitment(r: &Row<'_>) -> Result<Commitment> {
    let id: String = r.get("id")?;
    let from: String = r.get("from_person")?;
    let to: Option<String> = r.get("to_person")?;
    let evidence: String = r.get("evidence")?;
    let status: String = r.get("status")?;
    let standing: String = r.get("standing")?;
    let created_at: String = r.get("created_at")?;
    Ok(Commitment {
        id: id.parse().map_err(|e| corrupt(T, "id", e))?,
        from: from.parse().map_err(|e| corrupt(T, "from_person", e))?,
        to: to
            .map(|t| {
                t.parse::<PersonId>()
                    .map_err(|e| corrupt(T, "to_person", e))
            })
            .transpose()?,
        what: r.get("what")?,
        due: opt_utc_from_col(T, "due", r.get("due")?)?,
        evidence: serde_json::from_str::<Vec<ItemId>>(&evidence)
            .map_err(|e| corrupt(T, "evidence", e))?,
        status: CommitmentStatus::parse(&status).ok_or_else(|| corrupt(T, "status", &status))?,
        standing: Standing::parse(&standing).ok_or_else(|| corrupt(T, "standing", &standing))?,
        created_at: utc_from_col(T, "created_at", &created_at)?,
    })
}

impl Store {
    /// Record a commitment.
    pub fn insert_commitment(&self, c: &Commitment) -> Result<()> {
        self.conn().execute(
            "INSERT INTO commitment (id, from_person, to_person, what, due, evidence, status, standing, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                c.id.to_string(),
                c.from.to_string(),
                c.to.map(|p| p.to_string()),
                c.what,
                opt_utc_to_col(c.due),
                serde_json::to_string(&c.evidence)?,
                c.status.as_str(),
                c.standing.as_str(),
                utc_to_col(c.created_at),
            ],
        )?;
        Ok(())
    }

    /// One commitment.
    pub fn get_commitment(&self, id: CommitmentId) -> Result<Option<Commitment>> {
        self.conn()
            .query_row(
                "SELECT * FROM commitment WHERE id = ?1",
                params![id.to_string()],
                |r| Ok(row_to_commitment(r)),
            )
            .optional()?
            .transpose()
    }

    /// The user's word on a commitment, and whether it is done.
    pub fn set_commitment(
        &self,
        id: CommitmentId,
        standing: Standing,
        status: CommitmentStatus,
    ) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE commitment SET standing = ?2, status = ?3 WHERE id = ?1",
            params![id.to_string(), standing.as_str(), status.as_str()],
        )?;
        if n == 0 {
            return Err(crate::Error::NotFound(format!("commitment {id}")));
        }
        Ok(())
    }

    /// Every commitment still to keep, not rejected, soonest due first and
    /// the undated last.
    pub fn pending_commitments(&self) -> Result<Vec<Commitment>> {
        self.pending_commitments_upto(0)
    }

    /// The same, at most `limit` of them; 0 means all. Dated ones first,
    /// soonest first, then the undated with the most recent first, which is
    /// the order a person would work down.
    pub fn pending_commitments_upto(&self, limit: u32) -> Result<Vec<Commitment>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT * FROM commitment
             WHERE status IN ('open', 'overdue') AND standing <> 'rejected'
             ORDER BY due IS NULL, due, created_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(
            params![if limit == 0 { -1 } else { i64::from(limit) }],
            |r| Ok(row_to_commitment(r)),
        )?;
        rows.map(|r| r?).collect()
    }

    /// How many are open, without reading any of them.
    pub fn count_pending_commitments(&self) -> Result<u64> {
        let n: i64 = self.conn().query_row(
            "SELECT count(*) FROM commitment
             WHERE status IN ('open', 'overdue') AND standing <> 'rejected'",
            [],
            |r| r.get(0),
        )?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Commitments drawn from an item.
    pub fn commitments_from(&self, item: ItemId) -> Result<Vec<Commitment>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT * FROM commitment WHERE evidence LIKE ?1 ORDER BY created_at")?;
        let rows = stmt.query_map(params![format!("%\"{item}\"%")], |r| {
            Ok(row_to_commitment(r))
        })?;
        rows.map(|r| r?).collect()
    }

    /// Keep a day's digest, replacing an earlier one for the same day.
    pub fn put_digest(&self, digest: &Digest) -> Result<()> {
        self.conn().execute(
            "INSERT INTO digest (day, generated_at, value) VALUES (?1, ?2, ?3)
             ON CONFLICT (day) DO UPDATE SET generated_at = excluded.generated_at, value = excluded.value",
            params![
                digest.day.to_string(),
                utc_to_col(digest.generated_at),
                serde_json::to_string(digest)?,
            ],
        )?;
        Ok(())
    }

    /// One day's digest.
    pub fn get_digest(&self, day: NaiveDate) -> Result<Option<Digest>> {
        let value: Option<String> = self
            .conn()
            .query_row(
                "SELECT value FROM digest WHERE day = ?1",
                params![day.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        value
            .map(|v| serde_json::from_str(&v).map_err(|e| corrupt("digest", "value", e)))
            .transpose()
    }

    /// The most recent digest, if any.
    pub fn latest_digest(&self) -> Result<Option<Digest>> {
        let value: Option<String> = self
            .conn()
            .query_row(
                "SELECT value FROM digest ORDER BY day DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        value
            .map(|v| serde_json::from_str(&v).map_err(|e| corrupt("digest", "value", e)))
            .transpose()
    }

    /// Whether an open promise from `from` to `to` with the same words was
    /// recorded since `since`: the same promise said again.
    pub fn open_commitment_like(
        &self,
        from: genatrix_model::PersonId,
        to: Option<genatrix_model::PersonId>,
        what: &str,
        since: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool> {
        let n: i64 = self.conn().query_row(
            "SELECT count(*) FROM commitment
             WHERE from_person = ?1 AND to_person IS ?2 AND lower(trim(what)) = lower(trim(?3))
               AND status IN ('open', 'overdue') AND created_at >= ?4",
            params![
                from.to_string(),
                to.map(|t| t.to_string()),
                what,
                crate::time::utc_to_col(since)
            ],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Promises the model inferred and the user never judged. Counted, or
    /// removed so that the messages can be read again under new rules; what
    /// the user confirmed or rejected stays (design 07: a rejection is
    /// remembered).
    pub fn unjudged_commitments(&self, remove: bool) -> Result<usize> {
        let conn = self.conn();
        if remove {
            Ok(conn.execute("DELETE FROM commitment WHERE standing = 'inferred'", [])?)
        } else {
            let n: i64 = conn.query_row(
                "SELECT count(*) FROM commitment WHERE standing = 'inferred'",
                [],
                |r| r.get(0),
            )?;
            Ok(usize::try_from(n).unwrap_or(0))
        }
    }

    /// Current items of these directions without a given label annotation,
    /// newest first: what a pipeline that marks its work with a label has
    /// not yet looked at.
    pub fn items_without_label(
        &self,
        label: &str,
        limit: u32,
    ) -> Result<Vec<genatrix_model::Item>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT i.* FROM item i
             WHERE i.tombstoned = 0 AND length(i.text) > 0
               AND NOT EXISTS (SELECT 1 FROM item n WHERE n.supersedes = i.id)
               AND NOT EXISTS (SELECT 1 FROM annotation a WHERE a.item_id = i.id
                               AND a.kind = 'label' AND a.value LIKE ?1)
             ORDER BY i.occurred_ms DESC LIMIT ?2",
        )?;
        let pattern = format!("%\"label\":\"{label}\"%");
        let mut rows = stmt.query(params![pattern, limit])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(crate::item::row_to_item(r)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DbKey;
    use chrono::Utc;
    use genatrix_model::{DigestGroup, DigestPoint, HandleKind};

    #[test]
    fn a_commitment_is_kept_listed_and_judged() {
        let store = Store::open_in_memory(&DbKey::from_bytes([7; 32])).unwrap();
        let me = store
            .person_for_handle(HandleKind::Email, "me@example.com", "Me")
            .unwrap();
        let them = store
            .person_for_handle(HandleKind::Email, "a@example.com", "Ann")
            .unwrap();
        let item = ItemId::new();
        let c = Commitment {
            id: CommitmentId::new(),
            from: me,
            to: Some(them),
            what: "send the quote".into(),
            due: Some(Utc::now()),
            evidence: vec![item],
            status: CommitmentStatus::Open,
            standing: Standing::Inferred,
            created_at: Utc::now(),
        };
        store.insert_commitment(&c).unwrap();
        assert_eq!(store.pending_commitments().unwrap(), vec![c.clone()]);
        assert_eq!(store.count_pending_commitments().unwrap(), 1);
        assert_eq!(store.pending_commitments_upto(1).unwrap().len(), 1);
        assert_eq!(store.commitments_from(item).unwrap().len(), 1);
        store
            .set_commitment(c.id, Standing::Rejected, CommitmentStatus::Cancelled)
            .unwrap();
        assert!(store.pending_commitments().unwrap().is_empty());
        assert_eq!(store.count_pending_commitments().unwrap(), 0);
        let back = store.get_commitment(c.id).unwrap().unwrap();
        assert_eq!(back.standing, Standing::Rejected);
    }

    #[test]
    fn a_digest_is_kept_by_day_and_the_latest_is_found() {
        let store = Store::open_in_memory(&DbKey::from_bytes([7; 32])).unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 9, 22).unwrap();
        let digest = Digest {
            day,
            generated_at: Utc::now(),
            considered: 3,
            groups: vec![(
                DigestGroup::NeedsReply,
                vec![DigestPoint {
                    text: "Ann asked about Thursday".into(),
                    sources: vec![ItemId::new()],
                }],
            )],
        };
        store.put_digest(&digest).unwrap();
        assert_eq!(store.get_digest(day).unwrap(), Some(digest.clone()));
        assert_eq!(store.latest_digest().unwrap(), Some(digest.clone()));
        assert!(store.get_digest(day.succ_opt().unwrap()).unwrap().is_none());
        assert_eq!(digest.points(DigestGroup::NeedsReply).len(), 1);
        assert!(digest.points(DigestGroup::Promised).is_empty());
    }
}
