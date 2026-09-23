//! People, as seen through what passed between them and the user.
//!
//! Design: `docs/design/07-memory-profile.md`, "画像": a relationship is a
//! person, the roles the user confirmed, the notes the user wrote, and
//! statistics computed from the data, which need no confirmation because
//! they are arithmetic over items. Nothing here is the model's opinion.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Utc};
use genatrix_model::{Commitment, Item, Person, PersonId};
use rusqlite::{OptionalExtension, params};

use crate::error::Result;
use crate::item::row_to_item;
use crate::store::Store;
use crate::time::{utc_from_col, utc_to_col};

/// What counts between the user and a person: what passed one to one, by
/// mail or in a direct chat. What they said in a group or a channel belongs
/// to that group's conversation, not to theirs (design 06 v0.6), so every
/// person query here carries this condition.
const ONE_TO_ONE: &str =
    "thread_id NOT IN (SELECT id FROM thread WHERE kind IN ('group_chat', 'channel'))";

/// What the user has said about a person.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Relationship {
    /// Roles the user gave: colleague, family, client, ...
    pub roles: Vec<String>,
    /// Free text the user wrote.
    pub notes: String,
}

/// One person in the list, with enough to sort and glance.
#[derive(Clone, Debug)]
pub struct PersonOverview {
    /// Who.
    pub person: Person,
    /// Messages they wrote (to the user, or in a shared conversation).
    pub from_them: u64,
    /// Messages the user wrote to them.
    pub to_them: u64,
    /// The most recent message either way.
    pub last_at: Option<DateTime<Utc>>,
    /// Where they are known from.
    pub connectors: Vec<String>,
}

/// The arithmetic over one relationship.
#[derive(Clone, Debug, Default)]
pub struct RelationshipStats {
    /// Messages they wrote.
    pub from_them: u64,
    /// Messages the user wrote to them.
    pub to_them: u64,
    /// First and last message either way.
    pub first_at: Option<DateTime<Utc>>,
    /// See `first_at`.
    pub last_at: Option<DateTime<Utc>>,
    /// Messages per month, the last twelve months, oldest first.
    pub months: Vec<u32>,
    /// The user's typical time to answer them, in hours, when there were
    /// exchanges to measure.
    pub reply_hours: Option<f64>,
    /// The language their messages are mostly in: `zh`, `en`, or `mixed`.
    pub language: String,
    /// Messages per connector.
    pub connectors: BTreeMap<String, u64>,
}

/// Enough of an item to compute the statistics without the text.
struct Touch {
    at: DateTime<Utc>,
    from_them: bool,
    thread: String,
    connector: String,
    cjk: u32,
    letters: u32,
}

impl Store {
    /// Everyone but the user, most recently heard from first.
    ///
    /// Two passes over the items, not one per person: what each author
    /// wrote, and whom the user wrote to. A person a group chat brought in
    /// once is still a person here; the caller decides how many to show.
    pub fn people_overview(&self, limit: u32) -> Result<Vec<PersonOverview>> {
        struct Agg {
            from_them: u64,
            to_them: u64,
            last_at: Option<DateTime<Utc>>,
            connectors: std::collections::BTreeSet<String>,
        }
        let me = self.self_person()?.map(|p| p.id);
        let conn = self.conn();
        let mut agg: BTreeMap<String, Agg> = BTreeMap::new();

        // Counts and the newest, from `item_author_live` alone. Ordering on
        // occurred_ms rather than the RFC 3339 text keeps it in the index.
        let mut by_author = conn.prepare(&format!(
            "SELECT author, count(*), max(occurred_ms)
                 FROM item WHERE tombstoned = 0 AND author IS NOT NULL AND {ONE_TO_ONE}
                 GROUP BY author"
        ))?;
        for row in by_author.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        })? {
            let (author, count, last) = row?;
            let entry = agg.entry(author).or_insert(Agg {
                from_them: 0,
                to_them: 0,
                last_at: None,
                connectors: std::collections::BTreeSet::new(),
            });
            entry.from_them = u64::try_from(count).unwrap_or(0);
            entry.last_at = last.and_then(DateTime::from_timestamp_millis);
        }
        drop(by_author);

        // Which connectors each person has been seen on, from
        // `item_author_conn`. As its own pass because asking for it in the
        // one above costs a temporary b-tree for every person.
        let mut by_connector = conn.prepare(&format!(
            "SELECT DISTINCT author, connector
                 FROM item WHERE tombstoned = 0 AND author IS NOT NULL AND {ONE_TO_ONE}"
        ))?;
        for row in
            by_connector.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        {
            let (author, connector) = row?;
            if let Some(entry) = agg.get_mut(&author) {
                entry.connectors.insert(connector);
            }
        }
        drop(by_connector);

        if let Some(me) = me {
            let mut outbound = conn.prepare(&format!(
                "SELECT recipients, occurred_at FROM item
                     WHERE tombstoned = 0 AND author = ?1 AND {ONE_TO_ONE}"
            ))?;
            for row in outbound.query_map(params![me.to_string()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })? {
                let (recipients, at) = row?;
                let at = DateTime::parse_from_rfc3339(&at)
                    .ok()
                    .map(|d| d.with_timezone(&Utc));
                let ids: Vec<String> = serde_json::from_str(&recipients).unwrap_or_default();
                for id in ids {
                    let entry = agg.entry(id).or_insert(Agg {
                        from_them: 0,
                        to_them: 0,
                        last_at: None,
                        connectors: std::collections::BTreeSet::new(),
                    });
                    entry.to_them += 1;
                    if at > entry.last_at {
                        entry.last_at = at;
                    }
                }
            }
        }

        // The connection lock is not reentrant: release it before asking the
        // store anything else.
        drop(conn);
        let mut out = Vec::new();
        for person in self.all_persons()? {
            if person.is_self {
                continue;
            }
            let Some(a) = agg.get(&person.id.to_string()) else {
                continue;
            };
            out.push(PersonOverview {
                person,
                from_them: a.from_them,
                to_them: a.to_them,
                last_at: a.last_at,
                connectors: a.connectors.iter().cloned().collect(),
            });
        }
        out.sort_by_key(|o| std::cmp::Reverse(o.last_at));
        out.truncate(limit as usize);
        Ok(out)
    }

    /// The roles for several people at once, keyed by person. Same reason
    /// as [`Store::handles_for`]: one question instead of one per card.
    pub fn roles_for(&self, people: &[PersonId]) -> Result<BTreeMap<PersonId, Vec<String>>> {
        let mut out: BTreeMap<PersonId, Vec<String>> = BTreeMap::new();
        if people.is_empty() {
            return Ok(out);
        }
        let list = vec!["?"; people.len()].join(",");
        let ids: Vec<String> = people.iter().map(ToString::to_string).collect();
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT person_id, roles FROM relationship WHERE person_id IN ({list})"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, roles) = row?;
            let Ok(id) = id.parse::<PersonId>() else {
                continue;
            };
            out.insert(id, serde_json::from_str(&roles).unwrap_or_default());
        }
        Ok(out)
    }

    /// The arithmetic over everything between the user and one person.
    pub fn relationship_stats(&self, person: PersonId) -> Result<RelationshipStats> {
        let me = self
            .self_person()?
            .map(|p| p.id.to_string())
            .unwrap_or_default();
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT occurred_at, author, thread_id, connector, substr(text, 1, 400)
             FROM item
             WHERE tombstoned = 0 AND {ONE_TO_ONE}
               AND (author = ?1 OR (author = ?2 AND recipients LIKE ?3))
             ORDER BY occurred_ms"
        ))?;
        let pid = person.to_string();
        let touches: Vec<Touch> = stmt
            .query_map(params![pid, me, format!("%\"{pid}\"%")], |r| {
                let at: String = r.get(0)?;
                let author: Option<String> = r.get(1)?;
                let text: String = r.get(4)?;
                let (cjk, letters) = text.chars().fold((0u32, 0u32), |(c, l), ch| {
                    if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
                        (c + 1, l)
                    } else if ch.is_alphabetic() {
                        (c, l + 1)
                    } else {
                        (c, l)
                    }
                });
                Ok(Touch {
                    at: DateTime::parse_from_rfc3339(&at)
                        .map_or_else(|_| Utc::now(), |d| d.with_timezone(&Utc)),
                    from_them: author.as_deref() == Some(pid.as_str()),
                    thread: r.get(2)?,
                    connector: r.get(3)?,
                    cjk,
                    letters,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        drop(stmt);

        let mut stats = RelationshipStats::default();
        let now = Utc::now();
        let mut months = vec![0u32; 12];
        let (mut cjk, mut letters) = (0u64, 0u64);
        let mut last_from_them: BTreeMap<String, DateTime<Utc>> = BTreeMap::new();
        let mut replies: Vec<f64> = Vec::new();
        for t in &touches {
            if t.from_them {
                stats.from_them += 1;
                cjk += u64::from(t.cjk);
                letters += u64::from(t.letters);
                *stats.connectors.entry(t.connector.clone()).or_default() += 1;
                last_from_them.insert(t.thread.clone(), t.at);
            } else {
                stats.to_them += 1;
                if let Some(theirs) = last_from_them.remove(&t.thread) {
                    // Whole minutes, then hours: a month is well within
                    // what fits exactly.
                    let minutes = i32::try_from((t.at - theirs).num_minutes()).unwrap_or(i32::MAX);
                    let hours = f64::from(minutes) / 60.0;
                    if (0.0..24.0 * 30.0).contains(&hours) {
                        replies.push(hours);
                    }
                }
            }
            stats.first_at.get_or_insert(t.at);
            stats.last_at = Some(t.at);
            let months_ago = (now.year() - t.at.year()) * 12
                + (i32::try_from(now.month()).unwrap_or(1)
                    - i32::try_from(t.at.month()).unwrap_or(1));
            if (0..12).contains(&months_ago) {
                months[11 - usize::try_from(months_ago).unwrap_or(0)] += 1;
            }
        }
        stats.months = months;
        if !replies.is_empty() {
            replies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            stats.reply_hours = Some(replies[replies.len() / 2]);
        }
        stats.language = match (cjk, letters) {
            (0, 0) => String::new(),
            (c, l) if c * 3 > l => "zh".into(),
            (c, l) if l > c * 10 => "en".into(),
            _ => "mixed".into(),
        };
        Ok(stats)
    }

    /// Recent items between the user and a person, newest first.
    pub fn items_with_person(&self, person: PersonId, limit: u32) -> Result<Vec<Item>> {
        self.items_with_person_before(person, None, limit)
    }

    /// The same, older than an instant: how a conversation pages back.
    /// Newest first; the caller turns it round for display.
    pub fn items_with_person_before(
        &self,
        person: PersonId,
        before_ms: Option<i64>,
        limit: u32,
    ) -> Result<Vec<Item>> {
        let me = self
            .self_person()?
            .map(|p| p.id.to_string())
            .unwrap_or_default();
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT i.* FROM item i
             WHERE i.tombstoned = 0 AND i.{ONE_TO_ONE}
               AND NOT EXISTS (SELECT 1 FROM item n WHERE n.supersedes = i.id)
               AND (i.author = ?1 OR (i.author = ?2 AND i.recipients LIKE ?3))
               AND i.occurred_ms < ?5
             ORDER BY i.occurred_ms DESC LIMIT ?4"
        ))?;
        let pid = person.to_string();
        let mut rows = stmt.query(params![
            pid,
            me,
            format!("%\"{pid}\"%"),
            limit,
            before_ms.unwrap_or(i64::MAX)
        ])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(row_to_item(r)?);
        }
        Ok(out)
    }

    /// The newest thing each of several people wrote, keyed by person: the
    /// line under a name in a list of conversations. Answered from
    /// `item_author_live`, one lookup per person inside one statement.
    pub fn last_words(&self, people: &[PersonId]) -> Result<BTreeMap<PersonId, String>> {
        let mut out = BTreeMap::new();
        if people.is_empty() {
            return Ok(out);
        }
        let list = vec!["?"; people.len()].join(",");
        let ids: Vec<String> = people.iter().map(ToString::to_string).collect();
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT i.author, substr(i.text, 1, 160) FROM item i
             WHERE i.tombstoned = 0 AND i.author IN ({list}) AND i.{ONE_TO_ONE}
               AND i.occurred_ms = (SELECT max(j.occurred_ms) FROM item j
                                    WHERE j.author = i.author AND j.tombstoned = 0
                                      AND j.{ONE_TO_ONE})"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, text) = row?;
            if let Ok(id) = id.parse::<PersonId>() {
                out.entry(id).or_insert(text);
            }
        }
        Ok(out)
    }

    /// Commitments either way between the user and a person.
    pub fn commitments_with(&self, person: PersonId) -> Result<Vec<Commitment>> {
        let pid = person.to_string();
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT * FROM commitment WHERE from_person = ?1 OR to_person = ?1
             ORDER BY status IN ('open', 'overdue') DESC, due IS NULL, due, created_at DESC",
        )?;
        let rows = stmt.query_map(params![pid], |r| {
            Ok(crate::commitment::row_to_commitment(r))
        })?;
        rows.map(|r| r?).collect()
    }

    /// What the user said about a person, if anything.
    pub fn get_relationship(&self, person: PersonId) -> Result<Option<Relationship>> {
        let row: Option<(String, String)> = self
            .conn()
            .query_row(
                "SELECT roles, notes FROM relationship WHERE person_id = ?1",
                params![person.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(roles, notes)| {
            Ok(Relationship {
                roles: serde_json::from_str(&roles)
                    .map_err(|e| crate::error::corrupt("relationship", "roles", e))?,
                notes,
            })
        })
        .transpose()
    }

    /// Set roles and notes for a person, replacing what was there.
    pub fn set_relationship(&self, person: PersonId, relationship: &Relationship) -> Result<()> {
        self.conn().execute(
            "INSERT INTO relationship (person_id, roles, notes, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (person_id) DO UPDATE SET roles = excluded.roles, notes = excluded.notes,
                 updated_at = excluded.updated_at",
            params![
                person.to_string(),
                serde_json::to_string(&relationship.roles)?,
                relationship.notes,
                utc_to_col(Utc::now()),
            ],
        )?;
        let _ = utc_from_col;
        Ok(())
    }
}
