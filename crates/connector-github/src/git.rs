//! Pushing an approved commit.
//!
//! The commit is checked again here, at the last moment: the ref the core
//! filed the work under must still point at exactly the approved commit.
//! The token reaches git through its environment as an HTTP header for
//! github.com only, never on a command line, where any process could read
//! it, and never in a file.

use std::process::{Command, Stdio};

use base64::Engine as _;

use crate::{Reply, valid_repo};

fn git(mirror: &str, args: &[&str]) -> Command {
    let mut c = Command::new("/usr/bin/git");
    c.arg("-C")
        .arg(mirror)
        .args(args)
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    c
}

/// Push `tip` to `branch` on `repo`, if `filed_as` in the mirror is `tip`.
#[must_use]
pub fn push(
    token: &str,
    repo: &str,
    mirror: &str,
    filed_as: &str,
    tip: &str,
    branch: &str,
) -> Reply {
    if !valid_repo(repo) {
        return Reply::Refused {
            detail: format!("`{repo}` is not owner/name"),
        };
    }
    if !branch.starts_with("genatrix/") || branch.contains("..") || branch.contains(' ') {
        return Reply::Refused {
            detail: "only a branch of Genatrix's own is pushed".into(),
        };
    }
    if !filed_as.starts_with("refs/genatrix/") {
        return Reply::Refused {
            detail: "the work is not filed where Genatrix files it".into(),
        };
    }
    let safe = format!("safe.directory={mirror}");
    let filed = git(mirror, &["-c", &safe, "rev-parse", "--verify", filed_as]).output();
    match filed {
        Ok(out) if out.status.success() => {
            let found = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if found != tip {
                return Reply::Refused {
                    detail: format!(
                        "the filed work is now {found}, not the approved {tip}; nothing was pushed"
                    ),
                };
            }
        }
        _ => {
            return Reply::Refused {
                detail: "the approved work is not in the mirror; nothing was pushed".into(),
            };
        }
    }
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
    let url = format!("https://github.com/{repo}.git");
    let out = git(
        mirror,
        &[
            "-c",
            &safe,
            "push",
            "--quiet",
            "--porcelain",
            &url,
            &format!("{tip}:refs/heads/{branch}"),
        ],
    )
    .env("GIT_CONFIG_COUNT", "1")
    .env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraheader")
    .env(
        "GIT_CONFIG_VALUE_0",
        format!("Authorization: Basic {basic}"),
    )
    .output();
    match out {
        Ok(o) if o.status.success() => Reply::Done {
            detail: format!("https://github.com/{repo}/tree/{branch}"),
        },
        Ok(o) => Reply::Failed {
            detail: format!(
                "git push failed: {}",
                String::from_utf8_lossy(&o.stderr)
                    .trim()
                    .replace(token, "***")
            ),
        },
        Err(e) => Reply::Failed {
            detail: format!("could not run git: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &std::path::Path, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// Design 13, proposed invariant 24: what is pushed is the commit that
    /// was approved, checked at the last moment; anything else is refused
    /// before the network is touched.
    #[test]
    fn only_the_approved_commit_is_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let m = dir.path().join("m.git");
        std::fs::create_dir_all(&m).unwrap();
        run(&m, &["init", "--quiet", "--bare"]);
        let work = dir.path().join("w");
        std::fs::create_dir_all(&work).unwrap();
        run(&work, &["init", "--quiet", "-b", "main"]);
        run(
            &work,
            &[
                "-c",
                "user.name=u",
                "-c",
                "user.email=u@e",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "a",
            ],
        );
        let a = run(&work, &["rev-parse", "HEAD"]);
        run(
            &work,
            &[
                "-c",
                "user.name=u",
                "-c",
                "user.email=u@e",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "b",
            ],
        );
        let b = run(&work, &["rev-parse", "HEAD"]);
        run(
            &work,
            &[
                "push",
                "--quiet",
                m.to_str().unwrap(),
                &format!("{b}:refs/genatrix/d1"),
            ],
        );
        let mirror = m.to_str().unwrap();

        let refused = |r: Reply| matches!(r, Reply::Refused { .. });
        assert!(
            refused(push(
                "t",
                "o/r",
                mirror,
                "refs/genatrix/d1",
                &a,
                "genatrix/d1"
            )),
            "a moved ref"
        );
        assert!(refused(push(
            "t",
            "o/r",
            mirror,
            "refs/genatrix/none",
            &b,
            "genatrix/d1"
        )));
        assert!(refused(push(
            "t",
            "o/r",
            mirror,
            "refs/heads/main",
            &b,
            "genatrix/d1"
        )));
        assert!(
            refused(push("t", "o/r", mirror, "refs/genatrix/d1", &b, "main")),
            "not our branch"
        );
        assert!(refused(push(
            "t",
            "a/b/c",
            mirror,
            "refs/genatrix/d1",
            &b,
            "genatrix/d1"
        )));
    }
}
