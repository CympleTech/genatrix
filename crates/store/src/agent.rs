//! Installed functional agents, the versions the user approved, and their
//! runs (design 11). What an agent keeps for itself is in its own space
//! file, not here.

use genatrix_model::Level;
use rusqlite::{OptionalExtension, params};

use crate::error::Result;
use crate::item::level_from_col;
use crate::store::Store;

/// Whether an agent is woken by its triggers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    /// Runs when its triggers fire.
    Active,
    /// Installed, not woken.
    Paused,
}

impl AgentState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
        }
    }
}

/// One installed agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredAgent {
    /// Made at install; survives renames and upgrades.
    pub id: String,
    /// The manifest's name, for display.
    pub name: String,
    /// Hash of the approved package that runs now.
    pub version: String,
    /// Active or paused.
    pub state: AgentState,
    /// The level of everything it has read, and so of its space.
    pub space_level: Level,
    /// When it was installed.
    pub installed_ms: i64,
}

/// One run, as recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRun {
    /// The run's id.
    pub id: String,
    /// Which agent.
    pub agent_id: String,
    /// Which version ran.
    pub version: String,
    /// When it started.
    pub started_ms: i64,
    /// `items`, `message`, `schedule`, `apply`.
    pub invocation: String,
    /// The user's words, for a message; the schedule or kind otherwise.
    pub input: Option<String>,
    /// `ok`, `refused`, `limit`, `trap`, `agent`.
    pub outcome: String,
    /// Why it did not end well, if it did not.
    pub detail: Option<String>,
    /// Its answer to a message.
    pub answer: Option<String>,
    /// The highest level of anything it read.
    pub level: Level,
    /// Item ids it read.
    pub reads: Vec<String>,
    /// Actions it proposed.
    pub proposals: Vec<String>,
    /// What it logged.
    pub log: Vec<String>,
    /// Instructions spent.
    pub fuel: u64,
}

const T: &str = "agent";

fn row_to_agent(r: &rusqlite::Row<'_>) -> rusqlite::Result<(StoredAgent, String, String)> {
    let state: String = r.get("state")?;
    let level: String = r.get("space_level")?;
    Ok((
        StoredAgent {
            id: r.get("id")?,
            name: r.get("name")?,
            version: r.get("version")?,
            state: if state == "paused" {
                AgentState::Paused
            } else {
                AgentState::Active
            },
            space_level: Level::Public,
            installed_ms: r.get("installed_ms")?,
        },
        state,
        level,
    ))
}

fn finish(row: (StoredAgent, String, String)) -> Result<StoredAgent> {
    let (mut agent, _, level) = row;
    agent.space_level = level_from_col(T, &level)?;
    Ok(agent)
}

impl Store {
    /// Record a newly installed agent and the version the user approved.
    pub fn insert_agent(&self, agent: &StoredAgent, manifest: &str) -> Result<()> {
        self.tx(|tx| {
            tx.execute(
                "INSERT INTO agent (id, name, version, state, space_level, installed_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    agent.id,
                    agent.name,
                    agent.version,
                    agent.state.as_str(),
                    agent.space_level.as_str(),
                    agent.installed_ms
                ],
            )?;
            tx.execute(
                "INSERT INTO agent_version (agent_id, hash, manifest, approved_ms)
                 VALUES (?1, ?2, ?3, ?4)",
                params![agent.id, agent.version, manifest, agent.installed_ms],
            )?;
            Ok(())
        })
    }

    /// One agent.
    pub fn get_agent(&self, id: &str) -> Result<Option<StoredAgent>> {
        let row = self
            .conn()
            .query_row("SELECT * FROM agent WHERE id = ?1", [id], row_to_agent)
            .optional()?;
        row.map(finish).transpose()
    }

    /// Every installed agent, by name.
    pub fn all_agents(&self) -> Result<Vec<StoredAgent>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM agent ORDER BY name, id")?;
        let rows = stmt.query_map([], row_to_agent)?;
        rows.map(|r| finish(r?)).collect()
    }

    /// Whether the user approved this version of this agent.
    pub fn agent_version_approved(&self, agent_id: &str, hash: &str) -> Result<bool> {
        let n: i64 = self.conn().query_row(
            "SELECT count(*) FROM agent_version WHERE agent_id = ?1 AND hash = ?2",
            params![agent_id, hash],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Wake or pause an agent.
    pub fn set_agent_state(&self, id: &str, state: AgentState) -> Result<()> {
        self.conn().execute(
            "UPDATE agent SET state = ?2 WHERE id = ?1",
            params![id, state.as_str()],
        )?;
        Ok(())
    }

    /// Raise an agent's space to at least this level. Never lowers it: what
    /// the space holds was derived from what the agent read.
    pub fn raise_space_level(&self, id: &str, level: Level) -> Result<Level> {
        self.tx(|tx| {
            let now: String =
                tx.query_row("SELECT space_level FROM agent WHERE id = ?1", [id], |r| {
                    r.get(0)
                })?;
            let raised = level_from_col(T, &now)?.max(level);
            tx.execute(
                "UPDATE agent SET space_level = ?2 WHERE id = ?1",
                params![id, raised.as_str()],
            )?;
            Ok(raised)
        })
    }

    /// Record a run.
    pub fn insert_agent_run(&self, run: &AgentRun) -> Result<()> {
        self.conn().execute(
            "INSERT INTO agent_run (id, agent_id, version, started_ms, invocation, input,
                outcome, detail, answer, level, reads, proposals, log, fuel)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                run.id,
                run.agent_id,
                run.version,
                run.started_ms,
                run.invocation,
                run.input,
                run.outcome,
                run.detail,
                run.answer,
                run.level.as_str(),
                serde_json::to_string(&run.reads)?,
                serde_json::to_string(&run.proposals)?,
                serde_json::to_string(&run.log)?,
                i64::try_from(run.fuel).unwrap_or(i64::MAX),
            ],
        )?;
        Ok(())
    }

    /// An agent's most recent runs, newest first.
    pub fn agent_runs(&self, agent_id: &str, limit: u32) -> Result<Vec<AgentRun>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT * FROM agent_run WHERE agent_id = ?1 ORDER BY started_ms DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![agent_id, limit], |r| {
            Ok((
                AgentRun {
                    id: r.get("id")?,
                    agent_id: r.get("agent_id")?,
                    version: r.get("version")?,
                    started_ms: r.get("started_ms")?,
                    invocation: r.get("invocation")?,
                    input: r.get("input")?,
                    outcome: r.get("outcome")?,
                    detail: r.get("detail")?,
                    answer: r.get("answer")?,
                    level: Level::Public,
                    reads: Vec::new(),
                    proposals: Vec::new(),
                    log: Vec::new(),
                    fuel: u64::try_from(r.get::<_, i64>("fuel")?).unwrap_or(0),
                },
                r.get::<_, String>("level")?,
                r.get::<_, String>("reads")?,
                r.get::<_, String>("proposals")?,
                r.get::<_, String>("log")?,
            ))
        })?;
        rows.map(|row| {
            let (mut run, level, reads, proposals, log) = row?;
            run.level = level_from_col("agent_run", &level)?;
            run.reads = serde_json::from_str(&reads)?;
            run.proposals = serde_json::from_str(&proposals)?;
            run.log = serde_json::from_str(&log)?;
            Ok(run)
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use genatrix_keys::DbKey;

    fn agent() -> StoredAgent {
        StoredAgent {
            id: "a1".into(),
            name: "Hello".into(),
            version: "h1".into(),
            state: AgentState::Active,
            space_level: Level::Public,
            installed_ms: 5,
        }
    }

    #[test]
    fn installed_agents_and_their_approved_versions() {
        let s = Store::open_in_memory(&DbKey::from_bytes([1; 32])).unwrap();
        s.insert_agent(&agent(), "name = \"Hello\"").unwrap();
        assert_eq!(s.get_agent("a1").unwrap(), Some(agent()));
        assert!(s.agent_version_approved("a1", "h1").unwrap());
        assert!(!s.agent_version_approved("a1", "h2").unwrap());
        s.set_agent_state("a1", AgentState::Paused).unwrap();
        assert_eq!(s.all_agents().unwrap()[0].state, AgentState::Paused);
    }

    #[test]
    fn the_space_level_only_rises() {
        let s = Store::open_in_memory(&DbKey::from_bytes([1; 32])).unwrap();
        s.insert_agent(&agent(), "").unwrap();
        assert_eq!(
            s.raise_space_level("a1", Level::Secret).unwrap(),
            Level::Secret
        );
        assert_eq!(
            s.raise_space_level("a1", Level::Public).unwrap(),
            Level::Secret
        );
        assert_eq!(
            s.get_agent("a1").unwrap().unwrap().space_level,
            Level::Secret
        );
    }

    #[test]
    fn runs_are_kept_newest_first() {
        let s = Store::open_in_memory(&DbKey::from_bytes([1; 32])).unwrap();
        s.insert_agent(&agent(), "").unwrap();
        for (id, at) in [("r1", 1), ("r2", 2)] {
            s.insert_agent_run(&AgentRun {
                id: id.into(),
                agent_id: "a1".into(),
                version: "h1".into(),
                started_ms: at,
                invocation: "items".into(),
                input: None,
                outcome: "ok".into(),
                detail: None,
                answer: None,
                level: Level::Personal,
                reads: vec!["i1".into()],
                proposals: Vec::new(),
                log: vec!["seen".into()],
                fuel: 10,
            })
            .unwrap();
        }
        let runs = s.agent_runs("a1", 10).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, "r2");
        assert_eq!(runs[0].reads, vec!["i1".to_owned()]);
        assert_eq!(runs[0].level, Level::Personal);
    }
}
