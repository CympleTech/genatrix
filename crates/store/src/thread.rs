//! Threads.

use genatrix_model::{PersonId, Source, Thread, ThreadId, ThreadKind};
use rusqlite::{OptionalExtension, Row, params};

use crate::error::{Result, corrupt};
use crate::raw::connector_from_col;
use crate::store::Store;
use crate::time::{opt_utc_from_col, opt_utc_to_col};

const T: &str = "thread";

fn kind_to_col(k: ThreadKind) -> &'static str {
    match k {
        ThreadKind::MailThread => "mail_thread",
        ThreadKind::DirectChat => "direct_chat",
        ThreadKind::GroupChat => "group_chat",
        ThreadKind::Channel => "channel",
        ThreadKind::Calendar => "calendar",
        ThreadKind::Notebook => "notebook",
        ThreadKind::Folder => "folder",
    }
}

fn kind_from_col(s: &str) -> Result<ThreadKind> {
    Ok(match s {
        "mail_thread" => ThreadKind::MailThread,
        "direct_chat" => ThreadKind::DirectChat,
        "group_chat" => ThreadKind::GroupChat,
        "channel" => ThreadKind::Channel,
        "calendar" => ThreadKind::Calendar,
        "notebook" => ThreadKind::Notebook,
        "folder" => ThreadKind::Folder,
        other => return Err(corrupt(T, "kind", other)),
    })
}

pub(crate) fn row_to_thread(r: &Row<'_>) -> Result<Thread> {
    let id: String = r.get("id")?;
    let kind: String = r.get("kind")?;
    let connector: String = r.get("connector")?;
    let members: String = r.get("members")?;
    let members: Vec<PersonId> =
        serde_json::from_str(&members).map_err(|e| corrupt(T, "members", e))?;
    Ok(Thread {
        id: id.parse().map_err(|e| corrupt(T, "id", e))?,
        kind: kind_from_col(&kind)?,
        source: Source::new(
            connector_from_col(T, &connector)?,
            r.get::<_, String>("account")?,
            r.get::<_, String>("external_id")?,
        ),
        title: r.get("title")?,
        members,
        first_at: opt_utc_from_col(T, "first_at", r.get("first_at")?)?,
        last_at: opt_utc_from_col(T, "last_at", r.get("last_at")?)?,
    })
}

impl Store {
    /// Insert a thread, or update title, members and bounds if the same
    /// source container is already known. Returns the stored id, which may
    /// differ from `thread.id` when the container already existed.
    pub fn upsert_thread(&self, thread: &Thread) -> Result<ThreadId> {
        self.tx(|tx| {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT id FROM thread WHERE connector = ?1 AND account = ?2 AND external_id = ?3",
                    params![
                        thread.source.connector.as_str(),
                        thread.source.account,
                        thread.source.external_id
                    ],
                    |r| r.get(0),
                )
                .optional()?;
            let members = serde_json::to_string(&thread.members)?;
            if let Some(id) = existing {
                tx.execute(
                    "UPDATE thread SET title = ?2, members = ?3,
                        first_at = coalesce(min(first_at, ?4), ?4),
                        last_at  = coalesce(max(last_at, ?5), ?5)
                     WHERE id = ?1",
                    params![
                        id,
                        thread.title,
                        members,
                        opt_utc_to_col(thread.first_at),
                        opt_utc_to_col(thread.last_at)
                    ],
                )?;
                return id.parse().map_err(|e| corrupt(T, "id", e));
            }
            tx.execute(
                "INSERT INTO thread
                    (id, kind, connector, account, external_id, title, members, first_at, last_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    thread.id.to_string(),
                    kind_to_col(thread.kind),
                    thread.source.connector.as_str(),
                    thread.source.account,
                    thread.source.external_id,
                    thread.title,
                    members,
                    opt_utc_to_col(thread.first_at),
                    opt_utc_to_col(thread.last_at)
                ],
            )?;
            Ok(thread.id)
        })
    }

    /// Fetch a thread by id.
    pub fn get_thread(&self, id: ThreadId) -> Result<Option<Thread>> {
        self.conn()
            .query_row(
                "SELECT * FROM thread WHERE id = ?1",
                [id.to_string()],
                |r| Ok(row_to_thread(r)),
            )
            .optional()?
            .transpose()
    }

    /// Several threads at once, keyed by id. A conversation shows forty
    /// messages from a dozen threads; asking per message is forty locks.
    pub fn threads_by_id(
        &self,
        ids: &[ThreadId],
    ) -> Result<std::collections::BTreeMap<ThreadId, Thread>> {
        let mut out = std::collections::BTreeMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let list = vec!["?"; ids.len()].join(",");
        let keys: Vec<String> = ids.iter().map(ToString::to_string).collect();
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!("SELECT * FROM thread WHERE id IN ({list})"))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(keys.iter()), |r| {
            Ok(row_to_thread(r))
        })?;
        for row in rows {
            let thread = row??;
            out.insert(thread.id, thread);
        }
        Ok(out)
    }

    /// Find a thread by its source container.
    pub fn find_thread(&self, source: &Source) -> Result<Option<Thread>> {
        self.conn()
            .query_row(
                "SELECT * FROM thread WHERE connector = ?1 AND account = ?2 AND external_id = ?3",
                params![
                    source.connector.as_str(),
                    source.account,
                    source.external_id
                ],
                |r| Ok(row_to_thread(r)),
            )
            .optional()?
            .transpose()
    }

    /// All threads. Used by export.
    pub fn all_threads(&self) -> Result<Vec<Thread>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM thread ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok(row_to_thread(r)))?;
        rows.map(|r| r?).collect()
    }
}
