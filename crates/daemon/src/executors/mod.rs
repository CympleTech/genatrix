//! Executors in the core: projects, drafts, running dispatches, and the
//! GitHub connector that carries their results out.
//!
//! Design: `docs/design/13-executors.md`.
//!
//! The core keeps the judgement and the record; executors such as Claude
//! Code and Codex do the work in workspaces of their own, as their own
//! system user, started through the executor host (`genatrix-exec`). This
//! module is the core's side of that:
//!
//! - which executors exist and how to start them (`executors.toml`);
//! - the projects, roles and recipes on file, the built-in ones seeded;
//! - [`draft`]: a dispatch as a pending action, the task in the user's words;
//! - [`run`]: an approved dispatch carried out step by step, every text
//!   through the gate, every session through the host;
//! - [`github`]: approved pushes and pull requests, through a sandboxed
//!   one-shot connector with the user's token.

pub mod draft;
pub mod github;
pub mod host;
pub mod run;
pub mod suggest;
pub mod terminal;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use genatrix_dispatch::project::Project;
use genatrix_dispatch::recipe::Recipe;
use genatrix_dispatch::role::Role;
use genatrix_dispatch::workspace::Workspace;
use genatrix_exec::isolation::Isolation;
use serde::{Deserialize, Serialize};

use crate::system::System;

/// One executor: how it is started and whose service it sends to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorEntry {
    /// Its name, as recipes and projects name it.
    pub name: String,
    /// Whose service it talks to: what the egress record names.
    pub vendor: String,
    /// The command that starts its ACP adapter, as the executor's user.
    pub command: Vec<String>,
    /// The command that signs the executor's user in, run by
    /// `genatrix exec login`.
    #[serde(default)]
    pub login: Vec<String>,
    /// The search path it runs with.
    #[serde(default = "default_path")]
    pub path: String,
    /// The environment variable a sign-in token is given in, for an
    /// executor whose own login cannot work as a user with no desktop: no
    /// browser, no keychain. `genatrix exec login` then asks for the token
    /// and leaves it in the executor's user's home, where the host reads it
    /// when it starts the executor. Genatrix keeps no copy.
    #[serde(default)]
    pub token_env: Option<String>,
}

fn default_path() -> String {
    "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_owned()
}

/// `executors.toml`: where the shared directories are and which executors
/// there are.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecConfig {
    /// Where the mirrors and the workspaces live: `mirrors/` and `exec/`.
    #[serde(default = "default_root")]
    pub root: PathBuf,
    /// The longest one step may run, in minutes.
    #[serde(default = "default_timeout")]
    pub timeout_minutes: u64,
    /// The executors.
    #[serde(default = "default_executors", rename = "executor")]
    pub executors: Vec<ExecutorEntry>,
}

fn default_root() -> PathBuf {
    PathBuf::from("/Users/Shared/Genatrix")
}

const fn default_timeout() -> u64 {
    30
}

fn default_executors() -> Vec<ExecutorEntry> {
    vec![
        ExecutorEntry {
            name: "claude-code".into(),
            vendor: "anthropic".into(),
            command: vec![
                "npx".into(),
                "-y".into(),
                "@zed-industries/claude-agent-acp".into(),
            ],
            // The executor's user cannot see a `claude` installed in your
            // home, so its own copy is fetched the way the adapter is.
            login: vec![
                "npx".into(),
                "-y".into(),
                "@anthropic-ai/claude-code".into(),
                "/login".into(),
            ],
            path: default_path(),
            // Made with `claude setup-token`, as yourself.
            token_env: Some("CLAUDE_CODE_OAUTH_TOKEN".into()),
        },
        ExecutorEntry {
            name: "codex".into(),
            vendor: "openai".into(),
            command: vec![
                "npx".into(),
                "-y".into(),
                "@zed-industries/codex-acp".into(),
            ],
            login: vec![
                "npx".into(),
                "-y".into(),
                "@openai/codex".into(),
                "login".into(),
            ],
            path: default_path(),
            token_env: None,
        },
    ]
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            root: default_root(),
            timeout_minutes: default_timeout(),
            executors: default_executors(),
        }
    }
}

impl ExecConfig {
    /// Read `executors.toml`, or the defaults when there is none.
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// An executor by name.
    #[must_use]
    pub fn executor(&self, name: &str) -> Option<&ExecutorEntry> {
        self.executors.iter().find(|e| e.name == name)
    }

    /// Where a project's mirror is.
    #[must_use]
    pub fn mirror(&self, project: &str) -> PathBuf {
        self.root.join("mirrors").join(format!("{project}.git"))
    }

    /// Where a dispatch's workspace is.
    #[must_use]
    pub fn workspace(&self, dispatch: &str) -> PathBuf {
        self.root.join("exec").join(dispatch.to_lowercase())
    }
}

/// The core's executor state: configuration, the host, who runs as whom,
/// and the dispatches running now with their stop buttons.
pub struct Exec {
    /// From `executors.toml`.
    pub config: RwLock<ExecConfig>,
    host: RwLock<Arc<dyn host::Host>>,
    isolation: RwLock<Isolation>,
    running: Mutex<HashMap<String, Running>>,
}

/// A dispatch in progress.
#[derive(Default)]
struct Running {
    stopped: bool,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Exec {
    /// The executor state for a data directory.
    pub fn load(config: &crate::config::Config) -> anyhow::Result<Self> {
        Ok(Self {
            config: RwLock::new(ExecConfig::load(&config.executors_path())?),
            host: RwLock::new(Arc::new(host::ProcessHost)),
            isolation: RwLock::new(Isolation::exec_user()),
            running: Mutex::new(HashMap::new()),
        })
    }

    /// The configuration as it is now.
    pub fn config(&self) -> ExecConfig {
        self.config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn host(&self) -> Arc<dyn host::Host> {
        Arc::clone(
            &self
                .host
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    pub(crate) fn isolation(&self) -> Isolation {
        self.isolation
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// For tests: a stand-in host, executors as this user, a root of their
    /// own.
    #[cfg(test)]
    pub(crate) fn use_for_tests(&self, host: Arc<dyn host::Host>, root: PathBuf) {
        *self.host.write().unwrap() = host;
        *self.isolation.write().unwrap() = Isolation::SameUserForTests;
        self.config.write().unwrap().root = root;
    }

    fn begin(&self, id: &str) {
        self.running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.to_owned(), Running::default());
    }

    /// Hand over the stop button for the step about to run; `false` if the
    /// dispatch has been stopped already.
    fn arm(&self, id: &str, cancel: tokio::sync::oneshot::Sender<()>) -> bool {
        let mut running = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match running.get_mut(id) {
            Some(r) if !r.stopped => {
                r.cancel = Some(cancel);
                true
            }
            _ => false,
        }
    }

    fn stopped(&self, id: &str) -> bool {
        self.running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .is_none_or(|r| r.stopped)
    }

    fn end(&self, id: &str) {
        self.running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
    }

    /// Stop a running dispatch: the step in progress is cancelled, and no
    /// step after it starts. `false` if it is not running.
    pub fn stop(&self, id: &str) -> bool {
        let mut running = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(r) = running.get_mut(id) else {
            return false;
        };
        r.stopped = true;
        if let Some(cancel) = r.cancel.take() {
            let _ = cancel.send(());
        }
        true
    }
}

/// The definitions on file, read.
pub struct Definitions {
    /// Workspaces (design 14).
    pub workspaces: Vec<(Workspace, String)>,
    /// Projects.
    pub projects: Vec<(Project, String)>,
    /// Roles.
    pub roles: Vec<(Role, String)>,
    /// Recipes.
    pub recipes: Vec<(Recipe, String)>,
}

/// Put the built-in workspace, roles and recipes on file where nothing of
/// that name is in use. One the user filed under the same name stays
/// theirs; a built-in they removed comes back.
pub fn seed(store: &genatrix_store::Store) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp_millis();
    for (kind, files) in [
        ("workspace", genatrix_dispatch::builtin::WORKSPACE_FILES),
        ("role", genatrix_dispatch::builtin::ROLE_FILES),
        ("recipe", genatrix_dispatch::builtin::RECIPE_FILES),
    ] {
        let existing: Vec<String> = store
            .active_definitions(kind)?
            .into_iter()
            .map(|d| d.name)
            .collect();
        for (name, text) in files {
            if !existing.iter().any(|n| n == name) {
                store.put_definition(kind, name, &genatrix_dispatch::hash_of(text), text, now)?;
            }
        }
    }
    Ok(())
}

/// Read every active definition. One that no longer parses is skipped with
/// a warning, never half-used.
pub fn definitions(store: &genatrix_store::Store) -> anyhow::Result<Definitions> {
    fn read<T>(
        store: &genatrix_store::Store,
        kind: &str,
        parse: impl Fn(&str) -> Result<T, genatrix_dispatch::DefinitionError>,
    ) -> anyhow::Result<Vec<(T, String)>> {
        let mut out = Vec::new();
        for d in store.active_definitions(kind)? {
            match parse(&d.text) {
                Ok(v) => out.push((v, d.hash)),
                Err(e) => tracing::warn!(kind, name = %d.name, error = %e, "definition skipped"),
            }
        }
        Ok(out)
    }
    Ok(Definitions {
        workspaces: read(store, "workspace", Workspace::parse)?,
        projects: read(store, "project", Project::parse)?,
        roles: read(store, "role", Role::parse)?,
        recipes: read(store, "recipe", Recipe::parse)?,
    })
}

/// File a project, role or recipe, checked first. Returns its name and
/// hash. Adding a project decides which vendors see its code, so the page
/// and the terminal only allow it on this machine (design 13, "派单就是
/// 出境"; design 02, invariant 12).
pub fn file_definition(
    store: &genatrix_store::Store,
    kind: &str,
    text: &str,
) -> anyhow::Result<(String, String)> {
    seed(store)?;
    let name = match kind {
        "workspace" => {
            // Invariant 30: an account belongs to one workspace at most.
            let workspace = Workspace::parse(text)?;
            let others: Vec<Workspace> = definitions(store)?
                .workspaces
                .into_iter()
                .map(|(w, _)| w)
                .collect();
            genatrix_dispatch::workspace::fits(&workspace, &others)?;
            workspace.name
        }
        "project" => {
            // Invariant 30: a project is in exactly one workspace, and it is
            // one on file.
            let project = Project::parse(text)?;
            if !definitions(store)?
                .workspaces
                .iter()
                .any(|(w, _)| w.name == project.workspace)
            {
                anyhow::bail!(
                    "there is no workspace `{}`; file it first with `genatrix workspace add`",
                    project.workspace
                );
            }
            check_repo(&project)?;
            project.name
        }
        "role" => Role::parse(text)?.name,
        "recipe" => Recipe::parse(text)?.name,
        other => anyhow::bail!("`{other}` is not a kind of definition"),
    };
    let hash = genatrix_dispatch::hash_of(text);
    store.put_definition(
        kind,
        &name,
        &hash,
        text,
        chrono::Utc::now().timestamp_millis(),
    )?;
    Ok((name, hash))
}

/// Stop using a definition. A workspace that projects still name stays:
/// every project is in one that exists (invariant 30). Returns whether there
/// was one of that name.
pub fn retire_definition(
    store: &genatrix_store::Store,
    kind: &str,
    name: &str,
) -> anyhow::Result<bool> {
    if kind == "workspace" {
        let users: Vec<String> = definitions(store)?
            .projects
            .into_iter()
            .filter(|(p, _)| p.workspace == name)
            .map(|(p, _)| p.name)
            .collect();
        if !users.is_empty() {
            anyhow::bail!(
                "projects {} are in workspace `{name}`; move or remove them first",
                users.join(", ")
            );
        }
    }
    Ok(store.retire_definition(kind, name)?)
}

/// Whether the thread a dispatch came from may go to `project`'s
/// executors: its account must be in the project's workspace, as the
/// workspaces stand now (design 14, invariant 31). The words are for the
/// card or the dispatch's record.
pub fn check_source(
    store: &genatrix_store::Store,
    project: &Project,
    thread: &str,
) -> Result<(), String> {
    let id: genatrix_model::ThreadId = thread
        .parse()
        .map_err(|_| "the work's source is not a thread".to_owned())?;
    let thread = store
        .get_thread(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "the thread the work came from is gone".to_owned())?;
    let defs = definitions(store).map_err(|e| e.to_string())?;
    let workspaces: Vec<Workspace> = defs.workspaces.into_iter().map(|(w, _)| w).collect();
    let key = genatrix_dispatch::workspace::account_key(
        thread.source.connector.as_str(),
        &thread.source.account,
    );
    genatrix_dispatch::workspace::may_delegate(&workspaces, project, &key)
        .map_err(|e| e.to_string())
}

/// The workspace a source account belongs to, by its connector and
/// account id, if any.
#[must_use]
pub fn workspace_of<'a>(
    defs: &'a Definitions,
    source: &genatrix_model::Source,
) -> Option<&'a Workspace> {
    let key = genatrix_dispatch::workspace::account_key(source.connector.as_str(), &source.account);
    defs.workspaces.iter().map(|(w, _)| w).find(|w| w.has(&key))
}

/// A project's repository must be there, be a git repository, and have
/// its base branch with a commit on it: work is cloned from that branch.
/// Said when the project is filed, not when the first dispatch fails.
pub fn check_repo(project: &Project) -> anyhow::Result<()> {
    let repo = repo_path(project);
    if !repo.is_dir() {
        anyhow::bail!("`{}` does not exist on this computer", repo.display());
    }
    let is_repo = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "--git-dir"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !is_repo {
        anyhow::bail!(
            "`{}` is not a git repository; clone the project there first",
            repo.display()
        );
    }
    let has_base = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{}^{{commit}}", project.base))
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_base {
        anyhow::bail!(
            "`{}` has no branch `{}` with a commit on it; work starts from that branch",
            repo.display(),
            project.base
        );
    }
    Ok(())
}

/// Mark dispatches that were running when the core stopped: they cannot be
/// picked up halfway, since the executor session is gone with the host.
pub fn recover(system: &System) -> anyhow::Result<usize> {
    let rows = system.store.running_dispatches()?;
    let n = rows.len();
    for mut row in rows {
        row.state = "failed".into();
        row.detail = Some("Genatrix stopped while this was running; start it again".into());
        row.ended_ms = Some(chrono::Utc::now().timestamp_millis());
        system.store.update_dispatch(&row)?;
        let ws = system.exec.config().workspace(&row.id);
        let iso = system.exec.isolation();
        // Left to finish on its own: nothing waits for a cleanup.
        drop(tokio::task::spawn_blocking(move || {
            genatrix_exec::workspace::remove(&iso, &ws)
        }));
    }
    Ok(n)
}

/// The project's repository path, with `~` read as the user's home.
#[must_use]
pub fn repo_path(project: &Project) -> PathBuf {
    match project.repo.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map_or_else(
            || PathBuf::from(&project.repo),
            |h| PathBuf::from(h).join(rest),
        ),
        None => PathBuf::from(&project.repo),
    }
}
