//! Items: insert, fetch, timeline queries, full-text search.

use chrono::{DateTime, Utc};
use genatrix_model::{
    ContentHash, Direction, Item, ItemId, Kind, Level, PersonId, Source, ThreadId,
};
use rusqlite::{OptionalExtension, Row, ToSql, params};

use crate::error::{Result, corrupt};
use crate::raw::connector_from_col;
use crate::store::Store;
use crate::time::{offset_from_col, offset_to_col, utc_from_col, utc_to_col};

const T: &str = "item";

fn direction_to_col(d: Direction) -> &'static str {
    match d {
        Direction::Inbound => "inbound",
        Direction::Outbound => "outbound",
        Direction::Internal => "internal",
        Direction::Neutral => "neutral",
    }
}

fn direction_from_col(s: &str) -> Result<Direction> {
    Ok(match s {
        "inbound" => Direction::Inbound,
        "outbound" => Direction::Outbound,
        "internal" => Direction::Internal,
        "neutral" => Direction::Neutral,
        other => return Err(corrupt(T, "direction", other)),
    })
}

fn kind_to_col(k: Kind) -> &'static str {
    match k {
        Kind::Mail => "mail",
        Kind::Message => "message",
        Kind::Event => "event",
        Kind::Note => "note",
        Kind::File => "file",
    }
}

pub(crate) fn level_from_col(table: &'static str, s: &str) -> Result<Level> {
    Ok(match s {
        "public" => Level::Public,
        "personal" => Level::Personal,
        "secret" => Level::Secret,
        other => return Err(corrupt(table, "sensitivity", other)),
    })
}

fn row_to_item(r: &Row<'_>) -> Result<Item> {
    let id: String = r.get("id")?;
    let connector: String = r.get("connector")?;
    let raw_id: String = r.get("raw_id")?;
    let supersedes: Option<String> = r.get("supersedes")?;
    let thread_id: String = r.get("thread_id")?;
    let occurred_at: String = r.get("occurred_at")?;
    let ingested_at: String = r.get("ingested_at")?;
    let direction: String = r.get("direction")?;
    let author: Option<String> = r.get("author")?;
    let recipients: String = r.get("recipients")?;
    let blobs: String = r.get("blobs")?;
    let sensitivity: String = r.get("sensitivity")?;
    let payload: String = r.get("payload")?;
    let recipients: Vec<PersonId> =
        serde_json::from_str(&recipients).map_err(|e| corrupt(T, "recipients", e))?;
    let blobs: Vec<ContentHash> =
        serde_json::from_str(&blobs).map_err(|e| corrupt(T, "blobs", e))?;
    Ok(Item {
        id: id.parse().map_err(|e| corrupt(T, "id", e))?,
        source: Source::new(
            connector_from_col(T, &connector)?,
            r.get::<_, String>("account")?,
            r.get::<_, String>("external_id")?,
        ),
        raw_id: raw_id.parse().map_err(|e| corrupt(T, "raw_id", e))?,
        supersedes: supersedes
            .map(|s| s.parse().map_err(|e| corrupt(T, "supersedes", e)))
            .transpose()?,
        thread_id: thread_id.parse().map_err(|e| corrupt(T, "thread_id", e))?,
        occurred_at: offset_from_col(T, "occurred_at", &occurred_at)?,
        ingested_at: utc_from_col(T, "ingested_at", &ingested_at)?,
        direction: direction_from_col(&direction)?,
        author: author
            .map(|s| s.parse().map_err(|e| corrupt(T, "author", e)))
            .transpose()?,
        recipients,
        text: r.get("text")?,
        blobs,
        sensitivity: level_from_col(T, &sensitivity)?,
        tombstoned: r.get::<_, i64>("tombstoned")? != 0,
        payload: serde_json::from_str(&payload).map_err(|e| corrupt(T, "payload", e))?,
    })
}

/// Which versions of an item a query returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemVersion {
    /// Only the latest version of each source object: rows no other row
    /// supersedes. The timeline default.
    #[default]
    Current,
    /// Every stored version.
    All,
}

/// A timeline query. Every field is optional; the result is ordered by
/// `occurred_at` descending. Ingestion time never affects order.
#[derive(Clone, Debug, Default)]
pub struct ItemQuery {
    /// Only items at or after this instant.
    pub since: Option<DateTime<Utc>>,
    /// Only items before this instant.
    pub until: Option<DateTime<Utc>>,
    /// Only these kinds.
    pub kinds: Vec<Kind>,
    /// Only this direction.
    pub direction: Option<Direction>,
    /// Only this thread.
    pub thread: Option<ThreadId>,
    /// Only items authored by or addressed to this person.
    pub person: Option<PersonId>,
    /// Only items at most this sensitive. Used by the gate to scope reads.
    pub max_level: Option<Level>,
    /// Include upstream-deleted items. Off by default.
    pub include_tombstoned: bool,
    /// Which versions.
    pub version: ItemVersion,
    /// Page size.
    pub limit: u32,
    /// Offset into the ordered result.
    pub offset: u32,
}

impl ItemQuery {
    fn build(&self) -> (String, Vec<Box<dyn ToSql>>) {
        let mut where_: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        let mut arg = |v: Box<dyn ToSql>| -> String {
            args.push(v);
            format!("?{}", args.len())
        };
        if let Some(t) = self.since {
            let p = arg(Box::new(t.timestamp_millis()));
            where_.push(format!("i.occurred_ms >= {p}"));
        }
        if let Some(t) = self.until {
            let p = arg(Box::new(t.timestamp_millis()));
            where_.push(format!("i.occurred_ms < {p}"));
        }
        if !self.kinds.is_empty() {
            let ps: Vec<String> = self
                .kinds
                .iter()
                .map(|k| arg(Box::new(kind_to_col(*k))))
                .collect();
            where_.push(format!("i.kind IN ({})", ps.join(",")));
        }
        if let Some(d) = self.direction {
            let p = arg(Box::new(direction_to_col(d)));
            where_.push(format!("i.direction = {p}"));
        }
        if let Some(t) = self.thread {
            let p = arg(Box::new(t.to_string()));
            where_.push(format!("i.thread_id = {p}"));
        }
        if let Some(pid) = self.person {
            let p = arg(Box::new(pid.to_string()));
            where_.push(format!(
                "(i.author = {p} OR EXISTS (SELECT 1 FROM json_each(i.recipients) WHERE value = {p}))"
            ));
        }
        if let Some(l) = self.max_level {
            let allowed: Vec<String> = [Level::Public, Level::Personal, Level::Secret]
                .into_iter()
                .filter(|x| *x <= l)
                .map(|x| arg(Box::new(x.as_str())))
                .collect();
            where_.push(format!("i.sensitivity IN ({})", allowed.join(",")));
        }
        if !self.include_tombstoned {
            where_.push("i.tombstoned = 0".into());
        }
        if self.version == ItemVersion::Current {
            where_.push("NOT EXISTS (SELECT 1 FROM item n WHERE n.supersedes = i.id)".into());
        }
        let limit = if self.limit == 0 { 100 } else { self.limit };
        let sql = format!(
            "SELECT i.* FROM item i {} ORDER BY i.occurred_ms DESC, i.id DESC LIMIT {} OFFSET {}",
            if where_.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", where_.join(" AND "))
            },
            limit,
            self.offset
        );
        (sql, args)
    }
}

impl Store {
    /// Insert an item. The referenced raw, thread, and persons must exist.
    pub fn insert_item(&self, item: &Item) -> Result<()> {
        self.conn().execute(
            "INSERT INTO item
                (id, connector, account, external_id, raw_id, supersedes, thread_id,
                 occurred_at, occurred_ms, ingested_at, direction, author, recipients,
                 text, blobs, sensitivity, tombstoned, kind, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                item.id.to_string(),
                item.source.connector.as_str(),
                item.source.account,
                item.source.external_id,
                item.raw_id.to_string(),
                item.supersedes.map(|s| s.to_string()),
                item.thread_id.to_string(),
                offset_to_col(item.occurred_at),
                item.occurred_at.timestamp_millis(),
                utc_to_col(item.ingested_at),
                direction_to_col(item.direction),
                item.author.map(|a| a.to_string()),
                serde_json::to_string(&item.recipients)?,
                item.text,
                serde_json::to_string(&item.blobs)?,
                item.sensitivity.as_str(),
                i64::from(item.tombstoned),
                kind_to_col(item.kind()),
                serde_json::to_string(&item.payload)?,
            ],
        )?;
        Ok(())
    }

    /// Replace what was derived from the raw record: conversation, people,
    /// time, text, attachments, payload. Identity, source, raw record,
    /// sensitivity and tombstone stay. This is what a better normalization
    /// does to an existing item (design 01: a version is a new raw record,
    /// a re-derivation is not).
    pub fn rederive_item(&self, item: &Item) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE item SET thread_id = ?2, occurred_at = ?3, occurred_ms = ?4,
                 direction = ?5, author = ?6, recipients = ?7, text = ?8, blobs = ?9,
                 kind = ?10, payload = ?11
             WHERE id = ?1",
            params![
                item.id.to_string(),
                item.thread_id.to_string(),
                offset_to_col(item.occurred_at),
                item.occurred_at.timestamp_millis(),
                direction_to_col(item.direction),
                item.author.map(|a| a.to_string()),
                serde_json::to_string(&item.recipients)?,
                item.text,
                serde_json::to_string(&item.blobs)?,
                kind_to_col(item.kind()),
                serde_json::to_string(&item.payload)?,
            ],
        )?;
        if n == 0 {
            return Err(crate::error::Error::NotFound(format!("item {}", item.id)));
        }
        Ok(())
    }

    /// A random sample of current items the model has judged and the user
    /// has not yet confirmed or corrected: what a review pass looks at
    /// (design 04, "评测集": the user's overrides are the evaluation set).
    pub fn items_for_review(&self, limit: u32) -> Result<Vec<Item>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT i.* FROM item i
             WHERE i.tombstoned = 0
               AND NOT EXISTS (SELECT 1 FROM item n WHERE n.supersedes = i.id)
               AND EXISTS (SELECT 1 FROM annotation a WHERE a.item_id = i.id
                           AND a.kind = 'sensitivity' AND a.producer LIKE '%\"by\":\"model\"%')
               AND NOT EXISTS (SELECT 1 FROM annotation u WHERE u.item_id = i.id
                           AND u.kind = 'sensitivity' AND u.producer LIKE '%\"by\":\"user\"%')
             ORDER BY random() LIMIT ?1",
        )?;
        let mut rows = stmt.query(params![limit])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(row_to_item(r)?);
        }
        Ok(out)
    }

    /// Fetch an item by id.
    pub fn get_item(&self, id: ItemId) -> Result<Option<Item>> {
        self.conn()
            .query_row("SELECT * FROM item WHERE id = ?1", [id.to_string()], |r| {
                Ok(row_to_item(r))
            })
            .optional()?
            .transpose()
    }

    /// The current version of the item for a source object, if any.
    pub fn current_item(&self, source: &Source) -> Result<Option<Item>> {
        self.conn()
            .query_row(
                "SELECT i.* FROM item i
                 WHERE i.connector = ?1 AND i.account = ?2 AND i.external_id = ?3
                   AND NOT EXISTS (SELECT 1 FROM item n WHERE n.supersedes = i.id)
                 ORDER BY i.id DESC LIMIT 1",
                params![
                    source.connector.as_str(),
                    source.account,
                    source.external_id
                ],
                |r| Ok(row_to_item(r)),
            )
            .optional()?
            .transpose()
    }

    /// Update the cached effective sensitivity of an item. The judgement
    /// itself is an annotation; this only refreshes the cache.
    pub fn set_item_sensitivity(&self, id: ItemId, level: Level) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE item SET sensitivity = ?2 WHERE id = ?1",
            params![id.to_string(), level.as_str()],
        )?;
        if n == 0 {
            return Err(crate::Error::NotFound(format!("item {id}")));
        }
        Ok(())
    }

    /// Run a timeline query.
    pub fn query_items(&self, q: &ItemQuery) -> Result<Vec<Item>> {
        let (sql, args) = q.build();
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn ToSql> = args.iter().map(AsRef::as_ref).collect();
        let rows = stmt.query_map(refs.as_slice(), |r| Ok(row_to_item(r)))?;
        rows.map(|r| r?).collect()
    }

    /// Full-text search over item text. The query is matched as a phrase,
    /// so FTS syntax in user input is inert. Trigram indexing needs at least
    /// three characters; shorter queries return nothing.
    pub fn search_items(&self, query: &str, limit: u32) -> Result<Vec<Item>> {
        let q = query.trim();
        if q.chars().count() < 3 {
            return Ok(Vec::new());
        }
        let phrase = format!("\"{}\"", q.replace('"', "\"\""));
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT i.* FROM item_fts f JOIN item i ON i.rowid = f.rowid
             WHERE item_fts MATCH ?1 AND i.tombstoned = 0
               AND NOT EXISTS (SELECT 1 FROM item n WHERE n.supersedes = i.id)
             ORDER BY f.rank LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![phrase, limit.max(1)], |r| Ok(row_to_item(r)))?;
        rows.map(|r| r?).collect()
    }

    /// Number of items stored, all versions.
    pub fn count_items(&self) -> Result<u64> {
        let n: i64 = self
            .conn()
            .query_row("SELECT count(*) FROM item", [], |r| r.get(0))?;
        Ok(n.try_into().unwrap_or(0))
    }

    /// All items, all versions, in id order. Used by export.
    pub fn all_items(&self) -> Result<Vec<Item>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM item ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok(row_to_item(r)))?;
        rows.map(|r| r?).collect()
    }
}
