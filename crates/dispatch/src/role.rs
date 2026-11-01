//! A role: a prompt and what an executor in it may do.
//!
//! Design: `docs/design/13-executors.md`, "角色不是进程". Six roles do not
//! mean six resident agents. A step names a role; the core starts one
//! executor session with that role's prompt and permissions, and it ends
//! when the step does.

use serde::{Deserialize, Serialize};

use crate::{DefinitionError, check_name};

/// What a role lets an executor do inside its workspace. Anything outside
/// the workspace, and anything not listed, is refused (design 13, "第二层").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum May {
    /// Read files.
    Read,
    /// Change, create, move and delete files.
    Edit,
    /// Run the project's declared commands.
    Commands,
    /// Reach the network from its tools, where the project allows it too.
    Network,
}

/// A role, as written in its TOML file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Role {
    /// Its name.
    pub name: String,
    /// One line for the interface.
    pub title: String,
    /// What the executor is told it is and how to work.
    pub prompt: String,
    /// What it may do.
    pub may: Vec<May>,
}

impl Role {
    /// Read and check a role file.
    pub fn parse(text: &str) -> Result<Self, DefinitionError> {
        let role: Self = toml::from_str(text)?;
        check_name("role", &role.name)?;
        if role.prompt.trim().is_empty() {
            return Err(DefinitionError::Invalid(format!(
                "role `{}` has no prompt",
                role.name
            )));
        }
        Ok(role)
    }

    /// Whether the role allows this.
    #[must_use]
    pub fn may(&self, what: May) -> bool {
        self.may.contains(&what)
    }
}
