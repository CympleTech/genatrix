//! A recipe: the steps of one piece of work.
//!
//! Design: `docs/design/13-executors.md`, "流程模板". A recipe is all the
//! steps that will happen. The model may pick a recipe and fill in the task;
//! it cannot add a step, change an executor or raise the loop's bound while
//! the work runs (proposed invariant 26). The dispatch pins the recipe by
//! hash, so a recipe edited later does not change work already approved.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::project::Project;
use crate::role::Role;
use crate::{DefinitionError, check_name};

/// The most rounds a loop may have. Two executors can talk each other round
/// in circles forever; the bound is what stops them.
pub const MAX_ROUNDS: u32 = 5;

/// What a step leaves behind for the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Produces {
    /// Commits in the workspace.
    Commits,
    /// A verdict: pass, or changes with notes. Read from a fixed JSON shape.
    Verdict,
    /// Its closing words: a specification, a plan, a report.
    Notes,
}

/// One step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Its name within the recipe.
    pub id: String,
    /// The role it is done in.
    pub role: String,
    /// The executor that does it.
    pub executor: String,
    /// What it leaves behind.
    pub produces: Produces,
    /// Earlier steps whose output this one is given, besides the task.
    #[serde(default)]
    pub inputs: Vec<String>,
}

/// A bounded loop between a verdict step and the step that answers it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Loop {
    /// The verdict step, then the step that acts on its notes. The second
    /// comes right after the first.
    pub between: [String; 2],
    /// Fix rounds at most; after the last, the verdict stands as it is.
    pub max: u32,
}

/// An outward effect proposed when the work is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Propose {
    /// Push the commits to a branch of their own.
    PushBranch,
    /// Open a pull request from that branch.
    OpenPullRequest,
}

/// What happens at the end.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finish {
    /// Effects to propose, each one an action that waits for the user.
    #[serde(default)]
    pub propose: Vec<Propose>,
}

/// A recipe, as written in its TOML file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// Its name.
    pub name: String,
    /// One line for the interface.
    pub title: String,
    /// The steps, in order.
    #[serde(rename = "step")]
    pub steps: Vec<Step>,
    /// At most one loop.
    #[serde(default, rename = "loop")]
    pub repeat: Option<Loop>,
    /// What is proposed at the end.
    #[serde(default)]
    pub finish: Finish,
}

impl Recipe {
    /// Read and check a recipe file.
    pub fn parse(text: &str) -> Result<Self, DefinitionError> {
        let recipe: Self = toml::from_str(text)?;
        recipe.validate()?;
        Ok(recipe)
    }

    /// Check what the schema cannot.
    pub fn validate(&self) -> Result<(), DefinitionError> {
        let bad = |m: String| Err(DefinitionError::Invalid(m));
        check_name("recipe", &self.name)?;
        if self.steps.is_empty() {
            return bad(format!("recipe `{}` has no steps", self.name));
        }
        if self.steps.len() > 12 {
            return bad(format!("recipe `{}` has more than 12 steps", self.name));
        }
        let mut seen = BTreeSet::new();
        for step in &self.steps {
            check_name("step", &step.id)?;
            check_name("role", &step.role)?;
            check_name("executor", &step.executor)?;
            for input in &step.inputs {
                if !seen.contains(input.as_str()) {
                    return bad(format!(
                        "step `{}` takes `{input}`, which is not an earlier step",
                        step.id
                    ));
                }
            }
            if !seen.insert(step.id.as_str()) {
                return bad(format!("step `{}` appears twice", step.id));
            }
        }
        if let Some(l) = &self.repeat {
            let (Some(a), Some(b)) = (self.index_of(&l.between[0]), self.index_of(&l.between[1]))
            else {
                return bad("the loop names a step that does not exist".into());
            };
            if b != a + 1 {
                return bad("the loop's second step must come right after its first".into());
            }
            if self.steps[a].produces != Produces::Verdict {
                return bad(format!(
                    "the loop starts at `{}`, which gives no verdict",
                    self.steps[a].id
                ));
            }
            if l.max == 0 || l.max > MAX_ROUNDS {
                return bad(format!("the loop's max must be 1 to {MAX_ROUNDS}"));
            }
        }
        let p = &self.finish.propose;
        if p.contains(&Propose::OpenPullRequest) && !p.contains(&Propose::PushBranch) {
            return bad("a pull request needs the branch pushed first".into());
        }
        if !p.is_empty() && !self.steps.iter().any(|s| s.produces == Produces::Commits) {
            return bad("nothing to push: no step produces commits".into());
        }
        Ok(())
    }

    /// Where a step is.
    #[must_use]
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.id == id)
    }

    /// Check the recipe against a project and the roles there are: every
    /// executor allowed on the project, every role known. Done when a
    /// dispatch is drafted, so a card never offers work that cannot run.
    pub fn fits(&self, project: &Project, roles: &[Role]) -> Result<(), DefinitionError> {
        for step in &self.steps {
            if !project.allows(&step.executor) {
                return Err(DefinitionError::Invalid(format!(
                    "step `{}` wants `{}`, which project `{}` does not allow",
                    step.id, step.executor, project.name
                )));
            }
            if !roles.iter().any(|r| r.name == step.role) {
                return Err(DefinitionError::Invalid(format!(
                    "step `{}` is in role `{}`, which does not exist",
                    step.id, step.role
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(text: &str) -> Result<Recipe, DefinitionError> {
        Recipe::parse(text)
    }

    const BASE: &str = r#"
name = "r"
title = "r"
[[step]]
id = "implement"
role = "backend"
executor = "claude-code"
produces = "commits"
[[step]]
id = "review"
role = "test"
executor = "codex"
produces = "verdict"
[[step]]
id = "fix"
role = "backend"
executor = "claude-code"
produces = "commits"
inputs = ["review"]
[loop]
between = ["review", "fix"]
max = 2
[finish]
propose = ["push_branch", "open_pull_request"]
"#;

    #[test]
    fn the_builtin_shape_reads() {
        let r = recipe(BASE).unwrap();
        assert_eq!(r.steps.len(), 3);
        assert_eq!(r.repeat.as_ref().unwrap().max, 2);
    }

    #[test]
    fn a_loop_is_bounded_and_well_formed() {
        assert!(recipe(&BASE.replace("max = 2", "max = 0")).is_err());
        assert!(recipe(&BASE.replace("max = 2", "max = 99")).is_err());
        assert!(recipe(&BASE.replace("produces = \"verdict\"", "produces = \"notes\"")).is_err());
        assert!(recipe(&BASE.replace("[\"review\", \"fix\"]", "[\"fix\", \"review\"]")).is_err());
    }

    #[test]
    fn inputs_come_from_earlier_steps() {
        assert!(recipe(&BASE.replace("inputs = [\"review\"]", "inputs = [\"later\"]")).is_err());
    }

    #[test]
    fn a_pull_request_needs_a_push() {
        assert!(recipe(&BASE.replace("\"push_branch\", ", "")).is_err());
    }

    #[test]
    fn unknown_fields_are_refused() {
        assert!(recipe(&format!("{BASE}\n[extra]\nx = 1\n")).is_err());
    }
}
