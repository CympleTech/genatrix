//! `genatrix agent`: install, list, run, pause from a terminal on this
//! machine. A terminal here is loopback, which is what installing needs
//! (design 06, "设备分两种"). The page gets the same with A4.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Subcommand;
use genatrix_host::manifest::{BlobAccess, Effect, Manifest, Targets, Trigger};
use genatrix_host::{Invocation, Package};
use genatrix_store::AgentState;

use crate::system::System;

/// What to do with agents.
#[derive(Debug, Subcommand)]
pub enum AgentAction {
    /// Show what a package may do, and install it if you agree.
    Install {
        /// The package, a `.wasm` file.
        file: PathBuf,
        /// Do not ask; you have read what it may do.
        #[arg(long)]
        yes: bool,
    },
    /// List installed agents.
    List,
    /// Write to an agent, as its conversation would.
    Ask {
        /// The agent's id.
        id: String,
        /// What to say.
        message: String,
    },
    /// Stop waking an agent.
    Pause {
        /// The agent's id.
        id: String,
    },
    /// Wake an agent again.
    Resume {
        /// The agent's id.
        id: String,
    },
    /// Show an agent's recent runs.
    Runs {
        /// The agent's id.
        id: String,
    },
}

/// What a manifest allows, one line per grant, in words.
#[must_use]
pub fn describe(m: &Manifest) -> Vec<(&'static str, String)> {
    let r = &m.reads;
    let reads = if r.connectors.is_empty() {
        "nothing".to_owned()
    } else {
        let kinds = if r.kinds.is_empty() {
            "items".to_owned()
        } else {
            r.kinds.join(" and ")
        };
        let attachment = match (r.with_attachment, r.mime.is_empty()) {
            (false, _) => String::new(),
            (true, true) => " with an attachment".to_owned(),
            (true, false) => format!(" with an attachment of type {}", r.mime.join(", ")),
        };
        let mentioning = if r.matching.is_empty() {
            String::new()
        } else {
            format!(" mentioning {}", r.matching.join(" or "))
        };
        let window = r.days.map_or_else(
            || "all history".to_owned(),
            |d| format!("the last {d} days"),
        );
        let s = format!(
            "{kinds} from {}{attachment}{mentioning}, from {window}, up to {}",
            r.connectors.join(" and "),
            r.max_level.as_str()
        );
        s
    };
    let blobs = match m.blobs {
        BlobAccess::None => "no attachments".to_owned(),
        BlobAccess::Text => "the text of those attachments".to_owned(),
    };
    let proposes = if m.proposes.is_empty() {
        "nothing".to_owned()
    } else {
        m.proposes
            .iter()
            .map(|p| {
                let reach = match (&p.effect, &p.targets) {
                    (Effect::Own, _) => "in its own space".to_owned(),
                    (e, Some(Targets::Addresses(a))) => format!("{e:?} to {}", a.join(", ")),
                    (e, Some(Targets::Rule(rule))) => format!("{e:?}, {rule:?}"),
                    (e, None) => format!("{e:?}"),
                };
                format!("{} ({reach}), with your approval", p.label)
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    let model = if m.model.is_empty() {
        "no model".to_owned()
    } else {
        format!(
            "{}; on this machine only",
            m.model
                .iter()
                .map(|p| format!("{p:?}").to_lowercase())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let when = m
        .triggers
        .iter()
        .map(|t| match t {
            Trigger::Items => "when new items in scope arrive".to_owned(),
            Trigger::Message => "when you write to it".to_owned(),
            Trigger::Schedule { every, at, .. } => format!("{every} at {at}"),
        })
        .collect::<Vec<_>>()
        .join("; ");
    vec![
        ("reads", reads),
        ("takes", blobs),
        ("proposes", proposes),
        ("model", model),
        ("runs", when),
        (
            "cannot",
            "reach the network, read anything else, or see other agents' data".into(),
        ),
    ]
}

/// Carry out one `genatrix agent` command.
pub async fn command(system: Arc<System>, action: AgentAction) -> anyhow::Result<()> {
    match action {
        AgentAction::Install { file, yes } => {
            let bytes = std::fs::read(&file)?;
            let package = Package::read(bytes.clone())?;
            let m = &package.manifest;
            println!("{}  by {} (as it says)", m.name, m.author);
            println!("  {}", m.purpose);
            println!("  version {}", &package.hash[..16]);
            for (label, text) in describe(m) {
                println!("  {label:<9} {text}");
            }
            if !yes {
                print!("Install? [y/N] ");
                std::io::stdout().flush()?;
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                if !line.trim().eq_ignore_ascii_case("y") {
                    println!("Not installed.");
                    return Ok(());
                }
            }
            let agent = super::install(&system, bytes, chrono::Utc::now().timestamp_millis())?;
            println!("Installed as {}", agent.id);
        }
        AgentAction::List => {
            for a in system.store.all_agents()? {
                let state = match a.state {
                    AgentState::Active => "active",
                    AgentState::Paused => "paused",
                };
                println!(
                    "{}  {:<24} {state:<7} {}  {}",
                    a.id,
                    a.name,
                    a.space_level.as_str(),
                    &a.version[..16]
                );
            }
        }
        AgentAction::Ask { id, message } => {
            let outcome = super::run(system, &id, Invocation::Message(message)).await?;
            for line in &outcome.log {
                println!("  log: {line}");
            }
            match outcome.result {
                Ok(Some(answer)) => println!("{answer}"),
                Ok(None) => {}
                Err(e) => println!("did not finish: {e:?}"),
            }
        }
        AgentAction::Pause { id } => system.store.set_agent_state(&id, AgentState::Paused)?,
        AgentAction::Resume { id } => system.store.set_agent_state(&id, AgentState::Active)?,
        AgentAction::Runs { id } => {
            for r in system.store.agent_runs(&id, 20)? {
                println!(
                    "{}  {:<8} {:<8} {:<8} reads {:>3}  {}",
                    r.id,
                    r.invocation,
                    r.outcome,
                    r.level.as_str(),
                    r.reads.len(),
                    r.detail.unwrap_or_default()
                );
            }
        }
    }
    Ok(())
}
