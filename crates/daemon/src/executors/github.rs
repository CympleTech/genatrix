//! Approved pushes and pull requests, carried out by `genatrix-github`
//! (design 13, "第三层其实是老规矩：对外效果").
//!
//! The token lives in the keychain. For each approved action the core reads
//! it, starts the connector for that one action under a sandbox that allows
//! port 443 and nothing else, and hands the token over its standard input.
//! A pull request waits, approved, until its branch has been pushed; it is
//! not taken before, so its approval is not spent on a branch that is not
//! there.

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
/// The account within it.
pub const ACCOUNT: &str = "token";

/// The token, if the user signed in.
pub fn token() -> Option<String> {
    Keychain::named(SERVICE).read(ACCOUNT).ok().flatten()
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
    let Some(token) = token() else {
        tracing::info!("approved GitHub work waits: run `genatrix github login`");
        return Ok(0);
    };
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
            Reply::Done { .. } => Status::Executed { result: None },
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
        Reply::Refused { detail } | Reply::Failed { detail } | Reply::Unknown { detail } => {
            anyhow::bail!(detail)
        }
    }
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
