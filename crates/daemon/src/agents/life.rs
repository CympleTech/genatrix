//! An agent's life after install (design 11): woken by its triggers, tried
//! before it is installed, paused, exported, uninstalled.

use std::sync::{Arc, Mutex};

use anyhow::Context;
use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc, Weekday};
use genatrix_agent::action::Status;
use genatrix_host::manifest::{Effect, Manifest, TargetRule, Targets, Trigger};
use genatrix_host::{Invocation, Package, Run, RunError, wit};
use genatrix_store::{AgentState, Space, SpaceValue, StoredAgent};
use serde::Serialize;

use super::doors::{StoreDoors, Tried, scope};
use crate::system::System;

/// The most new items handed to an agent in one run.
const BATCH: u32 = 50;
/// The most recent items a trial hands over.
const TRIAL_ITEMS: u32 = 5;

/// How much a manifest allows, worst first (design 11, "装之前").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Reads, proposes nothing.
    Reads,
    /// Proposes only writes to its own space.
    Own,
    /// Proposes messages to people the user already deals with.
    Outward,
    /// Proposes messages to fixed addresses, who may be strangers.
    Strangers,
}

/// The worst thing this manifest allows.
#[must_use]
pub fn risk(m: &Manifest) -> Risk {
    m.proposes
        .iter()
        .map(|p| match (&p.effect, &p.targets) {
            (Effect::Own, _) => Risk::Own,
            (_, Some(Targets::Addresses(_))) => Risk::Strangers,
            (_, Some(Targets::Rule(TargetRule::SameThread | TargetRule::KnownContacts)) | None) => {
                Risk::Outward
            }
        })
        .max()
        .unwrap_or(Risk::Reads)
}

/// Wake every active agent whose trigger is due: new items in its scope,
/// or a schedule. Returns how many runs there were.
pub async fn wake(system: Arc<System>) -> anyhow::Result<usize> {
    let mut runs = 0;
    for agent in system.store.all_agents()? {
        if agent.state != AgentState::Active {
            continue;
        }
        let package = match super::load(&system, &agent) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(agent = %agent.id, error = %e, "an agent's package cannot be run");
                continue;
            }
        };
        let m = &package.manifest;
        if m.triggers.iter().any(|t| matches!(t, Trigger::Items)) {
            runs += usize::from(new_items(&system, &agent, m).await?);
        }
        for t in &m.triggers {
            let Trigger::Schedule { name, every, at } = t else {
                continue;
            };
            let Some(due) = last_due(every, at, Local::now()) else {
                continue;
            };
            let due_ms = due.timestamp_millis();
            let last = system.store.last_schedule_run(&agent.id, name)?;
            if due_ms > agent.installed_ms && last.is_none_or(|l| l < due_ms) {
                super::run(
                    Arc::clone(&system),
                    &agent.id,
                    Invocation::Schedule(name.clone()),
                )
                .await?;
                runs += 1;
            }
        }
    }
    Ok(runs)
}

/// Hand an agent the items that arrived in its scope since it last looked.
/// The cursor moves on whether or not the run went well: an item that
/// makes an agent fail should not make it fail forever.
async fn new_items(
    system: &Arc<System>,
    agent: &StoredAgent,
    m: &Manifest,
) -> anyhow::Result<bool> {
    let Some(cursor) = agent.cursor else {
        system
            .store
            .set_agent_cursor(&agent.id, system.store.latest_item_row()?)?;
        return Ok(false);
    };
    let whole = wit::Filter {
        since_ms: None,
        until_ms: None,
        text: None,
        limit: BATCH,
    };
    let Some(mut q) = scope(m, Utc::now(), &whole) else {
        return Ok(false);
    };
    q.after_row = Some(cursor);
    q.by_ingestion = true;
    let items = system.store.query_items(&q)?;
    let Some(last) = items.last() else {
        return Ok(false);
    };
    let next = system.store.item_row(last.id)?.unwrap_or(cursor);
    let ids = items.iter().map(|i| i.id.to_string()).collect();
    system.store.set_agent_cursor(&agent.id, next)?;
    super::run(Arc::clone(system), &agent.id, Invocation::Items(ids)).await?;
    Ok(true)
}

/// The most recent moment at or before `now` that `every` at `at` names,
/// in local time.
#[must_use]
pub fn last_due(every: &str, at: &str, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let time = NaiveTime::parse_from_str(at, "%H:%M").ok()?;
    let on = |date: chrono::NaiveDate| Local.from_local_datetime(&date.and_time(time)).earliest();
    let today = now.date_naive();
    match every.split_once(':') {
        None if every == "daily" => {
            let t = on(today)?;
            if t <= now {
                Some(t)
            } else {
                on(today - Duration::days(1))
            }
        }
        Some(("weekly", day)) => {
            let want: Weekday = day.parse().ok()?;
            (0..8)
                .map(|back| today - Duration::days(back))
                .filter(|d| d.weekday() == want)
                .filter_map(on)
                .find(|t| *t <= now)
        }
        Some(("monthly", day)) => {
            let d: u32 = day.parse().ok()?;
            let this = today.with_day(d).and_then(on)?;
            if this <= now {
                return Some(this);
            }
            let prev = if today.month() == 1 {
                today.with_year(today.year() - 1)?.with_month(12)?
            } else {
                today.with_day(1)?.with_month(today.month() - 1)?
            };
            prev.with_day(d).and_then(on)
        }
        _ => None,
    }
}

/// What a trial run found.
#[derive(Debug, Serialize)]
pub struct Trial {
    /// How many recent items it was given.
    pub items: usize,
    /// What it would have proposed.
    pub proposals: Vec<Tried>,
    /// What it logged.
    pub log: Vec<String>,
    /// `ok`, `refused`, `limit`, `trap`, `agent`, or `none` when it has no
    /// trigger that can be tried in advance.
    pub outcome: &'static str,
    /// Why, when it did not end well.
    pub detail: Option<String>,
}

/// Run a package before it is installed: on the most recent items in its
/// scope, with a throwaway space, proposals held back and shown instead
/// (design 11, ruling 9). Nothing it does is kept.
pub async fn trial(system: Arc<System>, package: Package) -> anyhow::Result<Trial> {
    if !package
        .manifest
        .triggers
        .iter()
        .any(|t| matches!(t, Trigger::Items))
    {
        return Ok(Trial {
            items: 0,
            proposals: Vec::new(),
            log: Vec::new(),
            outcome: "none",
            detail: None,
        });
    }
    let now = Utc::now();
    let agent = StoredAgent {
        id: format!("trial-{}", &package.hash[..12]),
        name: package.manifest.name.clone(),
        version: package.hash.clone(),
        state: AgentState::Active,
        space_level: genatrix_model::Level::Public,
        installed_ms: now.timestamp_millis(),
        cursor: None,
    };
    let mut key = [0u8; 32];
    rand::fill(&mut key);
    let space = Arc::new(Space::open_in_memory(
        &genatrix_keys::DbKey::from_bytes(key),
        package.manifest.quota.space_mb,
    )?);
    let sink = Arc::new(Mutex::new(Vec::new()));
    let runner = system.agents.runner()?;
    let handle = tokio::runtime::Handle::current();
    let manifest = package.manifest.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let recent = wit::Filter {
            since_ms: None,
            until_ms: None,
            text: None,
            limit: TRIAL_ITEMS,
        };
        let ids: Vec<String> = scope(&manifest, now, &recent)
            .and_then(|q| system.store.query_items(&q).ok())
            .unwrap_or_default()
            .iter()
            .map(|i| i.id.to_string())
            .collect();
        let count = ids.len();
        let doors = StoreDoors::new(
            Arc::clone(&system),
            agent,
            manifest,
            space,
            "trial".into(),
            now,
            handle,
        )
        .trial(Arc::clone(&sink));
        let run = Run {
            invocation: Invocation::Items(ids),
            now_ms: now.timestamp_millis(),
            seed: 0,
            space_level: genatrix_model::Level::Public,
        };
        (count, runner.run(&package, run, Box::new(doors)), sink)
    })
    .await
    .context("the trial did not finish")?;
    let (items, outcome, sink) = outcome;
    let (verdict, detail) = match &outcome.result {
        Ok(_) => ("ok", None),
        Err(RunError::Refused(e)) => ("refused", Some(e.clone())),
        Err(RunError::Limit(e)) => ("limit", Some(e.clone())),
        Err(RunError::Trap(e)) => ("trap", Some(e.clone())),
        Err(RunError::Agent(e)) => ("agent", Some(e.clone())),
    };
    let proposals = sink
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    Ok(Trial {
        items,
        proposals,
        log: outcome.log,
        outcome: verdict,
        detail,
    })
}

/// Actions an agent proposed that the user has not yet seen carried out:
/// pending, or approved and not yet taken.
fn open_actions(system: &System, agent: &StoredAgent) -> anyhow::Result<Vec<String>> {
    let runs: std::collections::HashSet<String> = system
        .store
        .agent_runs(&agent.id, 10_000)?
        .into_iter()
        .map(|r| r.id)
        .collect();
    let mut out = Vec::new();
    for status in ["pending", "approved"] {
        for a in system.actions.list(&system.store, Some(status), 1000)? {
            let mine = match &a.effect {
                genatrix_agent::Effect::Agent { agent: id, .. } => id == &agent.id,
                _ => runs.contains(&a.run_id),
            };
            let takeable = matches!(a.status, Status::Pending) || a.token_for_execution().is_some();
            if mine && takeable {
                out.push(a.id);
            }
        }
    }
    Ok(out)
}

/// Stop waking an agent, and take back what it proposed that has not been
/// carried out (design 11, "暂停与停用").
pub fn pause(system: &System, id: &str) -> anyhow::Result<usize> {
    let agent = system.store.get_agent(id)?.context("no such agent")?;
    system.store.set_agent_state(id, AgentState::Paused)?;
    let open = open_actions(system, &agent)?;
    for action in &open {
        if let Err(e) = system.actions.decline(
            &system.store,
            &system.ledger,
            action,
            "the agent was paused",
        ) {
            tracing::warn!(action, error = %e, "could not withdraw an agent's proposal");
        }
    }
    system.ledger.append(
        "agent_pause",
        id,
        &serde_json::json!({ "withdrawn": open.len() }),
    )?;
    Ok(open.len())
}

/// Wake an agent again.
pub fn resume(system: &System, id: &str) -> anyhow::Result<()> {
    system.store.get_agent(id)?.context("no such agent")?;
    system.store.set_agent_state(id, AgentState::Active)?;
    system
        .ledger
        .append("agent_resume", id, &serde_json::json!({}))?;
    Ok(())
}

/// Everything in an agent's space, as JSON the user can keep.
pub fn export(system: &System, id: &str) -> anyhow::Result<serde_json::Value> {
    let agent = system.store.get_agent(id)?.context("no such agent")?;
    let package = super::load(system, &agent)?;
    let space = system.agents.space(id, package.manifest.quota.space_mb)?;
    let tables = space.dump().map_err(anyhow::Error::msg)?;
    let value = |v: &SpaceValue| match v {
        SpaceValue::Null => serde_json::Value::Null,
        SpaceValue::Integer(i) => serde_json::json!(i),
        SpaceValue::Real(f) => serde_json::json!(f),
        SpaceValue::Text(s) => serde_json::json!(s),
        SpaceValue::Bytes(b) => serde_json::json!({ "hex": hex::encode(b) }),
    };
    let tables: serde_json::Map<String, serde_json::Value> = tables
        .iter()
        .map(|(name, columns, rows)| {
            (
                name.clone(),
                serde_json::json!({
                    "columns": columns,
                    "rows": rows.iter().map(|r| r.iter().map(value).collect::<Vec<_>>()).collect::<Vec<_>>(),
                }),
            )
        })
        .collect();
    Ok(serde_json::json!({
        "agent": agent.name,
        "version": agent.version,
        "exported_at": Utc::now().to_rfc3339(),
        "level": agent.space_level.as_str(),
        "tables": tables,
    }))
}

/// Remove an agent: withdraw what it proposed, delete its packages and its
/// space, forget it. The ledger keeps what it did.
pub fn uninstall(system: &System, id: &str) -> anyhow::Result<()> {
    pause(system, id)?;
    system.agents.forget_space(id);
    let dir = system.agents.dir.join(id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    system.store.delete_agent(id)?;
    system
        .ledger
        .append("agent_uninstall", id, &serde_json::json!({}))?;
    Ok(())
}

/// Bytes an agent's space takes on disk.
#[must_use]
pub fn space_bytes(system: &System, id: &str) -> u64 {
    std::fs::metadata(system.agents.dir.join(id).join("space.db")).map_or(0, |m| m.len())
}
