//! An approved dispatch, carried out (design 13, "流程模板", "派单就是
//! 出境", "记录").
//!
//! The action is executed the moment the work starts: approving it
//! approved starting this work, and a dispatch runs far longer than an
//! action waits for its report. From then on the dispatch's own record says
//! how it goes.
//!
//! Step by step, as the recipe pinned by hash says and nothing else: the
//! text for each step is composed from the role's prompt and the material,
//! put through the gate, which records it and signs a ticket for exactly
//! those bytes, and handed to the host. What the executor says comes back
//! as events, kept at the dispatch's level, never as items: it is not the
//! user's life, it is untrusted text, and it is never evidence for
//! anything. When the steps are done, the commits leave the workspace as a
//! bundle, are filed in the mirror, and the push and the pull request are
//! proposed, each waiting for the user.

use std::fmt::Write as _;
use std::sync::Arc;

use genatrix_agent::action::{Action, Effect, ExecutionToken, Status, Version};
use genatrix_dispatch::machine::{self, Done, Next};
use genatrix_dispatch::project::Project;
use genatrix_dispatch::prompt::{Material, Situation, compose};
use genatrix_dispatch::recipe::{Produces, Propose, Recipe};
use genatrix_dispatch::role::Role;
use genatrix_dispatch::scope::{Answer, Scope};
use genatrix_exec::wire::{Executor, FromHost, Start};
use genatrix_gate::gate::{Delegation, GateError, Initiator};
use genatrix_store::{DispatchRow, DispatchStep};

use crate::system::System;

/// Start every approved dispatch that nothing has started yet. Each runs in
/// a task of its own; this returns once they have started.
pub fn start_approved(system: &Arc<System>) -> anyhow::Result<usize> {
    let mut started = 0;
    for action in system.actions.list(&system.store, Some("approved"), 200)? {
        if !matches!(action.effect, Effect::Dispatch { .. }) {
            continue;
        }
        let Some((action, version, token)) =
            system
                .actions
                .take(&system.store, &system.ledger, &action.id, "work started")?
        else {
            continue;
        };
        started += 1;
        let system = Arc::clone(system);
        tokio::spawn(async move {
            let id = action.id.clone();
            if let Err(e) = carry_out(&system, action, version, token).await {
                tracing::warn!(dispatch = %id, error = %e, "dispatch could not run");
            }
        });
    }
    Ok(started)
}

/// The definitions a dispatch was approved with.
struct Pinned {
    project: Project,
    recipe: Recipe,
    roles: Vec<Role>,
    level: genatrix_model::Level,
    executors: Vec<(String, String)>,
}

fn pinned(system: &System, effect: &Effect) -> Result<Pinned, String> {
    let Effect::Dispatch {
        project_hash,
        recipe_hash,
        roles,
        level,
        executors,
        ..
    } = effect
    else {
        return Err("not a dispatch".into());
    };
    let text = |kind: &str, hash: &str| -> Result<String, String> {
        system
            .store
            .definition(kind, hash)
            .map_err(|e| e.to_string())?
            .map(|d| d.text)
            .ok_or_else(|| format!("the {kind} this was approved with is no longer on file"))
    };
    let project = Project::parse(&text("project", project_hash)?).map_err(|e| e.to_string())?;
    let recipe = Recipe::parse(&text("recipe", recipe_hash)?).map_err(|e| e.to_string())?;
    let mut role_defs = Vec::new();
    for (_, hash) in roles {
        role_defs.push(Role::parse(&text("role", hash)?).map_err(|e| e.to_string())?);
    }
    let level = match level.as_str() {
        "public" => genatrix_model::Level::Public,
        "personal" => genatrix_model::Level::Personal,
        other => return Err(format!("level `{other}` cannot be sent")),
    };
    Ok(Pinned {
        project,
        recipe,
        roles: role_defs,
        level,
        executors: executors.clone(),
    })
}

/// Report the action, then run the work.
async fn carry_out(
    system: &Arc<System>,
    action: Action,
    version: Version,
    token: ExecutionToken,
) -> anyhow::Result<()> {
    let id = action.id.clone();
    let pins = match pinned(system, &action.effect) {
        Ok(p) => p,
        Err(detail) => {
            system.actions.report(
                &system.store,
                &system.ledger,
                &id,
                token.expose(),
                Status::Failed { detail },
                None,
            )?;
            return Ok(());
        }
    };
    system.actions.report(
        &system.store,
        &system.ledger,
        &id,
        token.expose(),
        Status::Executed { result: None },
        None,
    )?;
    let now = chrono::Utc::now().timestamp_millis();
    let mut row = DispatchRow {
        id: id.clone(),
        project: pins.project.name.clone(),
        recipe: pins.recipe.name.clone(),
        recipe_hash: match &action.effect {
            Effect::Dispatch { recipe_hash, .. } => recipe_hash.clone(),
            _ => String::new(),
        },
        state: "running".into(),
        level: pins.level.as_str().to_owned(),
        branch: format!("genatrix/{}", id.to_lowercase()),
        base: None,
        tip: None,
        review_passed: None,
        detail: None,
        started_ms: now,
        ended_ms: None,
    };
    system.store.insert_dispatch(&row)?;
    system.exec.begin(&id);
    let outcome = work(system, &mut row, &pins, &version.payload).await;
    let (state, detail) = match outcome {
        Ok(Ending::Finished(detail)) => ("finished", detail),
        Ok(Ending::Stopped) => ("stopped", Some("stopped by the user".to_owned())),
        Ok(Ending::Failed(why)) => ("failed", Some(why)),
        Err(e) => ("failed", Some(e.to_string())),
    };
    // The workspace goes first: once the record says the work ended,
    // nothing of it is left on disk.
    let ws = system.exec.config().workspace(&id);
    let iso = system.exec.isolation();
    let _ = tokio::task::spawn_blocking(move || genatrix_exec::workspace::remove(&iso, &ws)).await;
    row.state = state.into();
    row.detail = detail;
    row.ended_ms = Some(chrono::Utc::now().timestamp_millis());
    system.store.update_dispatch(&row)?;
    system.exec.end(&id);
    Ok(())
}

enum Ending {
    Finished(Option<String>),
    Stopped,
    Failed(String),
}

/// What one finished step left.
struct Left {
    step: String,
    round: u32,
    produces: Produces,
    answer: String,
    notes: Option<String>,
}

#[allow(clippy::too_many_lines)] // one dispatch, in order, reads best whole
async fn work(
    system: &Arc<System>,
    row: &mut DispatchRow,
    pins: &Pinned,
    task: &str,
) -> anyhow::Result<Ending> {
    let config = system.exec.config();
    let iso = system.exec.isolation();
    let mirror = config.mirror(&pins.project.name);
    let ws = config.workspace(&row.id);

    if let genatrix_exec::isolation::Isolation::User { name } = &iso
        && !super::terminal::user_exists_named(name)
    {
        return Ok(Ending::Failed(format!(
            "the executor's system user `{name}` does not exist yet; run `genatrix exec setup` \
             on the computer Genatrix runs on"
        )));
    }
    // Code in: the mirror follows the user's repository, then the
    // executor's user clones it into its own workspace.
    let base = {
        let repo = super::repo_path(&pins.project);
        let (mirror, ws, iso, base_branch, branch) = (
            mirror.clone(),
            ws.clone(),
            iso.clone(),
            pins.project.base.clone(),
            row.branch.clone(),
        );
        let group = matches!(iso, genatrix_exec::isolation::Isolation::User { .. })
            .then_some(genatrix_exec::isolation::EXEC_GROUP);
        tokio::task::spawn_blocking(move || -> Result<String, String> {
            genatrix_exec::workspace::refresh_mirror(&repo, &mirror, group)
                .map_err(|e| e.to_string())?;
            if ws.exists() {
                let _ = genatrix_exec::workspace::remove(&iso, &ws);
            }
            genatrix_exec::workspace::prepare(&iso, &mirror, &ws, &base_branch, &branch)
                .map_err(|e| e.to_string())
        })
        .await?
    };
    let base = match base {
        Ok(b) => b,
        Err(e) => {
            return Ok(Ending::Failed(format!(
                "the workspace could not be prepared: {e}"
            )));
        }
    };
    row.base = Some(base.clone());
    system.store.update_dispatch(row)?;

    let mut done: Vec<Done> = Vec::new();
    let mut left: Vec<Left> = Vec::new();
    let mut seq = 0i64;
    let review_passed = loop {
        if system.exec.stopped(&row.id) {
            return Ok(Ending::Stopped);
        }
        let (index, round) = match machine::next(&pins.recipe, &done) {
            Next::Run { step, round } => (step, round),
            Next::Finish { review_passed } => break review_passed,
            Next::Stop { reason } => return Ok(Ending::Failed(reason)),
        };
        let step = &pins.recipe.steps[index];
        let Some(role) = pins.roles.iter().find(|r| r.name == step.role) else {
            return Ok(Ending::Failed(format!(
                "role `{}` was not pinned",
                step.role
            )));
        };
        let Some(entry) = config.executor(&step.executor) else {
            return Ok(Ending::Failed(format!(
                "executor `{}` is no longer set up",
                step.executor
            )));
        };
        let vendor = pins
            .executors
            .iter()
            .find(|(n, _)| n == &entry.name)
            .map(|(_, v)| v.clone());
        if vendor.as_deref() != Some(entry.vendor.as_str()) {
            // The card named a vendor; a changed executors.toml does not
            // send the work somewhere else.
            return Ok(Ending::Failed(format!(
                "executor `{}` now sends to `{}`, not the service that was approved",
                entry.name, entry.vendor
            )));
        }
        seq += 1;

        // The text: the role, the step, the task and what earlier steps
        // left, each in its envelope.
        let mut material = vec![Material {
            id: "task".into(),
            label: "the task, as the user approved it".into(),
            text: task.to_owned(),
        }];
        for input in &step.inputs {
            if let Some(l) = left.iter().rev().find(|l| &l.step == input) {
                material.push(Material {
                    id: format!("{}#{}", l.step, l.round),
                    label: match l.produces {
                        Produces::Verdict => "a review of the work so far".into(),
                        Produces::Notes => format!("what step `{}` wrote", l.step),
                        Produces::Commits => {
                            format!("what step `{}` said about its changes", l.step)
                        }
                    },
                    text: l.notes.clone().unwrap_or_else(|| l.answer.clone()),
                });
            }
        }
        let text = compose(
            role,
            step,
            &Situation {
                branch: &row.branch,
                base_commit: &base,
                commands: &pins.project.commands,
                round,
            },
            &material,
        );

        let mut record = DispatchStep {
            dispatch_id: row.id.clone(),
            seq,
            step: step.id.clone(),
            round: i64::from(round),
            role: role.name.clone(),
            executor: entry.name.clone(),
            state: "running".into(),
            started_ms: chrono::Utc::now().timestamp_millis(),
            ..Default::default()
        };
        system.store.put_dispatch_step(&record)?;

        // Through the gate: written down, then a ticket for these bytes.
        let prepared = system.gate.prepare_delegation(&Delegation {
            executor: entry.name.clone(),
            vendor: entry.vendor.clone(),
            initiator: Initiator::Dispatch {
                dispatch: row.id.clone(),
                step: step.id.clone(),
            },
            items: Vec::new(),
            level: pins.level,
            personal_allowed: pins.project.delegate_personal,
            identities: Vec::new(),
            text,
        });
        let prepared = match prepared {
            Ok(p) => p,
            Err(GateError::DelegationRefused { reason, .. }) => {
                record.state = "refused".into();
                record.error = Some(reason.clone());
                record.ended_ms = Some(chrono::Utc::now().timestamp_millis());
                system.store.put_dispatch_step(&record)?;
                return Ok(Ending::Failed(format!("step `{}`: {reason}", step.id)));
            }
            Err(e) => return Err(e.into()),
        };
        record.egress_id = Some(prepared.egress_id.clone());
        system.store.put_dispatch_step(&record)?;

        let start = Start {
            executor: Executor {
                name: entry.name.clone(),
                command: entry.command.clone(),
                path: Some(entry.path.clone()),
            },
            isolation: iso.clone(),
            scope: Scope {
                workspace: ws.to_string_lossy().into_owned(),
                commands: pins.project.commands.clone(),
                may: role.may.clone(),
                network: pins.project.network,
            },
            text: String::from_utf8_lossy(&prepared.body).into_owned(),
            ticket: prepared.ticket,
            key: system.gate.delegation_key().to_hex(),
            timeout_secs: config.timeout_minutes.saturating_mul(60),
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
        if !system.exec.arm(&row.id, cancel_tx) {
            return Ok(Ending::Stopped);
        }
        let keeper = {
            let system = Arc::clone(system);
            let id = row.id.clone();
            tokio::spawn(async move {
                while let Some(frame) = rx.recv().await {
                    let (kind, text) = match frame {
                        FromHost::Event(e) => (e.kind, e.text),
                        FromHost::Asked { ask, answer } => {
                            let what = ask.command.clone().unwrap_or_else(|| ask.paths.join(", "));
                            let kind = serde_json::to_value(ask.kind)
                                .ok()
                                .and_then(|v| v.as_str().map(str::to_owned))
                                .unwrap_or_default();
                            let said = match answer {
                                Answer::Allow => format!("allowed {kind} {what}"),
                                Answer::Deny(why) => format!("refused {kind} {what}: {why}"),
                            };
                            ("asked".to_owned(), said)
                        }
                        FromHost::End(_) => continue,
                    };
                    let _ = system.store.add_dispatch_event(
                        &id,
                        seq,
                        chrono::Utc::now().timestamp_millis(),
                        &kind,
                        &text,
                    );
                }
            })
        };
        let end = system.exec.host().session(start, tx, cancel_rx).await;
        let _ = keeper.await;

        record.stop = Some(end.stop.clone());
        record.answer = Some(end.answer.clone());
        record.error.clone_from(&end.error);
        record.ended_ms = Some(chrono::Utc::now().timestamp_millis());
        if system.exec.stopped(&row.id) {
            record.state = "cancelled".into();
            system.store.put_dispatch_step(&record)?;
            return Ok(Ending::Stopped);
        }
        if end.stop != "end_turn" {
            record.state = "failed".into();
            system.store.put_dispatch_step(&record)?;
            let why = end
                .error
                .unwrap_or_else(|| format!("the executor stopped: {}", end.stop));
            return Ok(Ending::Failed(format!("step `{}`: {why}", step.id)));
        }

        let mut verdict = None;
        match step.produces {
            Produces::Commits => {
                let message = format!("{}: {}", step.id, headline(task));
                let (iso, ws) = (iso.clone(), ws.clone());
                let commit = tokio::task::spawn_blocking(move || {
                    genatrix_exec::workspace::commit_all(&iso, &ws, &message)
                })
                .await?;
                match commit {
                    Ok(c) => record.commit_id = c,
                    Err(e) => {
                        record.state = "failed".into();
                        record.error = Some(e.to_string());
                        system.store.put_dispatch_step(&record)?;
                        return Ok(Ending::Failed(format!("step `{}`: {e}", step.id)));
                    }
                }
            }
            Produces::Verdict => {
                verdict = machine::read_verdict(&end.answer);
                record.verdict = verdict.as_ref().and_then(|v| serde_json::to_string(v).ok());
            }
            Produces::Notes => {}
        }
        record.state = "done".into();
        system.store.put_dispatch_step(&record)?;
        left.push(Left {
            step: step.id.clone(),
            round,
            produces: step.produces,
            notes: verdict.as_ref().map(|v| v.notes.clone()),
            answer: end.answer,
        });
        done.push(Done {
            step: step.id.clone(),
            verdict,
        });
    };
    row.review_passed = review_passed;

    // Commits out: a bundle, checked and filed in the mirror.
    let cap = usize::try_from(pins.project.bundle_cap_mb).unwrap_or(20) * 1024 * 1024;
    let bundle = {
        let (iso, ws, base) = (iso.clone(), ws.clone(), base.clone());
        tokio::task::spawn_blocking(move || genatrix_exec::workspace::bundle(&iso, &ws, &base, cap))
            .await?
    };
    let bytes = match bundle {
        Ok(Some(b)) => b,
        Ok(None) => {
            return Ok(Ending::Finished(Some(
                "no commits; the steps' answers are the result".into(),
            )));
        }
        Err(e) => return Ok(Ending::Failed(e.to_string())),
    };
    let received = {
        let (mirror, id, base) = (mirror.clone(), row.id.to_lowercase(), base.clone());
        tokio::task::spawn_blocking(move || {
            genatrix_exec::workspace::receive(&mirror, &bytes, &id, &base)
        })
        .await?
    };
    let received = match received {
        Ok(r) => r,
        Err(e) => return Ok(Ending::Failed(format!("the work came back unusable: {e}"))),
    };
    row.tip = Some(received.tip.clone());
    system.store.update_dispatch(row)?;

    let Some(github) = pins.project.github.clone() else {
        return Ok(Ending::Finished(Some(format!(
            "this project names no GitHub repository; the commits are kept in the mirror as \
             refs/genatrix/{}",
            row.id.to_lowercase()
        ))));
    };
    let account = format!("github:{github}");
    let summary = summary(&received, review_passed);
    if pins.recipe.finish.propose.contains(&Propose::PushBranch) {
        system.actions.propose(
            &system.store,
            &system.ledger,
            &row.id,
            Effect::PushBranch {
                account: account.clone(),
                project: pins.project.name.clone(),
                dispatch: row.id.clone(),
                branch: row.branch.clone(),
                base: base.clone(),
                tip: received.tip.clone(),
            },
            summary.clone(),
            format!("Push the work of dispatch {} to {github}", row.id),
            Vec::new(),
        )?;
    }
    if pins
        .recipe
        .finish
        .propose
        .contains(&Propose::OpenPullRequest)
    {
        let body = format!(
            "{}\n\n{}\n\n---\n{}\n\nMade by executors through Genatrix (dispatch {}).",
            task.trim(),
            match review_passed {
                Some(true) => "The review passed.".to_owned(),
                Some(false) => "The review did not pass within its rounds; read its notes below \
                                before merging."
                    .to_owned(),
                None => String::new(),
            },
            left.iter()
                .rev()
                .find(|l| l.produces == Produces::Verdict)
                .and_then(|l| l.notes.clone())
                .filter(|n| !n.trim().is_empty())
                .map(|n| format!("Review notes:\n\n{n}"))
                .unwrap_or_default(),
            row.id
        );
        system.actions.propose(
            &system.store,
            &system.ledger,
            &row.id,
            Effect::OpenPullRequest {
                account,
                project: pins.project.name.clone(),
                dispatch: row.id.clone(),
                branch: row.branch.clone(),
                base: pins.project.base.clone(),
            },
            format!("{}\n\n{body}", headline(task)),
            format!("Open a pull request for dispatch {}", row.id),
            Vec::new(),
        )?;
    }
    Ok(Ending::Finished(None))
}

/// The first line of the task, short enough for a commit subject.
#[must_use]
pub fn headline(task: &str) -> String {
    let line = task
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("work");
    if line.chars().count() <= 72 {
        line.to_owned()
    } else {
        let cut: String = line.chars().take(71).collect();
        format!("{cut}…")
    }
}

fn summary(received: &genatrix_exec::workspace::Received, review_passed: Option<bool>) -> String {
    let mut out = String::new();
    for (hash, subject) in &received.commits {
        let _ = writeln!(out, "{} {subject}", &hash[..hash.len().min(10)]);
    }
    out.push('\n');
    out.push_str(&received.stat);
    match review_passed {
        Some(true) => out.push_str("\n\nThe review passed."),
        Some(false) => out.push_str("\n\nThe review did not pass within its rounds."),
        None => {}
    }
    out
}
