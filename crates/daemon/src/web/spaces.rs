//! Spaces on the page: one home per workspace (design 06 v0.14, design 14).
//!
//! A space's home is a view over the one base, not a part of it kept
//! apart: its open tasks are the same task conversations Chats shows, its
//! work is the same dispatches, its record is the same ledger. What it
//! adds is the cut: only what this workspace's accounts brought and its
//! projects did.
//!
//! Its projects' GitHub issues and pull requests are read when the page
//! asks, through the GitHub connector, and kept in memory (design 13 v0.7).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json};
use genatrix_dispatch::workspace::Workspace;
use genatrix_ledger::{EntryFilter, kind};
use serde::Serialize;

use super::access::Caller;
use super::api::{ApiError, Party, task_parties};
use super::work::{DispatchView, ProjectView};
use crate::system::System;

type Shared = State<Arc<System>>;

/// The routes.
pub fn routes() -> axum::Router<Arc<System>> {
    axum::Router::new()
        .route("/api/spaces", get(list))
        .route("/api/spaces/{name}", get(one))
        .route("/api/spaces/{name}/github", get(github))
}

/// A dispatch is going on: running, waiting for your answer, or answered
/// and about to go on.
fn active(state: &str) -> bool {
    matches!(state, "running" | "waiting" | "answered")
}

#[derive(Serialize)]
struct Card {
    name: String,
    title: String,
    accounts: usize,
    projects: usize,
    todo: usize,
    active: usize,
}

/// Every space, with what is waiting in it.
async fn list(State(system): Shared) -> Result<Json<Vec<Card>>, ApiError> {
    let defs = crate::executors::definitions(&system.store)?;
    let dispatches = system.store.list_dispatches(None, 500)?;
    let mut out = Vec::new();
    for (w, _) in &defs.workspaces {
        let projects: BTreeSet<&str> = defs
            .projects
            .iter()
            .filter(|(p, _)| p.workspace == w.name)
            .map(|(p, _)| p.name.as_str())
            .collect();
        out.push(Card {
            name: w.name.clone(),
            title: w.shown().to_owned(),
            accounts: w.accounts.len(),
            projects: projects.len(),
            todo: task_parties(&system, Some(&w.name))?
                .iter()
                .filter(|t| t.status == Some("todo"))
                .count(),
            active: dispatches
                .iter()
                .filter(|d| projects.contains(d.project.as_str()) && active(&d.state))
                .count(),
        });
    }
    Ok(Json(out))
}

#[derive(Serialize)]
struct AccountLine {
    /// `<kind>:<id>`, as the workspace file names it.
    key: String,
    kind: String,
    id: String,
    /// The sync state's name and the same in words; `missing` when the
    /// workspace names an account that is not set up.
    state: String,
    text: String,
}

/// One dispatch in the record, with what went out for it.
#[derive(Serialize)]
struct Done {
    #[serde(flatten)]
    dispatch: DispatchView,
    /// How many texts the gate let go to executors for it.
    sent: usize,
    /// To which executors.
    to: Vec<String>,
}

#[derive(Serialize)]
struct Home {
    name: String,
    title: String,
    accounts: Vec<AccountLine>,
    projects: Vec<ProjectView>,
    /// Its open tasks, as Chats shows them.
    todo: Vec<Party>,
    /// Work going on.
    active: Vec<DispatchView>,
    /// Work done, newest first.
    record: Vec<Done>,
    /// Approvals waiting on this space's work: drafts not yet approved, and
    /// pushes and pull requests its finished work proposed.
    approvals: usize,
    /// Whose GitHub token its work pushes with: `own`, `default` or `none`.
    github: &'static str,
    /// Whether the space can be changed from here: this machine, or an
    /// admin device (design 06 v0.7).
    manage: bool,
}

/// One space's home.
async fn one(
    State(system): Shared,
    Extension(caller): Extension<Caller>,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    let defs = crate::executors::definitions(&system.store)?;
    let Some((space, _)) = defs.workspaces.iter().find(|(w, _)| w.name == name) else {
        return Ok((
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no such space" })),
        )
            .into_response());
    };
    let projects: Vec<_> = defs
        .projects
        .iter()
        .filter(|(p, _)| p.workspace == space.name)
        .map(|(p, _)| p.clone())
        .collect();
    let names: BTreeSet<String> = projects.iter().map(|p| p.name.clone()).collect();
    let dispatches: Vec<_> = system
        .store
        .list_dispatches(None, 500)?
        .into_iter()
        .filter(|d| names.contains(&d.project))
        .collect();
    let sent = sent_by_dispatch(&system, &dispatches)?;
    let ids: BTreeSet<&str> = dispatches.iter().map(|d| d.id.as_str()).collect();
    let approvals = system
        .actions
        .list(&system.store, Some("pending"), 500)?
        .iter()
        .filter(|a| match &a.effect {
            genatrix_agent::Effect::Dispatch { project, .. } => names.contains(project),
            genatrix_agent::Effect::PushBranch { dispatch, .. }
            | genatrix_agent::Effect::OpenPullRequest { dispatch, .. } => {
                ids.contains(dispatch.as_str())
            }
            _ => false,
        })
        .count();
    let (going, ended): (Vec<_>, Vec<_>) = dispatches.into_iter().partition(|d| active(&d.state));
    let todo: Vec<Party> = task_parties(&system, Some(&space.name))?
        .into_iter()
        .filter(|t| t.status == Some("todo"))
        .collect();
    let manage = super::admin::can_manage(&system, &caller);
    Ok(Json(Home {
        name: space.name.clone(),
        title: space.shown().to_owned(),
        accounts: account_lines(&system, space),
        projects: projects.into_iter().map(Into::into).collect(),
        todo,
        active: going.into_iter().map(Into::into).collect(),
        record: ended
            .into_iter()
            .take(30)
            .map(|d| {
                let (count, to) = sent.get(&d.id).cloned().unwrap_or_default();
                Done {
                    sent: count,
                    to: to.into_iter().collect(),
                    dispatch: d.into(),
                }
            })
            .collect(),
        approvals,
        github: github_sign_in(&space.name),
        manage,
    })
    .into_response())
}

/// For each dispatch, how many texts went to executors and to which, from
/// the egress records the gate wrote.
fn sent_by_dispatch(
    system: &System,
    dispatches: &[genatrix_store::DispatchRow],
) -> anyhow::Result<BTreeMap<String, (usize, BTreeSet<String>)>> {
    let wanted: BTreeSet<&str> = dispatches.iter().map(|d| d.id.as_str()).collect();
    let mut out: BTreeMap<String, (usize, BTreeSet<String>)> = BTreeMap::new();
    if wanted.is_empty() {
        return Ok(out);
    }
    let entries = system.ledger.entries(&EntryFilter {
        kind: Some(kind::EGRESS.into()),
        limit: 10_000,
        ..Default::default()
    })?;
    for e in entries {
        let Ok(r) = serde_json::from_value::<genatrix_gate::gate::EgressRecord>(e.body) else {
            continue;
        };
        let genatrix_gate::gate::Initiator::Dispatch { dispatch, .. } = &r.initiator else {
            continue;
        };
        let went = matches!(
            r.decision,
            genatrix_gate::gate::Decision::Allowed | genatrix_gate::gate::Decision::AllowedRedacted
        );
        if !wanted.contains(dispatch.as_str()) || !went {
            continue;
        }
        let slot = out.entry(dispatch.clone()).or_default();
        slot.0 += 1;
        slot.1.insert(r.target.clone());
    }
    Ok(out)
}

/// Each of the space's accounts, with how it is doing.
fn account_lines(system: &System, space: &Workspace) -> Vec<AccountLine> {
    let states: BTreeMap<String, _> = system
        .accounts
        .snapshot()
        .into_iter()
        .map(|a| (a.address.clone(), a))
        .collect();
    space
        .accounts
        .iter()
        .map(|key| {
            let (kind, id) = key.split_once(':').unwrap_or(("", key));
            let (state, text) = states.get(id).map_or_else(
                || {
                    (
                        "missing".to_owned(),
                        "not set up on this computer".to_owned(),
                    )
                },
                |a| {
                    let name = serde_json::to_value(&a.sync)
                        .ok()
                        .and_then(|v| v.get("state").and_then(|s| s.as_str()).map(str::to_owned))
                        .unwrap_or_default();
                    (name, a.text.clone())
                },
            );
            AccountLine {
                key: key.clone(),
                kind: kind.to_owned(),
                id: id.to_owned(),
                state,
                text,
            }
        })
        .collect()
}

/// Which GitHub token this space's work would push with (invariant 32).
fn github_sign_in(space: &str) -> &'static str {
    let chain = crate::keychain::Keychain::named(crate::executors::github::SERVICE);
    let [own, default] = crate::executors::github::accounts_tried(space);
    if chain.read(&own).ok().flatten().is_some() {
        "own"
    } else if chain.read(&default).ok().flatten().is_some() {
        "default"
    } else {
        "none"
    }
}

/// A pull request, with the work it came from when Genatrix opened it.
#[derive(Debug, Serialize)]
pub(crate) struct PullView {
    #[serde(flatten)]
    pull: genatrix_connector_github::list::Pull,
    /// The dispatch whose branch it is, if any.
    dispatch: Option<String>,
}

/// Which dispatch each pull request came from: one whose branch it is, in
/// the repository itself, not a fork.
pub(crate) fn link_pulls(
    pulls: Vec<genatrix_connector_github::list::Pull>,
    dispatches: &[(String, String)],
) -> Vec<PullView> {
    pulls
        .into_iter()
        .map(|pull| {
            let dispatch = (!pull.from_fork)
                .then(|| {
                    dispatches
                        .iter()
                        .find(|(_, branch)| *branch == pull.head)
                        .map(|(id, _)| id.clone())
                })
                .flatten();
            PullView { pull, dispatch }
        })
        .collect()
}

#[derive(Serialize)]
struct RepoView {
    project: String,
    repo: String,
    /// What the work on an issue is said to come from.
    source_prefix: String,
    issues: Vec<genatrix_connector_github::list::Issue>,
    pulls: Vec<PullView>,
    more_issues: bool,
    more_pulls: bool,
    read_ms: Option<i64>,
    error: Option<String>,
}

/// A space's projects' open issues and pull requests, read again when
/// older than two minutes.
async fn github(State(system): Shared, Path(name): Path<String>) -> Result<Response, ApiError> {
    let defs = crate::executors::definitions(&system.store)?;
    if !defs.workspaces.iter().any(|(w, _)| w.name == name) {
        return Ok((
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no such space" })),
        )
            .into_response());
    }
    let projects: Vec<(String, String)> = defs
        .projects
        .iter()
        .filter(|(p, _)| p.workspace == name)
        .filter_map(|(p, _)| p.github.clone().map(|g| (p.name.clone(), g)))
        .collect();
    let names: BTreeSet<&str> = projects.iter().map(|(p, _)| p.as_str()).collect();
    let dispatches: Vec<(String, String)> = system
        .store
        .list_dispatches(None, 500)?
        .into_iter()
        .filter(|d| names.contains(d.project.as_str()))
        .map(|d| (d.id, d.branch))
        .collect();
    // Each repository read at once; a slow one does not hold the others.
    let mut reads = tokio::task::JoinSet::new();
    for (i, (_, repo)) in projects.iter().enumerate() {
        let (system, space, repo) = (Arc::clone(&system), name.clone(), repo.clone());
        reads.spawn(async move {
            (
                i,
                crate::executors::github::listing(&system, &space, &repo).await,
            )
        });
    }
    let mut snaps = vec![crate::executors::github::Snapshot::default(); projects.len()];
    while let Some(done) = reads.join_next().await {
        if let Ok((i, snap)) = done {
            snaps[i] = snap;
        }
    }
    let out: Vec<RepoView> = projects
        .into_iter()
        .zip(snaps)
        .map(|((project, repo), snap)| {
            let listing = snap.listing.unwrap_or_default();
            RepoView {
                source_prefix: crate::executors::issue_prefix(&repo),
                project,
                repo,
                issues: listing.issues,
                pulls: link_pulls(listing.pulls, &dispatches),
                more_issues: listing.more_issues,
                more_pulls: listing.more_pulls,
                read_ms: snap.read_ms,
                error: snap.error,
            }
        })
        .collect();
    Ok(Json(serde_json::json!({ "repos": out })).into_response())
}

#[cfg(test)]
mod tests {
    use genatrix_connector_github::list::Pull;

    use super::*;

    fn pull(head: &str, from_fork: bool) -> Pull {
        Pull {
            number: 1,
            title: "t".into(),
            url: "u".into(),
            author: "a".into(),
            state: "open".into(),
            draft: false,
            head: head.into(),
            from_fork,
            base: "main".into(),
            updated: String::new(),
        }
    }

    #[test]
    fn a_pull_request_on_a_dispatchs_branch_is_that_dispatchs_and_a_forks_is_not() {
        let dispatches = vec![("01ABC".to_owned(), "genatrix/01abc".to_owned())];
        let linked = link_pulls(
            vec![
                pull("genatrix/01abc", false),
                pull("genatrix/01abc", true),
                pull("feature", false),
            ],
            &dispatches,
        );
        assert_eq!(linked[0].dispatch.as_deref(), Some("01ABC"));
        assert_eq!(linked[1].dispatch, None, "a fork may name any branch");
        assert_eq!(linked[2].dispatch, None);
    }
}
