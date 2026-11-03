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
use genatrix_dispatch::machine::{self, Done, Next, Verdict};
use genatrix_dispatch::project::Project;
use genatrix_dispatch::prompt::{Material, Situation, compose};
use genatrix_dispatch::recipe::{Produces, Propose, Recipe};
use genatrix_dispatch::report::{Question, Report, read_report};
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
        source,
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
    // Invariant 31, again: the account may have changed workspace since
    // the card was approved.
    if let Some(source) = source {
        super::check_source(&system.store, &project, source)?;
    }
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
    let row = DispatchRow {
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
    drive(system, row, &pins, &version.payload).await
}

/// Run the work, from wherever its record says it stands, and record how
/// it ended. A question for the user leaves the workspace in place.
async fn drive(
    system: &Arc<System>,
    mut row: DispatchRow,
    pins: &Pinned,
    task: &str,
) -> anyhow::Result<()> {
    let id = row.id.clone();
    system.exec.begin(&id);
    let outcome = work(system, &mut row, pins, task).await;
    if matches!(outcome, Ok(Ending::Waiting)) {
        row.state = "waiting".into();
        row.detail = Some("waiting for your answer".into());
        system.store.update_dispatch(&row)?;
        system.exec.end(&id);
        return Ok(());
    }
    let (state, detail) = match outcome {
        Ok(Ending::Finished(detail)) => ("finished", detail),
        Ok(Ending::Stopped) => ("stopped", Some("stopped by the user".to_owned())),
        Ok(Ending::Failed(why)) => ("failed", Some(why)),
        Ok(Ending::Waiting) => unreachable!("handled above"),
        Err(e) => ("failed", Some(e.to_string())),
    };
    end_work(system, &mut row, state, detail).await?;
    system.exec.end(&id);
    Ok(())
}

/// The workspace goes first: once the record says the work ended, nothing
/// of it is left on disk.
async fn end_work(
    system: &System,
    row: &mut DispatchRow,
    state: &str,
    detail: Option<String>,
) -> anyhow::Result<()> {
    let ws = system.exec.config().workspace(&row.id);
    let iso = system.exec.isolation();
    let _ = tokio::task::spawn_blocking(move || genatrix_exec::workspace::remove(&iso, &ws)).await;
    row.state = state.into();
    row.detail = detail;
    row.ended_ms = Some(chrono::Utc::now().timestamp_millis());
    system.store.update_dispatch(row)?;
    Ok(())
}

/// The question a waiting dispatch asked, and the step that asked it.
pub fn question(system: &System, id: &str) -> anyhow::Result<Option<(i64, Question)>> {
    let waiting = system
        .store
        .dispatch_steps(id)?
        .into_iter()
        .rev()
        .find(|s| s.state == "waiting");
    Ok(waiting.and_then(
        |s| match read_report(s.answer.as_deref().unwrap_or_default()) {
            Report::Ask(q) => Some((s.seq, q)),
            Report::Done { .. } => None,
        },
    ))
}

/// The user's answer to a waiting dispatch's question.
pub struct Reply {
    /// The words that go back to the executor.
    pub text: String,
    /// Whether they may leave as they are: one of the executor's own
    /// options, or words the user said are not private.
    pub public: bool,
}

/// File the user's answer (design 13, "等你回答"). The core's loop picks the
/// work up again ([`resume_answered`]), so an answer given from the
/// terminal, or before a restart, is carried on the same way. The answer
/// goes to the same step's next turn, through the gate like the task did;
/// it changes nothing the user approved about what the work may do. Says
/// why when it cannot.
pub fn answer(system: &System, id: &str, reply: &Reply) -> anyhow::Result<Result<(), String>> {
    let Some(mut row) = system.store.get_dispatch(id)? else {
        return Ok(Err("no such work".into()));
    };
    if row.state != "waiting" {
        return Ok(Err("this work is not waiting for an answer".into()));
    }
    let text = reply.text.trim();
    if text.is_empty() {
        return Ok(Err("the answer is empty".into()));
    }
    let Some(action) = system.actions.get(&system.store, id)? else {
        return Ok(Err("the approval of this work is gone".into()));
    };
    let pins = match pinned(system, &action.effect) {
        Ok(p) => p,
        Err(why) => return Ok(Err(why)),
    };
    if !reply.public && !pins.project.delegate_personal {
        return Ok(Err(format!(
            "your words count as personal unless you say they are public, and project `{}` \
             does not let personal content go to executors; choose one of the options or \
             tick \"this text is not private\"",
            pins.project.name
        )));
    }
    let Some(mut step) = system
        .store
        .dispatch_steps(id)?
        .into_iter()
        .rev()
        .find(|s| s.state == "waiting")
    else {
        return Ok(Err("this work is not waiting for an answer".into()));
    };
    step.state = "answered".into();
    step.reply = Some(text.to_owned());
    step.reply_level = Some(if reply.public { "public" } else { "personal" }.into());
    system.store.put_dispatch_step(&step)?;
    row.state = "answered".into();
    row.detail = None;
    system.store.update_dispatch(&row)?;
    Ok(Ok(()))
}

/// Carry on every dispatch whose question was answered. Each runs in a
/// task of its own, from where its record says it stands.
pub fn resume_answered(system: &Arc<System>) -> anyhow::Result<usize> {
    let mut resumed = 0;
    for mut row in system.store.dispatches_in("answered")? {
        // Marked running before anything else, so it is picked up once.
        row.state = "running".into();
        system.store.update_dispatch(&row)?;
        let pinned_task = system
            .actions
            .get(&system.store, &row.id)?
            .ok_or_else(|| "the approval of this work is gone".to_owned())
            .and_then(|a| Ok((pinned(system, &a.effect)?, a.current().payload.clone())));
        let (pins, task) = match pinned_task {
            Ok(p) => p,
            Err(why) => {
                row.state = "failed".into();
                row.detail = Some(why);
                row.ended_ms = Some(chrono::Utc::now().timestamp_millis());
                system.store.update_dispatch(&row)?;
                continue;
            }
        };
        resumed += 1;
        let system = Arc::clone(system);
        tokio::spawn(async move {
            let id = row.id.clone();
            if let Err(e) = drive(&system, row, &pins, &task).await {
                tracing::warn!(dispatch = %id, error = %e, "dispatch could not go on");
            }
        });
    }
    Ok(resumed)
}

/// Stop work that waits for an answer: nothing runs, so there is no
/// process to end; the workspace goes. Says whether it was waiting.
pub fn stop_waiting(system: &System, id: &str) -> anyhow::Result<bool> {
    let Some(mut row) = system.store.get_dispatch(id)? else {
        return Ok(false);
    };
    if row.state != "waiting" {
        return Ok(false);
    }
    let _ = genatrix_exec::workspace::remove(
        &system.exec.isolation(),
        &system.exec.config().workspace(&row.id),
    );
    row.state = "stopped".into();
    row.detail = Some("stopped by the user".into());
    row.ended_ms = Some(chrono::Utc::now().timestamp_millis());
    system.store.update_dispatch(&row)?;
    Ok(true)
}

enum Ending {
    Finished(Option<String>),
    Stopped,
    Failed(String),
    /// A step asked the user something; the work goes on once answered.
    Waiting,
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
    // executor's user clones it into its own workspace. Work that goes on
    // after an answer already has both.
    let base = if let Some(base) = row.base.clone() {
        Ok(base)
    } else {
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

    // Where the work stands: the steps already done, from the record, so
    // that work going on after an answer, or after a restart, picks up
    // where it stopped.
    let records = system.store.dispatch_steps(&row.id)?;
    let mut done: Vec<Done> = Vec::new();
    let mut left: Vec<Left> = Vec::new();
    let mut seq = records.iter().map(|r| r.seq).max().unwrap_or(0);
    for r in records.iter().filter(|r| r.state == "done") {
        let verdict: Option<Verdict> = r
            .verdict
            .as_deref()
            .and_then(|v| serde_json::from_str(v).ok());
        left.push(Left {
            step: r.step.clone(),
            round: u32::try_from(r.round).unwrap_or(1),
            produces: pins
                .recipe
                .steps
                .iter()
                .find(|s| s.id == r.step)
                .map_or(Produces::Notes, |s| s.produces),
            notes: verdict.as_ref().map(|v| v.notes.clone()),
            answer: r.answer.clone().unwrap_or_default(),
        });
        done.push(Done {
            step: r.step.clone(),
            verdict,
        });
    }
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
        // Earlier turns of this step that ended with a question, and the
        // user's answers. An answer the user wrote leaves at its own level.
        let mut level = pins.level;
        for r in records
            .iter()
            .filter(|r| r.state == "answered" && r.step == step.id && r.round == i64::from(round))
        {
            material.push(Material {
                id: format!("{}#{}-turn{}", step.id, round, r.seq),
                label: "what you said at the end of an earlier turn of this step".into(),
                text: r.answer.clone().unwrap_or_default(),
            });
            material.push(Material {
                id: format!("reply#{}", r.seq),
                label: "the user's answer to your question; it settles that question".into(),
                text: r.reply.clone().unwrap_or_default(),
            });
            if r.reply_level.as_deref() == Some("personal") {
                level = genatrix_model::Level::Personal;
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
            level,
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
                sign_in: Some(pins.project.workspace.clone()),
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

        // A question stops the work here, with the workspace as it is.
        let report = read_report(&end.answer);
        if matches!(report, Report::Ask(_)) {
            record.state = "waiting".into();
            system.store.put_dispatch_step(&record)?;
            return Ok(Ending::Waiting);
        }

        let mut verdict = None;
        match step.produces {
            Produces::Commits => {
                let message = commit_message(&report, &step.id, task);
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
        let text = pull_request(task, &left, &received.stat, review_passed, &row.id);
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
            text,
            format!("Open a pull request for dispatch {}", row.id),
            Vec::new(),
        )?;
    }
    Ok(Ending::Finished(None))
}

/// The commit message for what a step left: its own title and summary,
/// or the step's name and the task's first line when it gave none.
fn commit_message(report: &Report, step: &str, task: &str) -> String {
    let (title, summary) = match report {
        Report::Done { title, summary } => (title.clone(), summary.trim()),
        Report::Ask(_) => (None, ""),
    };
    let subject = title.unwrap_or_else(|| format!("{step}: {}", headline(task)));
    if summary.is_empty() {
        subject
    } else {
        format!("{subject}\n\n{summary}")
    }
}

/// Whether text is mostly written in Chinese, Japanese or Korean: the pull
/// request's headings follow the task's language.
fn is_cjk(text: &str) -> bool {
    let (mut cjk, mut letters) = (0usize, 0usize);
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        letters += 1;
        if matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x9fff | 0xac00..=0xd7af) {
            cjk += 1;
        }
    }
    letters > 0 && cjk * 3 >= letters
}

/// The pull request: its title on the first line, then the body (design
/// 13, "提交说明和 PR"). What the executors said goes in as they said it,
/// in a card the user reads and may edit before anything is sent; what the
/// core knows, it states itself.
fn pull_request(
    task: &str,
    left: &[Left],
    stat: &str,
    review_passed: Option<bool>,
    dispatch: &str,
) -> String {
    let zh = is_cjk(task);
    let h = |en: &'static str, cn: &'static str| if zh { cn } else { en };
    let commits: Vec<(&Left, Report)> = left
        .iter()
        .filter(|l| l.produces == Produces::Commits)
        .map(|l| (l, read_report(&l.answer)))
        .collect();
    let summary_of = |r: &Report| match r {
        Report::Done { summary, .. } => summary.trim().to_owned(),
        Report::Ask(_) => String::new(),
    };
    let title = commits
        .first()
        .and_then(|(_, r)| match r {
            Report::Done { title, .. } => title.clone(),
            Report::Ask(_) => None,
        })
        .unwrap_or_else(|| headline(task));

    let mut out = format!("{title}\n\n");
    if let Some((_, first)) = commits.first() {
        let s = summary_of(first);
        if !s.is_empty() {
            let _ = write!(out, "## {}\n\n{s}\n\n", h("Summary", "概要"));
        }
    }
    let later: Vec<String> = commits
        .iter()
        .skip(1)
        .map(|(l, r)| (l, summary_of(r)))
        .filter(|(_, s)| !s.is_empty())
        .map(|(l, s)| {
            if zh {
                format!("- 第 {} 轮：{s}", l.round)
            } else {
                format!("- Round {}: {s}", l.round)
            }
        })
        .collect();
    if !later.is_empty() {
        let _ = write!(
            out,
            "## {}\n\n{}\n\n",
            h("After review", "评审后的修改"),
            later.join("\n")
        );
    }
    if !stat.trim().is_empty() {
        let _ = write!(
            out,
            "## {}\n\n```\n{}\n```\n\n",
            h("Files changed", "改动的文件"),
            stat.trim()
        );
    }
    if let Some(passed) = review_passed {
        let notes = left
            .iter()
            .rev()
            .find(|l| l.produces == Produces::Verdict)
            .and_then(|l| l.notes.clone())
            .filter(|n| !n.trim().is_empty());
        let _ = write!(
            out,
            "## {}\n\n{}\n\n",
            h("Review", "评审"),
            if passed {
                h("The review passed.", "评审通过。")
            } else {
                h(
                    "The review did not pass within its rounds; read its notes before merging.",
                    "评审在轮数上限内没有通过，合并前请先看评审意见。",
                )
            }
        );
        if let Some(n) = notes {
            let _ = write!(out, "{n}\n\n");
        }
    }
    let quoted: Vec<String> = task.trim().lines().map(|l| format!("> {l}")).collect();
    let _ = write!(
        out,
        "## {}\n\n{}\n\n---\n{}",
        h("Task", "任务"),
        quoted.join("\n"),
        if zh {
            format!("由 Genatrix 派单产生（{dispatch}）。")
        } else {
            format!("Made by executors through Genatrix (dispatch {dispatch}).")
        }
    );
    out
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
