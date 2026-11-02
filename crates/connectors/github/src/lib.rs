//! The GitHub connector: carries approved executor work out.
//!
//! Design: `docs/design/13-executors.md`, "第三层其实是老规矩：对外效果";
//! `docs/design/05-connectors.md`, "动作执行".
//!
//! Executors never hold the user's GitHub credentials. When the user
//! approves a push, the core starts this program for that one action, in a
//! sandbox that lets it reach port 443 and nothing else, hands it the token
//! on its standard input, and reads the result from its standard output.
//! It pushes the approved commit, checked again against the mirror first,
//! or opens the approved pull request; then it exits. It never retries:
//! a push or a pull request made twice is worse than one that failed.
//!
//! One-shot rather than a long-running connector like mail: there is
//! nothing to keep in sync yet, and a process that lives for one action
//! holds the token for one action.

pub mod git;
pub mod rest;

use serde::{Deserialize, Serialize};

/// What the core asks for.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Who the token belongs to; for signing in.
    Whoami {
        /// The token.
        token: String,
    },
    /// Push the approved commit to a branch.
    Push {
        /// The token.
        token: String,
        /// `owner/name`.
        repo: String,
        /// The mirror the commit is filed in.
        mirror: String,
        /// The ref it is filed under.
        filed_as: String,
        /// The approved commit.
        tip: String,
        /// The branch to push to.
        branch: String,
    },
    /// Open a pull request.
    PullRequest {
        /// The token.
        token: String,
        /// `owner/name`.
        repo: String,
        /// The branch with the work.
        head: String,
        /// The branch to merge into.
        base: String,
        /// Title.
        title: String,
        /// Description.
        body: String,
    },
}

/// What came of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Reply {
    /// Done; `detail` is a login or a link.
    Done {
        /// What to show.
        detail: String,
    },
    /// Refused before anything was sent.
    Refused {
        /// Why.
        detail: String,
    },
    /// It went wrong, and nothing was made.
    Failed {
        /// Why.
        detail: String,
    },
    /// It may or may not have reached GitHub: the connection broke after
    /// the request went. Look before asking again.
    Unknown {
        /// What happened.
        detail: String,
    },
}

/// Carry out one request.
#[must_use]
pub fn handle(request: Request) -> Reply {
    match request {
        Request::Whoami { token } => match rest::whoami(&token) {
            Ok(login) => Reply::Done { detail: login },
            Err(e) => Reply::Failed {
                detail: e.to_string(),
            },
        },
        Request::Push {
            token,
            repo,
            mirror,
            filed_as,
            tip,
            branch,
        } => git::push(&token, &repo, &mirror, &filed_as, &tip, &branch),
        Request::PullRequest {
            token,
            repo,
            head,
            base,
            title,
            body,
        } => match rest::pull_request(&token, &repo, &head, &base, &title, &body) {
            Ok(url) => Reply::Done { detail: url },
            Err(rest::RestError::Unsure(detail)) => Reply::Unknown { detail },
            Err(e) => Reply::Failed {
                detail: e.to_string(),
            },
        },
    }
}

/// `owner/name`, plainly.
#[must_use]
pub fn valid_repo(repo: &str) -> bool {
    let parts: Vec<&str> = repo.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && !p.starts_with('.')
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}
