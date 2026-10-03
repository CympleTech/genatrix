//! Junk (design 01, "垃圾"): the parties the user does not want. Marking
//! one finds what to remove; the mark keeps what arrives later out.

use std::collections::HashSet;

use genatrix_model::{Handle, HandleKind, PersonId, Source, ThreadId};
use rusqlite::{OptionalExtension, params};

use crate::error::Result;
use crate::store::Store;

/// One party marked as junk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Junk {
    /// `person` or `thread`.
    pub kind: &'static str,
    /// The person's or the thread's id.
    pub id: String,
    /// Its name, as it was known.
    pub name: String,
    /// When it was marked.
    pub at: String,
}

impl Store {
    /// The items that make up a person's conversation with the user, every
    /// version: what they wrote outside groups and channels, and what was
    /// sent to them and nobody else but the user. Mail the user sent to
    /// several people at once, them among them, is not theirs alone and is
    /// not here.
    pub fn person_conversation_items(&self, person: PersonId) -> Result<HashSet<String>> {
        let me = self
            .self_person()?
            .map(|p| p.id.to_string())
            .unwrap_or_default();
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT i.id FROM item i
             WHERE i.thread_id NOT IN (SELECT id FROM thread WHERE kind IN ('group_chat', 'channel'))
               AND (i.author = ?1
                    OR (EXISTS (SELECT 1 FROM json_each(i.recipients) WHERE value = ?1)
                        AND NOT EXISTS (SELECT 1 FROM json_each(i.recipients)
                                        WHERE value NOT IN (?1, ?2))))",
        )?;
        let rows = stmt.query_map(params![person.to_string(), me], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Every item in a thread, every version.
    pub fn thread_items(&self, thread: ThreadId) -> Result<HashSet<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id FROM item WHERE thread_id = ?1")?;
        let rows = stmt.query_map([thread.to_string()], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Mark a person as junk, or unmark with `None`.
    pub fn set_person_junk(&self, person: PersonId, at: Option<&str>) -> Result<()> {
        self.conn().execute(
            "UPDATE person SET junk_at = ?2 WHERE id = ?1",
            params![person.to_string(), at],
        )?;
        Ok(())
    }

    /// Mark a group or channel as junk, or unmark with `None`.
    pub fn set_thread_junk(&self, thread: ThreadId, at: Option<&str>) -> Result<()> {
        self.conn().execute(
            "UPDATE thread SET junk_at = ?2 WHERE id = ?1",
            params![thread.to_string(), at],
        )?;
        Ok(())
    }

    /// Whether the person this handle belongs to is junk.
    pub fn is_junk_handle(&self, kind: HandleKind, value: &str) -> Result<bool> {
        let Some(handle) = self.find_handle(kind, &Handle::normalize(kind, value))? else {
            return Ok(false);
        };
        let at: Option<String> = self
            .conn()
            .query_row(
                "SELECT junk_at FROM person WHERE id = ?1",
                [handle.person_id.to_string()],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(at.is_some())
    }

    /// Whether the thread at this source is junk.
    pub fn is_junk_thread(&self, source: &Source) -> Result<bool> {
        let at: Option<String> = self
            .conn()
            .query_row(
                "SELECT junk_at FROM thread WHERE connector = ?1 AND account = ?2 AND external_id = ?3",
                params![source.connector.as_str(), source.account, source.external_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(at.is_some())
    }

    /// Everything marked as junk, newest first.
    pub fn junk(&self) -> Result<Vec<Junk>> {
        let conn = self.conn();
        let mut out = Vec::new();
        let mut stmt =
            conn.prepare("SELECT id, display_name, junk_at FROM person WHERE junk_at IS NOT NULL")?;
        for row in stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))? {
            let (id, name, at) = row?;
            out.push(Junk {
                kind: "person",
                id,
                name,
                at,
            });
        }
        let mut stmt = conn.prepare(
            "SELECT id, coalesce(title, ''), junk_at FROM thread WHERE junk_at IS NOT NULL",
        )?;
        for row in stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))? {
            let (id, name, at) = row?;
            out.push(Junk {
                kind: "thread",
                id,
                name,
                at,
            });
        }
        out.sort_by(|a, b| b.at.cmp(&a.at));
        Ok(out)
    }
}
