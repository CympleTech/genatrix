//! Approved pushes and pull requests, carried out by `genatrix-github`
//! (design 13, "第三层其实是老规矩：对外效果").
//!
//! The token lives in the keychain. For each approved action the core reads
//! it, starts the connector for that one action under a sandbox that allows
//! port 443 and nothing else, and hands the token over its standard input.
//! A pull request waits, approved, until its branch has been pushed; it is
//! not taken before, so its approval is not spent on a branch that is not
//! there.
//!
//! It also reads each project's open issues and pull requests, through the
//! same connector, when a space's page asks (design 13 v0.7, "读 issue 和
//! PR"). What comes back is kept in memory only.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use genatrix_agent::action::{Effect, Status};
use genatrix_connector_github::{Reply, Request};
use tokio::io::AsyncWriteExt as _;

use crate::keychain::Keychain;
use crate::system::System;

/// The keychain service and account the token is kept under.
pub const SERVICE: &str = "Genatrix GitHub";
/// The account within it for the default token, the one that belongs to
/// no workspace.
pub const ACCOUNT: &str = "token";

/// The keychain account a workspace's own token is under, or the default's
/// for none (design 14).
#[must_use]
pub fn account_for(workspace: Option<&str>) -> String {
    workspace.map_or_else(|| ACCOUNT.to_owned(), |w| format!("{ACCOUNT}:{w}"))
}

/// Where a workspace's work looks for a token, in order: its own, then
/// the default. Never another workspace's (invariant 32).
#[must_use]
pub fn accounts_tried(workspace: &str) -> [String; 2] {
    [account_for(Some(workspace)), account_for(None)]
}

/// The token a workspace's work pushes with, and which keychain account
/// it came from.
pub fn token_for(workspace: &str) -> anyhow::Result<Option<String>> {
    let chain = Keychain::named(SERVICE);
    for account in accounts_tried(workspace) {
        if let Some(token) = chain.read(&account)? {
            return Ok(Some(token));
        }
    }
    Ok(None)
}

/// The workspace a project is in now, by its name; `personal` when the
/// project is no longer on file.
fn workspace_of_project(system: &System, project: &str) -> String {
    super::definitions(&system.store)
        .ok()
        .and_then(|d| {
            d.projects
                .into_iter()
                .find(|(p, _)| p.name == project)
                .map(|(p, _)| p.workspace)
        })
        .unwrap_or_else(|| genatrix_dispatch::workspace::PERSONAL.to_owned())
}

/// The token, if the user signed in.
pub fn token() -> Option<String> {
    stored_token().ok().flatten()
}

/// The token, or why the keychain would not give it. A token that is there
/// but cannot be read is not the same as no token: the keychain refuses a
/// process with no access to the login session, such as one started over
/// SSH or inside some terminal multiplexers.
pub fn stored_token() -> anyhow::Result<Option<String>> {
    Keychain::named(SERVICE).read(ACCOUNT)
}

/// Carry out every approved push and pull request that can go now.
#[allow(clippy::too_many_lines)] // take, run, report, in one order
pub async fn execute_approved(system: &Arc<System>) -> anyhow::Result<usize> {
    let approved: Vec<_> = system
        .actions
        .list(&system.store, Some("approved"), 200)?
        .into_iter()
        .filter(|a| {
            matches!(
                a.effect,
                Effect::PushBranch { .. } | Effect::OpenPullRequest { .. }
            ) && a.token_for_execution().is_some()
        })
        .collect();
    if approved.is_empty() {
        return Ok(0);
    }
    // Each workspace's work goes with its own token, or the default one
    // (invariant 32); each looked up once.
    let mut tokens: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    let mut done = 0;
    // Pushes first, so a pull request approved with its push goes after it.
    let (pushes, prs): (Vec<_>, Vec<_>) = approved
        .into_iter()
        .partition(|a| matches!(a.effect, Effect::PushBranch { .. }));
    for action in pushes.into_iter().chain(prs) {
        if let Effect::OpenPullRequest { dispatch, .. } = &action.effect
            && !pushed(system, dispatch)?
        {
            continue;
        }
        let project = match &action.effect {
            Effect::PushBranch { project, .. } => project.clone(),
            Effect::OpenPullRequest { dispatch, .. } => system
                .store
                .get_dispatch(dispatch)?
                .map(|d| d.project)
                .unwrap_or_default(),
            _ => continue,
        };
        let workspace = workspace_of_project(system, &project);
        if !tokens.contains_key(&workspace) {
            match token_for(&workspace) {
                Ok(t) => {
                    tokens.insert(workspace.clone(), t);
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "approved GitHub work waits: the GitHub token is in the keychain but this \
                         process may not read it; start the core from a terminal on the desktop, \
                         not over SSH or inside tmux"
                    );
                    return Ok(done);
                }
            }
        }
        let token = tokens.get(&workspace).cloned().flatten();
        let Some(token) = token else {
            tracing::info!(
                %workspace,
                "approved GitHub work waits: run `genatrix github login --workspace {workspace}` or `genatrix github login`"
            );
            continue;
        };
        let Some((action, version, exec_token)) = system.actions.take(
            &system.store,
            &system.ledger,
            &action.id,
            "handed to GitHub",
        )?
        else {
            continue;
        };
        let config = system.exec.config();
        let request = match &action.effect {
            Effect::PushBranch {
                account,
                project,
                dispatch,
                branch,
                tip,
                ..
            } => Request::Push {
                token: token.clone(),
                repo: account.trim_start_matches("github:").to_owned(),
                mirror: config.mirror(project).to_string_lossy().into_owned(),
                filed_as: format!("refs/genatrix/{}", dispatch.to_lowercase()),
                tip: tip.clone(),
                branch: branch.clone(),
            },
            Effect::OpenPullRequest {
                account,
                branch,
                base,
                ..
            } => {
                let (title, body) = version
                    .payload
                    .split_once('\n')
                    .unwrap_or((version.payload.as_str(), ""));
                Request::PullRequest {
                    token: token.clone(),
                    repo: account.trim_start_matches("github:").to_owned(),
                    head: branch.clone(),
                    base: base.clone(),
                    title: title.trim().to_owned(),
                    body: body.trim().to_owned(),
                }
            }
            _ => continue,
        };
        let mirrors = config.root.join("mirrors");
        let reply = match run(system, &mirrors, &request).await {
            Ok(r) => r,
            Err(e) => Reply::Failed {
                detail: format!("the GitHub connector could not run: {e}"),
            },
        };
        let status = match &reply {
            Reply::Done { .. } | Reply::Listed { .. } => Status::Executed { result: None },
            Reply::Refused { detail } | Reply::Failed { detail } => Status::Failed {
                detail: detail.clone(),
            },
            Reply::Unknown { detail } => Status::Unknown {
                detail: detail.clone(),
            },
        };
        system.actions.report(
            &system.store,
            &system.ledger,
            &action.id,
            exec_token.expose(),
            status,
            None,
        )?;
        if let (Reply::Done { detail }, Effect::OpenPullRequest { dispatch, .. }) =
            (&reply, &action.effect)
            && let Some(mut row) = system.store.get_dispatch(dispatch)?
        {
            row.detail = Some(format!("pull request: {detail}"));
            system.store.update_dispatch(&row)?;
        }
        done += 1;
    }
    Ok(done)
}

/// Whether a dispatch's push went out.
fn pushed(system: &System, dispatch: &str) -> anyhow::Result<bool> {
    Ok(system
        .actions
        .list(&system.store, Some("executed"), 500)?
        .iter()
        .any(|a| matches!(&a.effect, Effect::PushBranch { dispatch: d, .. } if d == dispatch)))
}

/// The push a dispatch may be offered again: its latest push failed, was
/// declined, or expired, and none is waiting, approved or done. A push
/// whose outcome is unknown is not offered again: it may have gone.
fn push_to_offer_again(
    system: &System,
    dispatch: &str,
) -> anyhow::Result<Result<genatrix_agent::action::Action, String>> {
    let mut pushes: Vec<_> = system
        .actions
        .list(&system.store, None, 1000)?
        .into_iter()
        .filter(|a| matches!(&a.effect, Effect::PushBranch { dispatch: d, .. } if d == dispatch))
        .collect();
    pushes.sort_by_key(|a| a.created_at);
    if let Some(live) = pushes.iter().find(|a| {
        matches!(
            a.status,
            Status::Pending
                | Status::Approved { .. }
                | Status::Executed { .. }
                | Status::Unknown { .. }
        )
    }) {
        return Ok(Err(match live.status {
            Status::Executed { .. } => "this work was pushed already".to_owned(),
            Status::Unknown { .. } => {
                "the last push may have gone; look on GitHub before pushing again".to_owned()
            }
            _ => "a push for this work is waiting already".to_owned(),
        }));
    }
    Ok(pushes
        .pop()
        .ok_or_else(|| "this work has no push to offer again".to_owned()))
}

/// Whether [`push_again`] would offer a push for this dispatch.
pub fn can_push_again(system: &System, dispatch: &str) -> bool {
    matches!(push_to_offer_again(system, dispatch), Ok(Ok(_)))
}

/// Offer a failed push again: the same commit to the same branch, as a new
/// action that waits for approval like the first. A pull request approved
/// with the first goes once this one is carried out.
pub fn push_again(
    system: &System,
    dispatch: &str,
) -> anyhow::Result<Result<genatrix_agent::action::Action, String>> {
    let last = match push_to_offer_again(system, dispatch)? {
        Ok(a) => a,
        Err(why) => return Ok(Err(why)),
    };
    let action = system.actions.propose_by_user(
        &system.store,
        &system.ledger,
        dispatch,
        last.effect.clone(),
        last.current().payload.clone(),
        last.rationale.clone(),
    )?;
    Ok(Ok(action))
}

/// Ask GitHub who a token belongs to, through the same sandboxed connector.
pub async fn whoami(system: &System, token: &str) -> anyhow::Result<String> {
    let mirrors = system.exec.config().root.join("mirrors");
    match run(
        system,
        &mirrors,
        &Request::Whoami {
            token: token.to_owned(),
        },
    )
    .await?
    {
        Reply::Done { detail } => Ok(detail),
        Reply::Listed { .. } => anyhow::bail!("GitHub answered out of turn"),
        Reply::Refused { detail } | Reply::Failed { detail } | Reply::Unknown { detail } => {
            anyhow::bail!(detail)
        }
    }
}

/// Who each stored token belongs to, by keychain account, as GitHub last
/// said; asked once and kept, and forgotten when the token changes.
#[derive(Default)]
pub struct Logins {
    known: std::sync::Mutex<std::collections::HashMap<String, Result<String, String>>>,
}

impl Logins {
    fn get(&self, account: &str) -> Option<Result<String, String>> {
        self.known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(account)
            .cloned()
    }

    fn set(&self, account: &str, login: Option<Result<String, String>>) {
        let mut map = self
            .known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match login {
            Some(l) => map.insert(account.to_owned(), l),
            None => map.remove(account),
        };
    }
}

/// Whose token a keychain account holds: `None` when there is none, else
/// the login, or why it could not be told.
pub async fn login_of(system: &System, account: &str) -> Option<Result<String, String>> {
    let token = match Keychain::named(SERVICE).read(account) {
        Ok(Some(t)) => t,
        Ok(None) => return None,
        Err(e) => {
            return Some(Err(format!(
                "the keychain would not give the token to this process ({e})"
            )));
        }
    };
    if let Some(known) = system.exec.logins.get(account) {
        return Some(known);
    }
    let said = whoami(system, &token).await.map_err(|e| e.to_string());
    system.exec.logins.set(account, Some(said.clone()));
    Some(said)
}

/// Whether a token is plain enough to be one: GitHub's are letters, digits
/// and underscores.
#[must_use]
pub fn token_is_plain(token: &str) -> bool {
    (20..=255).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

/// Keep a token for a workspace, or as the default with `None`, once GitHub
/// has said whose it is; the login.
pub async fn sign_in(
    system: &System,
    workspace: Option<&str>,
    token: &str,
) -> anyhow::Result<String> {
    let token = token.trim();
    if !token_is_plain(token) {
        anyhow::bail!("that does not look like a GitHub token");
    }
    let login = whoami(system, token).await?;
    let account = account_for(workspace);
    Keychain::named(SERVICE).store(&account, token)?;
    system.exec.logins.set(&account, Some(Ok(login.clone())));
    system.exec.repos.forget(workspace);
    Ok(login)
}

/// Take a workspace's own token away, or the default with `None`. Whether
/// there was one.
pub fn sign_out(system: &System, workspace: Option<&str>) -> anyhow::Result<bool> {
    let account = account_for(workspace);
    let had = Keychain::named(SERVICE).forget(&account)?;
    system.exec.logins.set(&account, None);
    system.exec.repos.forget(workspace);
    Ok(had)
}

/// How long what was read stays fresh: a page opened sooner shows it again
/// rather than asking GitHub.
pub const FRESH: std::time::Duration = std::time::Duration::from_secs(120);

/// What was last read from one repository, for one workspace's token.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// The last listing that came back, kept when a later read fails.
    pub listing: Option<genatrix_connector_github::list::Listing>,
    /// When it came back, in milliseconds.
    pub read_ms: Option<i64>,
    /// Why the last read did not work, if it did not.
    pub error: Option<String>,
    tried: Option<std::time::Instant>,
}

/// Each repository's last read, by workspace and repository. One read of a
/// repository at a time; different repositories at once.
#[derive(Default)]
pub struct Repos {
    entries: std::sync::Mutex<std::collections::HashMap<(String, String), Entry>>,
}

/// One repository's last read, locked while it is read again.
type Entry = Arc<tokio::sync::Mutex<Snapshot>>;

impl Repos {
    /// Forget what was read with a workspace's token, or with every token
    /// for `None`, so the next look reads with the new one.
    fn forget(&self, workspace: Option<&str>) {
        let mut map = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.retain(|(w, _), _| workspace.is_some_and(|ws| ws != w));
    }

    fn entry(&self, workspace: &str, repo: &str) -> Entry {
        let mut map = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(
            map.entry((workspace.to_owned(), repo.to_lowercase()))
                .or_default(),
        )
    }
}

/// A repository's open issues and pull requests, read again when what is
/// kept is older than [`FRESH`]. With the workspace's own token or the
/// default one (invariant 32); a read that fails keeps the last listing and
/// says why.
pub async fn listing(system: &System, workspace: &str, repo: &str) -> Snapshot {
    let entry = system.exec.repos.entry(workspace, repo);
    let mut snap = entry.lock().await;
    if snap.tried.is_some_and(|t| t.elapsed() < FRESH) {
        return snap.clone();
    }
    snap.tried = Some(std::time::Instant::now());
    let token = match token_for(workspace) {
        Ok(Some(t)) => t,
        Ok(None) => {
            snap.error = Some(format!(
                "no GitHub token; run `genatrix github login --workspace {workspace}`"
            ));
            return snap.clone();
        }
        Err(e) => {
            snap.error = Some(format!(
                "the GitHub token is in the keychain but this process may not read it ({e}); \
                 start the core from a terminal on the desktop"
            ));
            return snap.clone();
        }
    };
    let mirrors = system.exec.config().root.join("mirrors");
    let request = Request::List {
        token,
        repo: repo.to_owned(),
    };
    match run(system, &mirrors, &request).await {
        Ok(Reply::Listed { listing }) => {
            snap.listing = Some(listing);
            snap.read_ms = Some(chrono::Utc::now().timestamp_millis());
            snap.error = None;
        }
        Ok(Reply::Refused { detail } | Reply::Failed { detail } | Reply::Unknown { detail }) => {
            snap.error = Some(detail);
        }
        Ok(Reply::Done { .. }) => snap.error = Some("GitHub answered out of turn".into()),
        Err(e) => snap.error = Some(format!("the GitHub connector could not run: {e}")),
    }
    snap.clone()
}

async fn run(system: &System, mirrors: &Path, request: &Request) -> anyhow::Result<Reply> {
    let binary = crate::connectors::binary_for("github")
        .ok_or_else(|| anyhow::anyhow!("genatrix-github is not beside this program"))?;
    let run_dir = system.config.data_dir.join("run");
    let profile_path = run_dir.join("github.sb");
    std::fs::write(
        &profile_path,
        profile(&system.config.data_dir, mirrors, &binary)?,
    )?;
    let sandbox = Path::new("/usr/bin/sandbox-exec");
    let mut command = if sandbox.exists() {
        let mut c = tokio::process::Command::new(sandbox);
        c.arg("-f").arg(&profile_path).arg(&binary);
        c
    } else {
        tracing::warn!("no sandbox-exec on this system; the GitHub connector runs unconfined");
        tokio::process::Command::new(&binary)
    };
    let mut child = command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", std::env::var_os("HOME").unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdin"))?;
    stdin.write_all(&serde_json::to_vec(request)?).await?;
    drop(stdin);
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("the GitHub connector took too long"))??;
    Ok(serde_json::from_slice(&out.stdout)?)
}

/// The sandbox the GitHub connector runs in: out to port 443 and the
/// resolver only; no data directory, no keychain files; read the mirrors;
/// start no program but itself and git.
pub fn profile(data_dir: &Path, mirrors: &Path, binary: &Path) -> anyhow::Result<String> {
    let data = quoted(&data_dir.canonicalize()?.to_string_lossy());
    let binary = quoted(&binary.canonicalize()?.to_string_lossy());
    let mirrors = mirrors.canonicalize().map_or_else(
        |_| quoted(&mirrors.to_string_lossy()),
        |m| quoted(&m.to_string_lossy()),
    );
    let mut out = String::from(
        "(version 1)\n\
         ;; genatrix-github: out to HTTPS only; no data directory, no keychain files;\n\
         ;; no programs but itself and git.\n\
         (allow default)\n\
         (deny network*)\n\
         (allow network-outbound (remote ip \"*:443\"))\n\
         (allow network-outbound (remote unix-socket (path-literal \"/private/var/run/mDNSResponder\")))\n",
    );
    let _ = writeln!(out, "(deny file-read* file-write* (subpath {data}))");
    if let Some(home) = std::env::var_os("HOME") {
        let k = Path::new(&home).join("Library/Keychains");
        let _ = writeln!(
            out,
            "(deny file-read* file-write* (subpath {}))",
            quoted(&k.to_string_lossy())
        );
    }
    let _ = writeln!(out, "(deny file-write* (subpath {mirrors}))");
    let _ = writeln!(out, "(deny process-exec*)");
    let _ = writeln!(out, "(allow process-exec (literal {binary}))");
    for git in [
        "(literal \"/usr/bin/git\")",
        "(literal \"/usr/bin/xcrun\")",
        "(subpath \"/Library/Developer/CommandLineTools\")",
        "(subpath \"/Applications/Xcode.app/Contents/Developer\")",
    ] {
        let _ = writeln!(out, "(allow process-exec {git})");
    }
    Ok(out)
}

fn quoted(path: &str) -> String {
    let mut out = String::from("\"");
    for c in path.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_something_shaped_like_a_github_token_is_kept() {
        assert!(token_is_plain(
            "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyz"
        ));
        assert!(token_is_plain("ghp_0123456789abcdefghijABCDEFGHIJ012345"));
        assert!(!token_is_plain(""));
        assert!(!token_is_plain("short"));
        assert!(!token_is_plain("ghp_0123456789abcdefghij ABCDEFGHIJ012345"));
        assert!(!token_is_plain("ghp_0123456789abcdefghij;ABCDEFGHIJ012345"));
    }

    #[test]
    fn a_new_token_is_read_with_at_once() {
        let repos = Repos::default();
        let _ = repos.entry("dpt", "o/r");
        let _ = repos.entry("mine", "o/s");
        repos.forget(Some("dpt"));
        assert_eq!(repos.entries.lock().unwrap().len(), 1, "only dpt's");
        repos.forget(None);
        assert!(
            repos.entries.lock().unwrap().is_empty(),
            "the default may be anyone's"
        );
    }

    #[test]
    fn the_connector_reaches_https_and_nothing_of_ours() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("genatrix-github");
        std::fs::write(&bin, b"").unwrap();
        let text = profile(dir.path(), &dir.path().join("mirrors"), &bin).unwrap();
        assert!(text.contains("(deny network*)"));
        assert!(text.contains("*:443"));
        assert!(!text.contains("*:22\""), "no SSH");
        assert!(text.contains("(deny file-read* file-write* (subpath"));
        assert!(text.contains("Library/Keychains"));
        assert!(text.contains("(deny process-exec*)"));
        assert!(text.contains("/usr/bin/git"));
    }
}
