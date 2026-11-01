//! The git work around a session: code in through a mirror, commits out as a
//! bundle.
//!
//! Design: `docs/design/13-executors.md`, "本机隔离", ruling 3. The executor
//! never sees the user's working tree, its remotes or their credentials.
//! The core keeps a mirror of the project's repository that the executor's
//! user may read; the executor's user clones it into a workspace of its own;
//! when the work is done the new commits leave as a bundle, which the core
//! checks and files in the mirror under a ref of its own. Pushing is a later
//! action, done by a connector with the user's credentials.
//!
//! Every call here is blocking; the core runs them off its async threads.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use crate::isolation::Isolation;

/// Something git said no to.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct GitError(pub String);

/// What a bundle brought back, once checked and filed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Received {
    /// The newest commit.
    pub tip: String,
    /// Each commit since the base, newest first: id and subject.
    pub commits: Vec<(String, String)>,
    /// `git diff --stat` from the base to the tip.
    pub stat: String,
}

fn run(isolation: &Isolation, words: &[&str], dir: Option<&Path>) -> Result<Output, GitError> {
    let mut line = vec!["/usr/bin/git".to_owned()];
    if let Some(dir) = dir {
        line.push("-C".to_owned());
        line.push(dir.to_string_lossy().into_owned());
    }
    line.extend(words.iter().map(|a| (*a).to_owned()));
    let argv = isolation.wrap(&line);
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| GitError("no git".into()))?;
    let mut command = Command::new(program);
    command
        .args(rest)
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0");
    command
        .output()
        .map_err(|e| GitError(format!("could not run git: {e}")))
}

fn ok(isolation: &Isolation, args: &[&str], dir: Option<&Path>) -> Result<String, GitError> {
    let out = run(isolation, args, dir)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        Err(GitError(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// Bring the mirror up to date with the user's repository, creating it the
/// first time, and let `group` read it. Run as the user.
pub fn refresh_mirror(repo: &Path, mirror: &Path, group: Option<&str>) -> Result<(), GitError> {
    let me = Isolation::SameUserForTests;
    if mirror.exists() {
        // Branches and tags only: the mirror's own `refs/genatrix/*` stay.
        ok(
            &me,
            &[
                "fetch",
                "--quiet",
                "--prune",
                "origin",
                "+refs/heads/*:refs/heads/*",
                "+refs/tags/*:refs/tags/*",
            ],
            Some(mirror),
        )?;
    } else {
        if let Some(parent) = mirror.parent() {
            std::fs::create_dir_all(parent).map_err(|e| GitError(e.to_string()))?;
        }
        ok(
            &me,
            &[
                "clone",
                "--quiet",
                "--bare",
                &repo.to_string_lossy(),
                &mirror.to_string_lossy(),
            ],
            None,
        )?;
        ok(
            &me,
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/heads/*",
            ],
            Some(mirror),
        )?;
    }
    if let Some(group) = group {
        let path = mirror.to_string_lossy().into_owned();
        for (program, args) in [
            ("/usr/bin/chgrp", vec!["-R", group, &path]),
            ("/bin/chmod", vec!["-R", "g+rX,o-rwx", &path]),
        ] {
            let status = Command::new(program)
                .args(&args)
                .status()
                .map_err(|e| GitError(e.to_string()))?;
            if !status.success() {
                return Err(GitError(format!(
                    "could not let the executor's group read the mirror ({program})"
                )));
            }
        }
    }
    Ok(())
}

/// Clone the mirror into a fresh workspace on a new branch, as the
/// executor's user. Returns the commit the work starts from.
pub fn prepare(
    isolation: &Isolation,
    mirror: &Path,
    workspace: &Path,
    base: &str,
    branch: &str,
) -> Result<String, GitError> {
    let safe = format!("safe.directory={}", mirror.to_string_lossy());
    ok(
        isolation,
        &[
            "-c",
            &safe,
            "clone",
            "--quiet",
            "--no-hardlinks",
            "--branch",
            base,
            &mirror.to_string_lossy(),
            &workspace.to_string_lossy(),
        ],
        None,
    )?;
    ok(
        isolation,
        &["switch", "--quiet", "-c", branch],
        Some(workspace),
    )?;
    ok(
        isolation,
        &["config", "user.name", "Genatrix executor"],
        Some(workspace),
    )?;
    ok(
        isolation,
        &["config", "user.email", "executor@genatrix.invalid"],
        Some(workspace),
    )?;
    // The executor has no business with where the code came from.
    ok(isolation, &["remote", "remove", "origin"], Some(workspace))?;
    ok(isolation, &["rev-parse", "HEAD"], Some(workspace))
}

/// Commit whatever the executor left in the workspace. `None` when it left
/// nothing.
pub fn commit_all(
    isolation: &Isolation,
    workspace: &Path,
    message: &str,
) -> Result<Option<String>, GitError> {
    ok(isolation, &["add", "--all"], Some(workspace))?;
    let staged = run(isolation, &["diff", "--cached", "--quiet"], Some(workspace))?;
    if staged.status.success() {
        return Ok(None);
    }
    ok(
        isolation,
        &["commit", "--quiet", "--no-verify", "-m", message],
        Some(workspace),
    )?;
    ok(isolation, &["rev-parse", "HEAD"], Some(workspace)).map(Some)
}

/// The commits since `base`, as a bundle. `None` when there are none.
pub fn bundle(
    isolation: &Isolation,
    workspace: &Path,
    base: &str,
    cap_bytes: usize,
) -> Result<Option<Vec<u8>>, GitError> {
    let count = ok(
        isolation,
        &["rev-list", "--count", &format!("{base}..HEAD")],
        Some(workspace),
    )?;
    if count.trim() == "0" {
        return Ok(None);
    }
    let out = run(
        isolation,
        &["bundle", "create", "--quiet", "-", &format!("{base}..HEAD")],
        Some(workspace),
    )?;
    if !out.status.success() {
        return Err(GitError(format!(
            "git bundle failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    if out.stdout.len() > cap_bytes {
        return Err(GitError(format!(
            "the work is {} MB, over this project's limit of {} MB",
            out.stdout.len() / (1024 * 1024),
            cap_bytes / (1024 * 1024)
        )));
    }
    Ok(Some(out.stdout))
}

/// Check a bundle against the mirror and file its commits under
/// `refs/genatrix/<id>`. Run as the user. A bundle that does not apply on
/// top of `base`, or carries anything but that one line of history, is
/// refused.
pub fn receive(mirror: &Path, bytes: &[u8], id: &str, base: &str) -> Result<Received, GitError> {
    let me = Isolation::SameUserForTests;
    let file = tempfile_in(mirror, bytes)?;
    let path = file.to_string_lossy().into_owned();
    let result = (|| {
        ok(&me, &["bundle", "verify", "--quiet", &path], Some(mirror))?;
        let heads = ok(&me, &["bundle", "list-heads", &path], Some(mirror))?;
        let tip = heads
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().next())
            .filter(|t| t.len() == 40 && t.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| GitError("the bundle names no commit".into()))?
            .to_owned();
        let filed = format!("refs/genatrix/{id}");
        ok(
            &me,
            &["fetch", "--quiet", &path, &format!("{tip}:{filed}")],
            Some(mirror),
        )?;
        ok(
            &me,
            &["merge-base", "--is-ancestor", base, &tip],
            Some(mirror),
        )
        .map_err(|_| GitError("the work does not start from the commit it was given".into()))?;
        let log = ok(
            &me,
            &["log", "--format=%H%x09%s", &format!("{base}..{tip}")],
            Some(mirror),
        )?;
        let commits = log
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(h, s)| (h.to_owned(), s.to_owned()))
            .collect();
        let stat = ok(&me, &["diff", "--stat", base, &tip], Some(mirror))?;
        Ok(Received { tip, commits, stat })
    })();
    let _ = std::fs::remove_file(&file);
    result
}

/// The full diff of filed work, for the page.
pub fn diff(mirror: &Path, base: &str, tip: &str) -> Result<String, GitError> {
    ok(
        &Isolation::SameUserForTests,
        &["diff", "--no-color", base, tip],
        Some(mirror),
    )
}

/// Remove a workspace, as its owner.
pub fn remove(isolation: &Isolation, workspace: &Path) -> Result<(), GitError> {
    let argv = isolation.wrap(&[
        "/bin/rm".to_owned(),
        "-rf".to_owned(),
        workspace.to_string_lossy().into_owned(),
    ]);
    let (program, rest) = argv.split_first().ok_or_else(|| GitError("no rm".into()))?;
    let status = Command::new(program)
        .args(rest)
        .status()
        .map_err(|e| GitError(e.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(GitError("could not remove the workspace".into()))
    }
}

fn tempfile_in(dir: &Path, bytes: &[u8]) -> Result<std::path::PathBuf, GitError> {
    let name = format!(
        "incoming-{}-{}.bundle",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let path = dir.join(name);
    std::fs::write(&path, bytes).map_err(|e| GitError(e.to_string()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("/usr/bin/git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn repo(root: &Path) -> std::path::PathBuf {
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet", "-b", "main"]);
        git(&repo, &["config", "user.name", "u"]);
        git(&repo, &["config", "user.email", "u@example.com"]);
        std::fs::write(repo.join("README"), "hello\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "first"]);
        // Something in the user's own repository the executor must not see.
        git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://user:token@example.com/x.git",
            ],
        );
        repo
    }

    /// The whole way round: mirror, workspace, commit, bundle, filed back.
    #[test]
    fn work_goes_out_through_a_mirror_and_comes_back_as_a_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let mirror = dir.path().join("mirrors/p.git");
        refresh_mirror(&repo, &mirror, None).unwrap();
        let me = Isolation::SameUserForTests;
        let ws = dir.path().join("exec/d1");
        let base = prepare(&me, &mirror, &ws, "main", "genatrix/d1").unwrap();
        let remotes = ok(&me, &["remote", "-v"], Some(&ws)).unwrap();
        assert!(
            !remotes.contains("token"),
            "the workspace knows no remotes: {remotes}"
        );

        assert_eq!(commit_all(&me, &ws, "nothing").unwrap(), None);
        assert_eq!(bundle(&me, &ws, &base, 1 << 20).unwrap(), None);

        std::fs::write(ws.join("NEW"), "made by the executor\n").unwrap();
        let tip = commit_all(&me, &ws, "implement: add NEW").unwrap().unwrap();
        let bytes = bundle(&me, &ws, &base, 1 << 20).unwrap().unwrap();
        assert!(bundle(&me, &ws, &base, 10).is_err(), "the cap holds");

        // The user's repository moves on meanwhile; the mirror follows and
        // keeps what was filed.
        let got = receive(&mirror, &bytes, "d1", &base).unwrap();
        assert_eq!(got.tip, tip);
        assert_eq!(got.commits.len(), 1);
        assert_eq!(got.commits[0].1, "implement: add NEW");
        assert!(got.stat.contains("NEW"));
        std::fs::write(repo.join("README"), "hello again\n").unwrap();
        git(&repo, &["commit", "--quiet", "-am", "second"]);
        refresh_mirror(&repo, &mirror, None).unwrap();
        let kept = ok(&me, &["rev-parse", "refs/genatrix/d1"], Some(&mirror)).unwrap();
        assert_eq!(kept, tip);
        assert!(
            diff(&mirror, &base, &tip)
                .unwrap()
                .contains("made by the executor")
        );
    }

    #[test]
    fn a_bundle_from_elsewhere_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path());
        let mirror = dir.path().join("m.git");
        refresh_mirror(&repo, &mirror, None).unwrap();
        let base = ok(
            &Isolation::SameUserForTests,
            &["rev-parse", "HEAD"],
            Some(&repo),
        )
        .unwrap();
        assert!(receive(&mirror, b"not a bundle", "x", &base).is_err());
        // History that does not grow from the base.
        let other = dir.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        git(&other, &["init", "--quiet", "-b", "main"]);
        git(&other, &["config", "user.name", "u"]);
        git(&other, &["config", "user.email", "u@example.com"]);
        git(&other, &["commit", "--quiet", "--allow-empty", "-m", "a"]);
        let a = ok(
            &Isolation::SameUserForTests,
            &["rev-parse", "HEAD"],
            Some(&other),
        )
        .unwrap();
        git(&other, &["commit", "--quiet", "--allow-empty", "-m", "b"]);
        let out = Command::new("/usr/bin/git")
            .arg("-C")
            .arg(&other)
            .args(["bundle", "create", "--quiet", "-", &format!("{a}..HEAD")])
            .output()
            .unwrap();
        assert!(receive(&mirror, &out.stdout, "y", &base).is_err());
    }
}
