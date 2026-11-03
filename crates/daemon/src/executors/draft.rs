//! A dispatch as a pending action (design 13, "第一层：派单").
//!
//! The card is the whole of what the user approves: the task in their own
//! words, which is the text that will go to the executors, the project, the
//! recipe's steps with who does each and whose service that is, and the
//! level the text goes at. Everything that would make the work impossible
//! is refused here, so the card never offers what cannot run.

use std::collections::BTreeSet;

use genatrix_agent::action::{Action, Effect};

use crate::system::System;

/// What the user asked for.
#[derive(Clone, Debug)]
pub struct Request {
    /// The project's name.
    pub project: String,
    /// The recipe's name.
    pub recipe: String,
    /// The task, in the user's words.
    pub task: String,
    /// Whether the user says the task is public. Their words are personal
    /// unless they say so; only they can lower a level (design 02).
    pub public: bool,
    /// The thread the work comes from, such as a Lark task, if any.
    pub source: Option<String>,
}

/// Why a draft was refused.
#[derive(Debug, thiserror::Error)]
pub enum DraftError {
    /// Something about the request; the words are for the page.
    #[error("{0}")]
    Refused(String),
    /// Something broke.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

fn refused<T>(why: impl Into<String>) -> Result<T, DraftError> {
    Err(DraftError::Refused(why.into()))
}

/// Draft a dispatch: checked, then filed as a pending action written by the
/// user.
pub fn draft(system: &System, request: &Request) -> Result<Action, DraftError> {
    let task = request.task.trim();
    if task.is_empty() {
        return refused("say what the work is");
    }
    if task.len() > 20_000 {
        return refused(
            "the task is too long; put the details in the repository and point to them",
        );
    }
    let defs = super::definitions(&system.store)?;
    let Some((project, project_hash)) = defs
        .projects
        .iter()
        .find(|(p, _)| p.name == request.project)
    else {
        return refused(format!("there is no project `{}`", request.project));
    };
    let Some((recipe, recipe_hash)) = defs.recipes.iter().find(|(r, _)| r.name == request.recipe)
    else {
        return refused(format!("there is no recipe `{}`", request.recipe));
    };
    let roles: Vec<_> = defs.roles.iter().map(|(r, _)| r.clone()).collect();
    if let Err(e) = recipe.fits(project, &roles) {
        return refused(e.to_string());
    }
    // Invariant 31: what came from an account goes only to its own
    // workspace's executors.
    if let Some(source) = &request.source
        && let Err(why) = super::check_source(&system.store, project, source)
    {
        return refused(why);
    }
    if !recipe.finish.propose.is_empty() && project.github.is_none() {
        // Not refused: the commits stay in the mirror. Said on the card.
        tracing::info!(project = %project.name, "no GitHub repository; nothing will be proposed");
    }
    // The gate's one judgement on bytes, made now so the card is honest.
    if system.gate.level_of_typed_text(task) == genatrix_model::Level::Secret {
        return refused(
            "the task contains what looks like a code, a password or an account number, and \
             secret content never goes to an executor; take it out",
        );
    }
    if !request.public && !project.delegate_personal {
        return refused(format!(
            "your words count as personal unless you say they are public, and project `{}` \
             does not let personal content go to executors; tick \"this text is not private\" \
             if it is not",
            project.name
        ));
    }
    let config = system.exec.config();
    let mut executors = Vec::new();
    let mut role_names = BTreeSet::new();
    for step in &recipe.steps {
        let Some(e) = config.executor(&step.executor) else {
            return refused(format!(
                "executor `{}` is not set up in executors.toml",
                step.executor
            ));
        };
        if !executors
            .iter()
            .any(|(n, _): &(String, String)| n == &e.name)
        {
            executors.push((e.name.clone(), e.vendor.clone()));
        }
        role_names.insert(step.role.clone());
    }
    let roles = role_names
        .into_iter()
        .filter_map(|name| {
            defs.roles
                .iter()
                .find(|(r, _)| r.name == name)
                .map(|(_, hash)| (name, hash.clone()))
        })
        .collect();
    let effect = Effect::Dispatch {
        project: project.name.clone(),
        project_hash: project_hash.clone(),
        recipe: recipe.name.clone(),
        recipe_hash: recipe_hash.clone(),
        roles,
        level: if request.public { "public" } else { "personal" }.to_owned(),
        executors,
        source: request.source.clone(),
    };
    Ok(system.actions.propose_by_user(
        &system.store,
        &system.ledger,
        "dispatch",
        effect,
        task.to_owned(),
        recipe.title.clone(),
    )?)
}

/// One labelled line on a card for executor work.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Fact {
    /// What it is.
    pub label: String,
    /// The value.
    pub value: String,
}

/// What a card for executor work shows above its draft: where the work
/// goes, who does each step, whose service sees the text and the code.
#[must_use]
#[allow(clippy::too_many_lines)] // one card per kind, side by side
pub fn facts(system: &System, effect: &Effect) -> Vec<Fact> {
    let fact = |label: &str, value: String| Fact {
        label: label.to_owned(),
        value,
    };
    match effect {
        Effect::Dispatch {
            project,
            recipe_hash,
            executors,
            level,
            ..
        } => {
            let defs = super::definitions(&system.store).ok();
            let space = defs.as_ref().and_then(|d| {
                d.projects
                    .iter()
                    .find(|(p, _)| &p.name == project)
                    .and_then(|(p, _)| d.workspaces.iter().find(|(w, _)| w.name == p.workspace))
                    .map(|(w, _)| w.shown().to_owned())
            });
            let mut out = vec![fact("Project", project.clone())];
            if let Some(space) = space {
                out.push(fact("Workspace", space));
            }
            let recipe = system
                .store
                .definition("recipe", recipe_hash)
                .ok()
                .flatten()
                .and_then(|d| genatrix_dispatch::recipe::Recipe::parse(&d.text).ok());
            if let Some(r) = recipe {
                let steps: Vec<String> = r
                    .steps
                    .iter()
                    .map(|s| format!("{} ({}, {})", s.id, s.role, s.executor))
                    .collect();
                out.push(fact("Steps", steps.join(" → ")));
                if let Some(l) = &r.repeat {
                    out.push(fact(
                        "Loop",
                        format!(
                            "{} ↔ {}, at most {} rounds",
                            l.between[0], l.between[1], l.max
                        ),
                    ));
                }
                let proposes: Vec<&str> = r
                    .finish
                    .propose
                    .iter()
                    .map(|p| match p {
                        genatrix_dispatch::recipe::Propose::PushBranch => "push a branch",
                        genatrix_dispatch::recipe::Propose::OpenPullRequest => {
                            "open a pull request"
                        }
                    })
                    .collect();
                if !proposes.is_empty() {
                    out.push(fact(
                        "Then",
                        format!(
                            "propose to {}, each waiting for you",
                            proposes.join(" and ")
                        ),
                    ));
                }
            }
            let vendors: Vec<String> = executors
                .iter()
                .map(|(n, v)| format!("{n} → {v}"))
                .collect();
            out.push(fact("Your words and the code go to", vendors.join(", ")));
            out.push(fact(
                "Sent as",
                if level == "public" {
                    "public: as written".to_owned()
                } else {
                    "personal: names and numbers replaced".to_owned()
                },
            ));
            out
        }
        Effect::PushBranch {
            account,
            branch,
            base,
            tip,
            dispatch,
            ..
        } => vec![
            fact(
                "Repository",
                account.trim_start_matches("github:").to_owned(),
            ),
            fact("Branch", branch.clone()),
            fact("From", base.chars().take(10).collect()),
            fact("Commit", tip.chars().take(10).collect()),
            fact("Work", dispatch.clone()),
        ],
        Effect::OpenPullRequest {
            account,
            branch,
            base,
            dispatch,
            ..
        } => vec![
            fact(
                "Repository",
                account.trim_start_matches("github:").to_owned(),
            ),
            fact("From", branch.clone()),
            fact("Into", base.clone()),
            fact("Work", dispatch.clone()),
        ],
        _ => Vec::new(),
    }
}

/// Mark a pending dispatch's task public, or personal again: a new version,
/// so an approval given before does not carry over (design 03).
pub fn set_level(system: &System, id: &str, public: bool) -> Result<Action, DraftError> {
    let Some(mut action) = system.actions.get(&system.store, id)? else {
        return refused("no such action");
    };
    let Effect::Dispatch { level, project, .. } = &mut action.effect else {
        return refused("only a dispatch has a level to set");
    };
    let defs = super::definitions(&system.store)?;
    let allows_personal = defs
        .projects
        .iter()
        .find(|(p, _)| &p.name == project)
        .is_some_and(|(p, _)| p.delegate_personal);
    if !public && !allows_personal {
        return refused("this project does not let personal content go to executors");
    }
    if public { "public" } else { "personal" }.clone_into(level);
    let payload = action.current().payload.clone();
    action
        .edit(payload)
        .map_err(|e| DraftError::Refused(e.to_string()))?;
    system.actions.save_changed(
        &system.store,
        &system.ledger,
        &action,
        if public {
            "marked public"
        } else {
            "marked personal"
        },
    )?;
    Ok(action)
}
