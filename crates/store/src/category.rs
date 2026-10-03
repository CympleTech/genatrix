//! Categories (design 01, "分类"): the cached effective category of each
//! item, the user's word about a sender, and the senders whose mail is
//! quiet, for the one entry they are gathered under.

use genatrix_model::{Category, Item, PersonId};
use rusqlite::{OptionalExtension, params};

use crate::error::Result;
use crate::store::Store;

/// The categories hidden by default, as SQL.
pub const QUIET: &str = "('newsletter', 'promotion')";

/// A sender whose mail is newsletters or promotions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuietSender {
    /// Who.
    pub person: PersonId,
    /// Their name.
    pub name: String,
    /// How many of their items are quiet.
    pub items: u64,
    /// The newest, in UTC milliseconds.
    pub last_ms: i64,
    /// Which quiet category most of them are.
    pub category: Category,
}

impl Store {
    /// Items no category judgement has looked at yet, newest first.
    pub fn items_without_category(&self, limit: u32) -> Result<Vec<Item>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT i.* FROM item i
             WHERE NOT EXISTS (SELECT 1 FROM annotation a WHERE a.item_id = i.id AND a.kind = 'category')
             ORDER BY i.rowid DESC LIMIT ?1",
        )?;
        let mut rows = stmt.query([limit])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(crate::item::row_to_item(r)?);
        }
        Ok(out)
    }

    /// Cache an item's effective category.
    pub fn set_item_category(
        &self,
        item: genatrix_model::ItemId,
        category: Category,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE item SET category = ?2 WHERE id = ?1",
            params![item.to_string(), category.as_str()],
        )?;
        Ok(())
    }

    /// An item's cached category.
    pub fn item_category(&self, item: genatrix_model::ItemId) -> Result<Category> {
        let c: Option<String> = self
            .conn()
            .query_row(
                "SELECT category FROM item WHERE id = ?1",
                [item.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(c.as_deref().and_then(Category::parse).unwrap_or_default())
    }

    /// What the user said about a sender, if anything.
    pub fn person_category(&self, person: PersonId) -> Result<Option<Category>> {
        let c: Option<String> = self
            .conn()
            .query_row(
                "SELECT category FROM person WHERE id = ?1",
                [person.to_string()],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(c.as_deref().and_then(Category::parse))
    }

    /// Record what the user said about a sender.
    pub fn set_person_category(&self, person: PersonId, category: Option<Category>) -> Result<()> {
        self.conn().execute(
            "UPDATE person SET category = ?2 WHERE id = ?1",
            params![person.to_string(), category.map(Category::as_str)],
        )?;
        Ok(())
    }

    /// The category most of a sender's items are in, if they wrote any.
    pub fn sender_category(&self, person: PersonId) -> Result<Option<Category>> {
        let c: Option<String> = self
            .conn()
            .query_row(
                "SELECT category FROM item WHERE author = ?1
                 GROUP BY category ORDER BY count(*) DESC LIMIT 1",
                [person.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(c.as_deref().and_then(Category::parse))
    }

    /// The ids of every item a person wrote.
    pub fn items_by(&self, person: PersonId) -> Result<Vec<genatrix_model::ItemId>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id FROM item WHERE author = ?1")?;
        let rows = stmt.query_map([person.to_string()], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for id in rows {
            if let Ok(id) = id?.parse() {
                out.push(id);
            }
        }
        Ok(out)
    }

    /// Senders of quiet mail, newest first, and how many items that is in all.
    pub fn quiet_senders(&self, limit: u32) -> Result<(Vec<QuietSender>, u64)> {
        let conn = self.conn();
        let total: i64 = conn.query_row(
            &format!(
                "SELECT count(*) FROM item WHERE tombstoned = 0 AND category IN {QUIET}
                   AND NOT EXISTS (SELECT 1 FROM item n WHERE n.supersedes = item.id)"
            ),
            [],
            |r| r.get(0),
        )?;
        let mut stmt = conn.prepare(&format!(
            "SELECT i.author, coalesce(p.display_name, ''), count(*), max(i.occurred_ms),
                    sum(i.category = 'promotion')
             FROM item i JOIN person p ON p.id = i.author
             WHERE i.tombstoned = 0 AND i.category IN {QUIET} AND p.junk_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM item n WHERE n.supersedes = i.id)
             GROUP BY i.author ORDER BY max(i.occurred_ms) DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map([limit], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name, items, last_ms, promotions) = row?;
            let Ok(person) = id.parse() else { continue };
            out.push(QuietSender {
                person,
                name,
                items: u64::try_from(items).unwrap_or(0),
                last_ms,
                category: if promotions * 2 > items {
                    Category::Promotion
                } else {
                    Category::Newsletter
                },
            });
        }
        Ok((out, u64::try_from(total).unwrap_or(0)))
    }
}
