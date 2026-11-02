//! Definitions and dispatches for executors (design 13).
//!
//! The store keeps definitions as the text they were written in and the
//! dispatch record as plain rows; the meaning of both lives above, in
//! `genatrix-dispatch` and the daemon.

use rusqlite::{OptionalExtension, params};

use crate::error::Result;
use crate::store::Store;

/// A project, role or recipe as filed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    /// `project`, `role` or `recipe`.
    pub kind: String,
    /// Its name.
    pub name: String,
    /// SHA-256 of the text.
    pub hash: String,
    /// The TOML.
    pub text: String,
    /// When it was filed.
    pub added_ms: i64,
    /// Whether it is the one in use under its name.
    pub active: bool,
}

/// One dispatch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchRow {
    /// The approved action's id.
    pub id: String,
    /// Project name.
    pub project: String,
    /// Recipe name.
    pub recipe: String,
    /// The recipe's hash, as approved.
    pub recipe_hash: String,
    /// `running`, `finished`, `stopped`, `failed`.
    pub state: String,
    /// The level of the work.
    pub level: String,
    /// The branch the work is on.
    pub branch: String,
    /// The commit it started from.
    pub base: Option<String>,
    /// The newest commit it produced.
    pub tip: Option<String>,
    /// Whether the last review passed, for a recipe with a review.
    pub review_passed: Option<bool>,
    /// Why it stopped or failed.
    pub detail: Option<String>,
    /// Started.
    pub started_ms: i64,
    /// Ended.
    pub ended_ms: Option<i64>,
}

/// One step of a dispatch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DispatchStep {
    /// The dispatch.
    pub dispatch_id: String,
    /// Its place, from 1.
    pub seq: i64,
    /// The recipe step.
    pub step: String,
    /// Which round of the loop.
    pub round: i64,
    /// The role.
    pub role: String,
    /// The executor.
    pub executor: String,
    /// `running`, `done`, `failed`, `refused`, `cancelled`; `waiting` when
    /// it ended with a question for the user, `answered` once they answered.
    pub state: String,
    /// The egress record of the text it was given.
    pub egress_id: Option<String>,
    /// ACP's stop reason.
    pub stop: Option<String>,
    /// What the executor said, joined.
    pub answer: Option<String>,
    /// The verdict, as JSON, for a verdict step.
    pub verdict: Option<String>,
    /// The commit made from what it left, for a commits step.
    pub commit_id: Option<String>,
    /// What went wrong.
    pub error: Option<String>,
    /// Started.
    pub started_ms: i64,
    /// Ended.
    pub ended_ms: Option<i64>,
    /// The user's answer to the question the step ended with.
    pub reply: Option<String>,
    /// The level that answer leaves at: `public` or `personal`.
    pub reply_level: Option<String>,
}

/// One thing an executor said or did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchEvent {
    /// The step's place.
    pub seq: i64,
    /// Order within the step.
    pub n: i64,
    /// When.
    pub at_ms: i64,
    /// `message`, `thought`, `tool`, `tool_update`, `plan`, `asked`.
    pub kind: String,
    /// The text.
    pub text: String,
}

fn definition_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Definition> {
    Ok(Definition {
        kind: r.get(0)?,
        name: r.get(1)?,
        hash: r.get(2)?,
        text: r.get(3)?,
        added_ms: r.get(4)?,
        active: r.get::<_, i64>(5)? != 0,
    })
}

const DISPATCH_COLUMNS: &str = "id, project, recipe, recipe_hash, state, level, branch, base, tip, \
                                review_passed, detail, started_ms, ended_ms";

fn dispatch_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DispatchRow> {
    Ok(DispatchRow {
        id: r.get(0)?,
        project: r.get(1)?,
        recipe: r.get(2)?,
        recipe_hash: r.get(3)?,
        state: r.get(4)?,
        level: r.get(5)?,
        branch: r.get(6)?,
        base: r.get(7)?,
        tip: r.get(8)?,
        review_passed: r.get::<_, Option<i64>>(9)?.map(|v| v != 0),
        detail: r.get(10)?,
        started_ms: r.get(11)?,
        ended_ms: r.get(12)?,
    })
}

const STEP_COLUMNS: &str = "dispatch_id, seq, step, round, role, executor, state, egress_id, stop, \
                            answer, verdict, commit_id, error, started_ms, ended_ms, reply, \
                            reply_level";

fn step_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DispatchStep> {
    Ok(DispatchStep {
        dispatch_id: r.get(0)?,
        seq: r.get(1)?,
        step: r.get(2)?,
        round: r.get(3)?,
        role: r.get(4)?,
        executor: r.get(5)?,
        state: r.get(6)?,
        egress_id: r.get(7)?,
        stop: r.get(8)?,
        answer: r.get(9)?,
        verdict: r.get(10)?,
        commit_id: r.get(11)?,
        error: r.get(12)?,
        started_ms: r.get(13)?,
        ended_ms: r.get(14)?,
        reply: r.get(15)?,
        reply_level: r.get(16)?,
    })
}

impl Store {
    /// File a definition and make it the active one under its name. Filing
    /// the same text again only makes it active again.
    pub fn put_definition(
        &self,
        kind: &str,
        name: &str,
        hash: &str,
        text: &str,
        now_ms: i64,
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE definition SET active = 0 WHERE kind = ?1 AND name = ?2",
            params![kind, name],
        )?;
        tx.execute(
            "INSERT INTO definition (kind, name, hash, text, added_ms, active)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)
             ON CONFLICT (kind, hash) DO UPDATE SET active = 1",
            params![kind, name, hash, text, now_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Stop using a definition under its name. Its text stays, for the
    /// dispatches that named it.
    pub fn retire_definition(&self, kind: &str, name: &str) -> Result<bool> {
        let n = self.conn().execute(
            "UPDATE definition SET active = 0 WHERE kind = ?1 AND name = ?2 AND active = 1",
            params![kind, name],
        )?;
        Ok(n > 0)
    }

    /// The active definitions of a kind, by name.
    pub fn active_definitions(&self, kind: &str) -> Result<Vec<Definition>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT kind, name, hash, text, added_ms, active FROM definition
             WHERE kind = ?1 AND active = 1 ORDER BY name",
        )?;
        let rows = stmt.query_map(params![kind], definition_row)?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// One definition by hash, active or not.
    pub fn definition(&self, kind: &str, hash: &str) -> Result<Option<Definition>> {
        self.conn()
            .query_row(
                "SELECT kind, name, hash, text, added_ms, active FROM definition
                 WHERE kind = ?1 AND hash = ?2",
                params![kind, hash],
                definition_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Begin the record of a dispatch.
    pub fn insert_dispatch(&self, d: &DispatchRow) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT INTO dispatch ({DISPATCH_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
            ),
            params![
                d.id,
                d.project,
                d.recipe,
                d.recipe_hash,
                d.state,
                d.level,
                d.branch,
                d.base,
                d.tip,
                d.review_passed.map(i64::from),
                d.detail,
                d.started_ms,
                d.ended_ms
            ],
        )?;
        Ok(())
    }

    /// Write what changed on a dispatch.
    pub fn update_dispatch(&self, d: &DispatchRow) -> Result<()> {
        self.conn().execute(
            "UPDATE dispatch SET state = ?2, base = ?3, tip = ?4, review_passed = ?5, detail = ?6,
                    ended_ms = ?7, level = ?8
             WHERE id = ?1",
            params![
                d.id,
                d.state,
                d.base,
                d.tip,
                d.review_passed.map(i64::from),
                d.detail,
                d.ended_ms,
                d.level
            ],
        )?;
        Ok(())
    }

    /// One dispatch.
    pub fn get_dispatch(&self, id: &str) -> Result<Option<DispatchRow>> {
        self.conn()
            .query_row(
                &format!("SELECT {DISPATCH_COLUMNS} FROM dispatch WHERE id = ?1"),
                params![id],
                dispatch_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// The latest dispatches, for one project or all.
    pub fn list_dispatches(&self, project: Option<&str>, limit: u32) -> Result<Vec<DispatchRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {DISPATCH_COLUMNS} FROM dispatch
             WHERE (?1 IS NULL OR project = ?1) ORDER BY started_ms DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![project, limit], dispatch_row)?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// Dispatches left running, as a restart finds them.
    pub fn running_dispatches(&self) -> Result<Vec<DispatchRow>> {
        self.dispatches_in("running")
    }

    /// Dispatches in one state, oldest first.
    pub fn dispatches_in(&self, state: &str) -> Result<Vec<DispatchRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {DISPATCH_COLUMNS} FROM dispatch WHERE state = ?1 ORDER BY started_ms"
        ))?;
        let rows = stmt.query_map(params![state], dispatch_row)?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// Write a step, new or changed.
    pub fn put_dispatch_step(&self, s: &DispatchStep) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT INTO dispatch_step ({STEP_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                 ON CONFLICT (dispatch_id, seq) DO UPDATE SET
                    state = excluded.state, egress_id = excluded.egress_id, stop = excluded.stop,
                    answer = excluded.answer, verdict = excluded.verdict,
                    commit_id = excluded.commit_id, error = excluded.error,
                    ended_ms = excluded.ended_ms, reply = excluded.reply,
                    reply_level = excluded.reply_level"
            ),
            params![
                s.dispatch_id,
                s.seq,
                s.step,
                s.round,
                s.role,
                s.executor,
                s.state,
                s.egress_id,
                s.stop,
                s.answer,
                s.verdict,
                s.commit_id,
                s.error,
                s.started_ms,
                s.ended_ms,
                s.reply,
                s.reply_level
            ],
        )?;
        Ok(())
    }

    /// The steps of a dispatch, in order.
    pub fn dispatch_steps(&self, id: &str) -> Result<Vec<DispatchStep>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {STEP_COLUMNS} FROM dispatch_step WHERE dispatch_id = ?1 ORDER BY seq"
        ))?;
        let rows = stmt.query_map(params![id], step_row)?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// Add an event to a step.
    pub fn add_dispatch_event(
        &self,
        id: &str,
        seq: i64,
        at_ms: i64,
        kind: &str,
        text: &str,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT INTO dispatch_event (dispatch_id, seq, n, at_ms, kind, text)
             VALUES (?1, ?2,
                     (SELECT COALESCE(MAX(n), 0) + 1 FROM dispatch_event WHERE dispatch_id = ?1 AND seq = ?2),
                     ?3, ?4, ?5)",
            params![id, seq, at_ms, kind, text],
        )?;
        Ok(())
    }

    /// The events of a dispatch, after `(seq, n)` when given, oldest first.
    pub fn dispatch_events(
        &self,
        id: &str,
        after: Option<(i64, i64)>,
        limit: u32,
    ) -> Result<Vec<DispatchEvent>> {
        let (s, n) = after.unwrap_or((0, 0));
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT seq, n, at_ms, kind, text FROM dispatch_event
             WHERE dispatch_id = ?1 AND (seq > ?2 OR (seq = ?2 AND n > ?3))
             ORDER BY seq, n LIMIT ?4",
        )?;
        let rows = stmt.query_map(params![id, s, n, limit], |r| {
            Ok(DispatchEvent {
                seq: r.get(0)?,
                n: r.get(1)?,
                at_ms: r.get(2)?,
                kind: r.get(3)?,
                text: r.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;
    use crate::{DispatchRow, DispatchStep};

    fn store() -> Store {
        Store::open_in_memory(&genatrix_keys::DbKey::from_bytes([3; 32])).unwrap()
    }

    #[test]
    fn a_changed_definition_is_a_new_row_and_the_old_one_stays() {
        let s = store();
        s.put_definition("recipe", "r", "h1", "one", 1).unwrap();
        s.put_definition("recipe", "r", "h2", "two", 2).unwrap();
        let active = s.active_definitions("recipe").unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].hash, "h2");
        let old = s.definition("recipe", "h1").unwrap().unwrap();
        assert_eq!(old.text, "one");
        assert!(!old.active);
        assert!(s.retire_definition("recipe", "r").unwrap());
        assert!(s.active_definitions("recipe").unwrap().is_empty());
    }

    #[test]
    fn a_dispatch_keeps_its_steps_and_events_in_order() {
        let s = store();
        let mut d = DispatchRow {
            id: "d1".into(),
            project: "p".into(),
            recipe: "r".into(),
            recipe_hash: "h".into(),
            state: "running".into(),
            level: "public".into(),
            branch: "genatrix/d1".into(),
            base: None,
            tip: None,
            review_passed: None,
            detail: None,
            started_ms: 1,
            ended_ms: None,
        };
        s.insert_dispatch(&d).unwrap();
        assert_eq!(s.running_dispatches().unwrap().len(), 1);
        let mut step = DispatchStep {
            dispatch_id: "d1".into(),
            seq: 1,
            step: "implement".into(),
            round: 1,
            role: "backend".into(),
            executor: "claude-code".into(),
            state: "running".into(),
            started_ms: 2,
            ..Default::default()
        };
        s.put_dispatch_step(&step).unwrap();
        step.state = "waiting".into();
        s.put_dispatch_step(&step).unwrap();
        step.state = "answered".into();
        step.reply = Some("SQLite".into());
        step.reply_level = Some("public".into());
        s.put_dispatch_step(&step).unwrap();
        let back = &s.dispatch_steps("d1").unwrap()[0];
        assert_eq!(back.reply.as_deref(), Some("SQLite"));
        assert_eq!(back.reply_level.as_deref(), Some("public"));
        step.state = "done".into();
        step.commit_id = Some("abc".into());
        s.put_dispatch_step(&step).unwrap();
        for t in ["a", "b", "c"] {
            s.add_dispatch_event("d1", 1, 3, "message", t).unwrap();
        }
        let events = s.dispatch_events("d1", None, 10).unwrap();
        assert_eq!(
            events.iter().map(|e| e.n).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(s.dispatch_events("d1", Some((1, 2)), 10).unwrap().len(), 1);
        d.state = "finished".into();
        d.tip = Some("abc".into());
        d.review_passed = Some(true);
        s.update_dispatch(&d).unwrap();
        let back = s.get_dispatch("d1").unwrap().unwrap();
        assert_eq!(back.review_passed, Some(true));
        assert_eq!(
            s.dispatch_steps("d1").unwrap()[0].commit_id.as_deref(),
            Some("abc")
        );
        assert!(s.running_dispatches().unwrap().is_empty());
        assert_eq!(s.list_dispatches(Some("p"), 5).unwrap().len(), 1);
    }
}
