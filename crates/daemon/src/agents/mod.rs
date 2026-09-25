//! Functional agents in the running core (design 11): install, verify,
//! run, record.
//!
//! The sandbox is `genatrix-host`; this module is the other side of its
//! doors. An agent never runs from a package whose hash the user did not
//! approve (design 02, invariant 17), its space is its own file with its
//! own key (15), and what it reads is what its manifest names (14).

mod doors;
pub mod life;
pub mod terminal;
pub mod words;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use genatrix_host::{Invocation, Outcome, Package, Run, RunError, Runner};
use genatrix_keys::MasterKey;
use genatrix_store::{AgentRun, AgentState, Space, StoredAgent};
use sha2::{Digest, Sha256};

use crate::system::System;

/// The agents side of the core: where packages and spaces live, and the
/// one sandbox they share.
pub struct Agents {
    dir: PathBuf,
    master: MasterKey,
    runner: OnceLock<Arc<Runner>>,
    spaces: Mutex<HashMap<String, Arc<Space>>>,
    /// One run at a time. Runs are short and rare; taking turns keeps a
    /// space from seeing two runs' statements interleaved.
    turn: tokio::sync::Mutex<()>,
    /// Packages uploaded from the page, by hash, waiting for their install.
    pub(crate) staged: Mutex<HashMap<String, (Instant, Vec<u8>)>>,
}

impl std::fmt::Debug for Agents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agents")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl Agents {
    /// Agents under `dir`, their spaces keyed from `master`.
    #[must_use]
    pub fn new(dir: PathBuf, master: MasterKey) -> Self {
        Self {
            dir,
            master,
            runner: OnceLock::new(),
            spaces: Mutex::new(HashMap::new()),
            turn: tokio::sync::Mutex::new(()),
            staged: Mutex::new(HashMap::new()),
        }
    }

    fn runner(&self) -> anyhow::Result<Arc<Runner>> {
        if let Some(r) = self.runner.get() {
            return Ok(Arc::clone(r));
        }
        let made = Arc::new(Runner::new()?);
        Ok(Arc::clone(self.runner.get_or_init(|| made)))
    }

    fn package_path(&self, agent_id: &str, hash: &str) -> PathBuf {
        self.dir.join(agent_id).join(format!("{hash}.wasm"))
    }

    /// Close an agent's space, so its file can go.
    fn forget_space(&self, agent_id: &str) {
        self.spaces
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(agent_id);
    }

    fn space(&self, agent_id: &str, quota_mb: u32) -> anyhow::Result<Arc<Space>> {
        let mut spaces = self
            .spaces
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(s) = spaces.get(agent_id) {
            return Ok(Arc::clone(s));
        }
        let dir = self.dir.join(agent_id);
        std::fs::create_dir_all(&dir)?;
        let space = Arc::new(Space::open(
            dir.join("space.db"),
            &self.master.space_key(agent_id),
            quota_mb,
        )?);
        spaces.insert(agent_id.to_owned(), Arc::clone(&space));
        Ok(space)
    }
}

/// Install a package the user approved: keep its bytes, record the agent
/// and this version as approved. The caller has shown the manifest and has
/// the user's word; this function does not ask again.
pub fn install(system: &System, bytes: Vec<u8>, now_ms: i64) -> anyhow::Result<StoredAgent> {
    let package = Package::read(bytes)?;
    let agent = StoredAgent {
        id: ulid::Ulid::new().to_string(),
        name: package.manifest.name.clone(),
        version: package.hash.clone(),
        state: AgentState::Active,
        space_level: genatrix_model::Level::Public,
        installed_ms: now_ms,
        // New items are what arrives from now on.
        cursor: Some(system.store.latest_item_row()?),
    };
    let path = system.agents.package_path(&agent.id, &agent.version);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, &package.bytes)?;
    std::fs::rename(&partial, &path)?;
    let manifest = toml::to_string(&package.manifest)?;
    system.store.insert_agent(&agent, &manifest)?;
    system.ledger.append(
        "agent_install",
        &agent.id,
        &serde_json::json!({ "name": agent.name, "version": agent.version }),
    )?;
    Ok(agent)
}

/// The package an agent runs, read from disk and checked against the
/// version the user approved. Any other bytes are refused, whatever
/// replaced them (design 02, invariant 17).
pub fn load(system: &System, agent: &StoredAgent) -> anyhow::Result<Package> {
    let path = system.agents.package_path(&agent.id, &agent.version);
    let bytes = std::fs::read(&path).with_context(|| format!("{}", path.display()))?;
    let hash = hex::encode(Sha256::digest(&bytes));
    if hash != agent.version || !system.store.agent_version_approved(&agent.id, &hash)? {
        bail!("the package on disk is not the version you approved");
    }
    Ok(Package::read(bytes)?)
}

/// Run an agent once and record the run.
pub async fn run(
    system: Arc<System>,
    agent_id: &str,
    invocation: Invocation,
) -> anyhow::Result<Outcome> {
    let _turn = system.agents.turn.lock().await;
    let agent = system
        .store
        .get_agent(agent_id)?
        .with_context(|| format!("no agent {agent_id}"))?;
    if agent.state == AgentState::Paused && !matches!(invocation, Invocation::Apply { .. }) {
        bail!("{} is paused", agent.name);
    }
    let package = load(&system, &agent)?;
    let quota = package.manifest.quota;
    let space = system.agents.space(&agent.id, quota.space_mb)?;
    let runner = system.agents.runner()?;

    let run_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now();
    let run = Run {
        invocation: invocation.clone(),
        now_ms: now.timestamp_millis(),
        seed: rand::random(),
        space_level: agent.space_level,
    };
    space.set_deadline(Some(
        Instant::now() + Duration::from_secs(u64::from(quota.seconds)),
    ));
    let doors = doors::StoreDoors::new(
        Arc::clone(&system),
        agent.clone(),
        package.manifest.clone(),
        Arc::clone(&space),
        run_id.clone(),
        now,
        tokio::runtime::Handle::current(),
    );
    let applying = matches!(invocation, Invocation::Apply { .. });
    if applying {
        space.begin_apply().map_err(anyhow::Error::msg)?;
    }
    let outcome = tokio::task::spawn_blocking(move || runner.run(&package, run, Box::new(doors)))
        .await
        .context("the agent's run did not finish")?;
    if applying {
        // An approved effect lands whole or not at all.
        space.end_apply(outcome.result.is_ok());
    }
    if space.finish() {
        tracing::warn!(agent = %agent.id, "a transaction left open was rolled back");
    }
    space.set_deadline(None);

    system.store.raise_space_level(&agent.id, outcome.taint)?;
    record(
        &system,
        &agent,
        &run_id,
        now.timestamp_millis(),
        &invocation,
        &outcome,
    )?;
    Ok(outcome)
}

/// Carry out every approved proposal of an agent's own kind: hand its
/// payload to the agent's `apply`, then report like a connector would.
/// Returns how many were carried out, well or not.
pub async fn execute_approved(system: Arc<System>) -> anyhow::Result<usize> {
    use genatrix_agent::action::Status;
    let handed = system
        .actions
        .approved_in_core(&system.store, &system.ledger)?;
    let count = handed.len();
    for (action, version, token) in handed {
        let genatrix_agent::Effect::Agent {
            agent,
            version: proposed_by,
            agent_kind,
            ..
        } = &action.effect
        else {
            continue;
        };
        let current = system.store.get_agent(agent)?;
        let status = match current {
            None => Status::Failed {
                detail: "the agent is no longer installed".into(),
            },
            Some(a) if &a.version != proposed_by => Status::Failed {
                detail: "the agent changed since it proposed this; ask it again".into(),
            },
            Some(_) => {
                let invocation = Invocation::Apply {
                    kind: agent_kind.clone(),
                    payload: version.payload.clone(),
                };
                match run(Arc::clone(&system), agent, invocation).await {
                    Ok(Outcome { result: Ok(_), .. }) => Status::Executed { result: None },
                    Ok(Outcome { result: Err(e), .. }) => Status::Failed {
                        detail: format!("{e:?}"),
                    },
                    Err(e) => Status::Failed {
                        detail: e.to_string(),
                    },
                }
            }
        };
        if let Err(e) = system.actions.report(
            &system.store,
            &system.ledger,
            &action.id,
            token.expose(),
            status,
            None,
        ) {
            tracing::warn!(action = %action.id, error = %e, "could not report an agent action");
        }
    }
    Ok(count)
}

fn record(
    system: &System,
    agent: &StoredAgent,
    run_id: &str,
    started_ms: i64,
    invocation: &Invocation,
    outcome: &Outcome,
) -> anyhow::Result<()> {
    let (kind, input) = match invocation {
        Invocation::Items(ids) => ("items", Some(ids.join(","))),
        Invocation::Message(text) => ("message", Some(text.clone())),
        Invocation::Schedule(name) => ("schedule", Some(name.clone())),
        Invocation::Apply { kind, .. } => ("apply", Some(kind.clone())),
    };
    let (verdict, detail, answer) = match &outcome.result {
        Ok(answer) => ("ok", None, answer.clone()),
        Err(RunError::Refused(e)) => ("refused", Some(e.clone()), None),
        Err(RunError::Limit(e)) => ("limit", Some(e.clone()), None),
        Err(RunError::Trap(e)) => ("trap", Some(e.clone()), None),
        Err(RunError::Agent(e)) => ("agent", Some(e.clone()), None),
    };
    system.store.insert_agent_run(&AgentRun {
        id: run_id.to_owned(),
        agent_id: agent.id.clone(),
        version: agent.version.clone(),
        started_ms,
        invocation: kind.to_owned(),
        input,
        outcome: verdict.to_owned(),
        detail,
        answer,
        level: outcome.taint,
        reads: outcome.reads.clone(),
        proposals: outcome.proposals.clone(),
        log: outcome.log.clone(),
        fuel: outcome.fuel_used,
    })?;
    // The ledger keeps references and a hash of the agent's words, not the
    // words: they may carry what it read (design 03, "运行记录").
    let words = serde_json::to_string(&(&outcome.log, &outcome.result.as_ref().ok()))?;
    system.ledger.append(
        "agent_run",
        &agent.id,
        &serde_json::json!({
            "run": run_id,
            "version": agent.version,
            "invocation": kind,
            "outcome": verdict,
            "level": outcome.taint.as_str(),
            "reads": outcome.reads,
            "proposals": outcome.proposals,
            "fuel": outcome.fuel_used,
            "words_sha256": hex::encode(Sha256::digest(words.as_bytes())),
        }),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
