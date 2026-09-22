//! The model processes: started, sandboxed, watched, restarted.
//!
//! Design: `docs/design/04-model-layer.md`, "两个进程". Inference runs in
//! `genatrix-infer` under a sandbox with no network; the gateway,
//! `genatrix-llm`, is what the core talks to, and what checks every ticket.
//! The design leaves starting them to the core, and this is where the core
//! does it, the same way it starts the mail connector: found beside this
//! binary, restarted when they stop, their state shown to the interface.
//!
//! The ticket key is generated when the core starts and handed to the
//! gateway in its environment. It is the token design 04 names between the
//! two, and it lives as long as this process.
//!
//! No model on disk is not an error. Collecting and searching mail need no
//! model, and design 09 downloads the weights in a first-run screen that
//! does not exist yet; until it does, the README says how. The state says
//! plainly what is missing.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use genatrix_connector::Fault;
use genatrix_keys::TicketKey;
use genatrix_llm::registry::Endpoint;
use serde::Serialize;
use tokio::sync::watch;

use crate::system::System;

/// What the model side is doing, for the interface.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelState {
    /// Not running, and not going to: what is missing.
    Off {
        /// Why.
        detail: String,
    },
    /// The processes are up or coming up; the gateway does not answer yet.
    Starting,
    /// The gateway answers and the inference socket is there.
    Ready,
    /// A process stopped and is being started again.
    Retrying {
        /// Which and why.
        detail: String,
        /// Failures in a row.
        attempt: u32,
        /// Seconds until the next start.
        next_in_secs: u64,
    },
}

impl ModelState {
    /// One line for the page.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Off { detail } => format!("off: {detail}"),
            Self::Starting => "starting".to_owned(),
            Self::Ready => "ready".to_owned(),
            Self::Retrying {
                detail,
                next_in_secs,
                ..
            } => format!("{detail}; starting again in {next_in_secs}s"),
        }
    }

    /// Whether a model call would be answered.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// The two binaries, found beside this one or named in the environment.
fn binary(name: &str, env: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(env) {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let beside = std::env::current_exe().ok()?.parent()?.join(name);
    beside.is_file().then_some(beside)
}

/// What one local model needs to run.
#[derive(Clone, Debug)]
struct Local {
    name: String,
    dir: PathBuf,
    socket: PathBuf,
}

/// Start the inference process and the gateway for the local model in the
/// registry, and keep them running. Returns at once; the state channel on
/// the system says how it is going.
pub fn start(system: Arc<System>, key: &TicketKey) -> anyhow::Result<()> {
    let state = system.model_state.clone();
    let off = |detail: String| {
        tracing::info!(%detail, "model side off");
        state.send_replace(ModelState::Off { detail });
    };

    let Some(entry) = system
        .gateway_config
        .registry
        .models
        .iter()
        .find(|m| matches!(m.endpoint, Endpoint::LocalSocket { .. }))
    else {
        off("no local model in gateway.toml".into());
        return Ok(());
    };
    let Endpoint::LocalSocket { path } = &entry.endpoint else {
        unreachable!("filtered above");
    };
    let local_socket = PathBuf::from(path);
    let local = Local {
        name: entry.model.clone(),
        dir: system.config.data_dir.join("models").join(&entry.model),
        socket: local_socket.clone(),
    };
    if !local.dir.join("config.json").is_file() {
        off(format!(
            "model weights are not at {}; see the README for the download",
            local.dir.display()
        ));
        return Ok(());
    }
    let (Some(infer), Some(gateway)) = (
        binary("genatrix-infer", "GENATRIX_INFER_BIN"),
        binary("genatrix-llm", "GENATRIX_LLM_BIN"),
    ) else {
        off("genatrix-infer or genatrix-llm is not beside this binary; `cargo build --release --workspace`".into());
        return Ok(());
    };

    state.send_replace(ModelState::Starting);
    let run_dir = system.config.data_dir.join("run");
    std::fs::create_dir_all(&run_dir)?;

    tokio::spawn(supervise("inference", Arc::clone(&system), move || {
        spawn_infer(&infer, &local, &run_dir)
    }));
    let key_hex = key.to_hex();
    let config_path = system.config.gateway_config_path();
    tokio::spawn(supervise("gateway", Arc::clone(&system), move || {
        spawn_gateway(&gateway, &config_path, &key_hex)
    }));
    let infer_socket = local_socket;
    tokio::spawn(readiness(system, infer_socket));
    Ok(())
}

/// Start a process, wait for it to end, say so, start it again.
async fn supervise<F>(what: &'static str, system: Arc<System>, spawn: F)
where
    F: Fn() -> std::io::Result<tokio::process::Child> + Send + 'static,
{
    let mut attempt: u32 = 0;
    loop {
        let mut child = match spawn() {
            Ok(child) => child,
            Err(e) => {
                attempt = attempt.saturating_add(1);
                let wait = Fault::backoff(attempt);
                tracing::error!(what, error = %e, "could not start");
                system.model_state.send_replace(ModelState::Retrying {
                    detail: format!("the {what} process could not be started: {e}"),
                    attempt,
                    next_in_secs: wait.as_secs(),
                });
                tokio::time::sleep(wait).await;
                continue;
            }
        };
        tracing::info!(what, pid = child.id(), "started");
        let started = std::time::Instant::now();
        let status = child.wait().await;
        tracing::warn!(what, ?status, "stopped");
        attempt = if started.elapsed().as_secs() > 60 {
            0
        } else {
            attempt.saturating_add(1)
        };
        let wait = Fault::backoff(attempt);
        system.model_state.send_replace(ModelState::Retrying {
            detail: format!("the {what} process stopped"),
            attempt,
            next_in_secs: wait.as_secs(),
        });
        tokio::time::sleep(wait).await;
    }
}

/// The inference process under its own sandbox profile, which it prints.
fn spawn_infer(
    binary: &Path,
    local: &Local,
    run_dir: &Path,
) -> std::io::Result<tokio::process::Child> {
    let mut common = vec![
        "--model-dir".to_owned(),
        local.dir.display().to_string(),
        "--model-name".to_owned(),
        local.name.clone(),
        "--socket".to_owned(),
        local.socket.display().to_string(),
    ];
    let profile = std::process::Command::new(binary)
        .args(&common)
        .arg("--print-sandbox-profile")
        .output()?;
    if !profile.status.success() {
        return Err(std::io::Error::other(format!(
            "could not get the sandbox profile: {}",
            String::from_utf8_lossy(&profile.stderr).trim()
        )));
    }
    let profile_path = run_dir.join("infer.sb");
    std::fs::write(&profile_path, &profile.stdout)?;

    let sandbox_exec = Path::new("/usr/bin/sandbox-exec");
    let mut command = if sandbox_exec.exists() {
        let mut c = tokio::process::Command::new(sandbox_exec);
        c.arg("-f").arg(&profile_path).arg(binary);
        c
    } else {
        tracing::warn!("no sandbox-exec on this system; inference runs unconfined");
        tokio::process::Command::new(binary)
    };
    common.push("--idle-timeout".to_owned());
    common.push("0".to_owned());
    command
        .args(&common)
        .env_remove("GENATRIX_TICKET_KEY")
        .env_remove("GENATRIX_IMAP_PASSWORD")
        .kill_on_drop(true)
        .spawn()
}

/// The gateway, with the ticket key in its environment and nothing else of
/// ours.
fn spawn_gateway(
    binary: &Path,
    config: &Path,
    key_hex: &str,
) -> std::io::Result<tokio::process::Child> {
    tokio::process::Command::new(binary)
        .arg("--config")
        .arg(config)
        .env("GENATRIX_TICKET_KEY", key_hex)
        .env_remove("GENATRIX_IMAP_PASSWORD")
        .kill_on_drop(true)
        .spawn()
}

/// Ready when the gateway answers and the inference socket exists; back to
/// starting when either goes away. Retrying is set by the supervisors and
/// left alone here until the gateway answers again.
async fn readiness(system: Arc<System>, infer_socket: PathBuf) {
    // A socket file can be left over from an earlier run; only an answer
    // means the process behind it is up.
    let infer = genatrix_llm::LocalClient::new(infer_socket);
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let up = infer.healthy().await && system.caller.gateway_healthy().await;
        let current = system.model_state.borrow().clone();
        if up && !current.is_ready() {
            tracing::info!("model side ready");
            system.model_state.send_replace(ModelState::Ready);
        } else if !up && current.is_ready() {
            // Retrying is set by the supervisors and left alone here.
            system.model_state.send_replace(ModelState::Starting);
        }
    }
}

/// A state channel starting from "not started here", for a `System` opened
/// by a command that does not run the model side.
#[must_use]
pub fn idle_state() -> watch::Sender<ModelState> {
    watch::channel(ModelState::Off {
        detail: "not started by this command".into(),
    })
    .0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_describes_itself_for_the_page() {
        assert_eq!(ModelState::Ready.describe(), "ready");
        assert!(
            ModelState::Off {
                detail: "no weights".into()
            }
            .describe()
            .contains("no weights")
        );
        assert!(
            ModelState::Retrying {
                detail: "the gateway process stopped".into(),
                attempt: 2,
                next_in_secs: 4
            }
            .describe()
            .ends_with("in 4s")
        );
        assert!(ModelState::Ready.is_ready());
        assert!(!ModelState::Starting.is_ready());
    }
}
