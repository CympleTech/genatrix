//! Groups and channels as conversations.
//!
//! Design: `docs/design/06-interface.md` v0.6, "对话". A multi-party
//! container is one party in the list of conversations, however many people
//! are in it; its members are not listed one by one. These are the
//! questions the Chats page asks about such a container: the list of them
//! with their size and last activity, the last thing said in each, the
//! conversation a page at a time, and who speaks most.

use std::collections::BTreeMap;

use genatrix_model::{Item, PersonId, Thread, ThreadId};
use rusqlite::params;

use crate::error::Result;
use crate::item::row_to_item;
use crate::store::Store;

/// One group or channel in the list.
#[derive(Clone, Debug)]
pub struct GroupOverview {
    /// The container.
    pub thread: Thread,
    /// Messages kept from it.
    pub messages: u64,
    /// Of those, the user's own.
    pub mine: u64,
    /// The first and the latest, in milliseconds.
    pub first_ms: i64,
    /// See `first_ms`.
    pub last_ms: i64,
}

/// The last thing said in a container: who, and the start of it.
#[derive(Clone, Debug)]
pub struct LastWord {
    /// Who said it, when known.
    pub author: Option<PersonId>,
    /// The start of it.
    pub text: String,
}

const MULTI: &str = "t.kind IN ('group_chat', 'channel')";

impl Store {
    /// Every group and channel with something kept from it, most recently
    /// active first. Answered from `item_thread` for the counts and times;
    /// the user's own share needs the author, so it reads the rows.
    pub fn group_overview(&self) -> Result<Vec<GroupOverview>> {
        let me = self
            .self_person()?
            .map(|p| p.id.to_string())
            .unwrap_or_default();
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT t.*, count(i.id) AS n_messages,
                    sum(CASE WHEN i.author = ?1 THEN 1 ELSE 0 END) AS n_mine,
                    min(i.occurred_ms) AS first_ms, max(i.occurred_ms) AS last_ms
             FROM thread t JOIN item i ON i.thread_id = t.id
             WHERE {MULTI} AND i.tombstoned = 0
             GROUP BY t.id
             ORDER BY last_ms DESC"
        ))?;
        let rows = stmt.query_map(params![me], |r| {
            Ok((
                crate::thread::row_to_thread(r),
                r.get::<_, i64>("n_messages")?,
                r.get::<_, i64>("n_mine")?,
                r.get::<_, i64>("first_ms")?,
                r.get::<_, i64>("last_ms")?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (thread, messages, mine, first_ms, last_ms) = row?;
            out.push(GroupOverview {
                thread: thread?,
                messages: u64::try_from(messages).unwrap_or(0),
                mine: u64::try_from(mine).unwrap_or(0),
                first_ms,
                last_ms,
            });
        }
        Ok(out)
    }

    /// The last thing said in each of several containers, keyed by thread.
    pub fn group_last_words(&self, threads: &[ThreadId]) -> Result<BTreeMap<ThreadId, LastWord>> {
        let mut out = BTreeMap::new();
        if threads.is_empty() {
            return Ok(out);
        }
        let list = vec!["?"; threads.len()].join(",");
        let ids: Vec<String> = threads.iter().map(ToString::to_string).collect();
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT i.thread_id, i.author, substr(i.text, 1, 160) FROM item i
             WHERE i.tombstoned = 0 AND i.thread_id IN ({list})
               AND i.occurred_ms = (SELECT max(j.occurred_ms) FROM item j
                                    WHERE j.thread_id = i.thread_id AND j.tombstoned = 0)"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (thread, author, text) = row?;
            let Ok(thread) = thread.parse::<ThreadId>() else {
                continue;
            };
            out.entry(thread).or_insert(LastWord {
                author: author.and_then(|a| a.parse().ok()),
                text,
            });
        }
        Ok(out)
    }

    /// A container's conversation, older than an instant, newest first.
    pub fn items_in_thread_before(
        &self,
        thread: ThreadId,
        before_ms: Option<i64>,
        limit: u32,
    ) -> Result<Vec<Item>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT i.* FROM item i
             WHERE i.thread_id = ?1 AND i.tombstoned = 0 AND i.occurred_ms < ?2
               AND NOT EXISTS (SELECT 1 FROM item n WHERE n.supersedes = i.id)
             ORDER BY i.occurred_ms DESC LIMIT ?3",
        )?;
        let mut rows = stmt.query(params![
            thread.to_string(),
            before_ms.unwrap_or(i64::MAX),
            limit
        ])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(row_to_item(r)?);
        }
        Ok(out)
    }

    /// How many different people have said something in a container. Not
    /// its membership: the connectors do not keep member lists, and a count
    /// of members that is really a count of one would be worse than none.
    pub fn voices(&self, thread: ThreadId) -> Result<u64> {
        let n: i64 = self.conn().query_row(
            "SELECT count(DISTINCT author) FROM item
             WHERE thread_id = ?1 AND tombstoned = 0 AND author IS NOT NULL",
            params![thread.to_string()],
            |r| r.get(0),
        )?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Who speaks most in a container, with how often.
    pub fn speakers(&self, thread: ThreadId, limit: u32) -> Result<Vec<(PersonId, u64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT author, count(*) FROM item
             WHERE thread_id = ?1 AND tombstoned = 0 AND author IS NOT NULL
             GROUP BY author ORDER BY 2 DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![thread.to_string(), limit], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (author, n) = row?;
            if let Ok(id) = author.parse() {
                out.push((id, u64::try_from(n).unwrap_or(0)));
            }
        }
        Ok(out)
    }
}
