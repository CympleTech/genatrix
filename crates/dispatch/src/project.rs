//! A project: one repository and what executors may do with it.
//!
//! Design: `docs/design/13-executors.md`, "概念" and "派单就是出境".
//!
//! Which executors may see the repository is the user's decision, made
//! when the project is added: the code is not an item and does not pass the
//! gate, so this list is the whole of what decides who sees it.

use serde::{Deserialize, Serialize};

use crate::{DefinitionError, check_name};

/// A project, as written in its TOML file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// Its name.
    pub name: String,
    /// The workspace it belongs to (design 14). Every project is in one.
    #[serde(default = "default_workspace")]
    pub workspace: String,
    /// The repository on this machine, read by the core to refresh the
    /// mirror. Never given to an executor.
    pub repo: String,
    /// `owner/name` on GitHub, where pushes and pull requests go.
    #[serde(default)]
    pub github: Option<String>,
    /// The branch work starts from and pull requests target.
    #[serde(default = "default_base")]
    pub base: String,
    /// Executors that may see this repository, by name.
    pub executors: Vec<String>,
    /// Commands an executor may run in its workspace, exactly as written.
    #[serde(default)]
    pub commands: Vec<String>,
    /// Whether an executor may reach the network from its tools, to fetch
    /// dependencies. Its own conversation with its vendor is not this.
    #[serde(default)]
    pub network: bool,
    /// Whether personal content may go to executors, redacted (design 13,
    /// ruling 11). Off unless switched on here.
    #[serde(default)]
    pub delegate_personal: bool,
    /// The largest bundle of commits accepted back, in megabytes.
    #[serde(default = "default_bundle_cap")]
    pub bundle_cap_mb: u32,
}

fn default_workspace() -> String {
    crate::workspace::PERSONAL.to_owned()
}

fn default_base() -> String {
    "main".to_owned()
}

const fn default_bundle_cap() -> u32 {
    20
}

impl Project {
    /// Read and check a project file.
    pub fn parse(text: &str) -> Result<Self, DefinitionError> {
        let project: Self = toml::from_str(text)?;
        project.validate()?;
        Ok(project)
    }

    /// Check what the schema cannot.
    pub fn validate(&self) -> Result<(), DefinitionError> {
        check_name("project", &self.name)?;
        check_name("workspace", &self.workspace)?;
        if self.repo.trim().is_empty() {
            return Err(DefinitionError::Invalid("repo is empty".into()));
        }
        if self.executors.is_empty() {
            return Err(DefinitionError::Invalid(
                "no executor may see this project; name at least one".into(),
            ));
        }
        for e in &self.executors {
            check_name("executor", e)?;
        }
        if let Some(github) = &self.github {
            let parts: Vec<&str> = github.split('/').collect();
            let fine = parts.len() == 2
                && parts.iter().all(|p| {
                    !p.is_empty()
                        && p.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                });
            if !fine {
                return Err(DefinitionError::Invalid(format!(
                    "github `{github}`: expected owner/name"
                )));
            }
        }
        if !valid_branch(&self.base) {
            return Err(DefinitionError::Invalid(format!(
                "base branch `{}` is not a plain branch name",
                self.base
            )));
        }
        for c in &self.commands {
            if c.trim().is_empty() || c.contains('\n') {
                return Err(DefinitionError::Invalid(
                    "a command is empty or spans lines".into(),
                ));
            }
        }
        if self.bundle_cap_mb == 0 || self.bundle_cap_mb > 200 {
            return Err(DefinitionError::Invalid(
                "bundle_cap_mb must be between 1 and 200".into(),
            ));
        }
        Ok(())
    }

    /// Whether this executor may see the repository.
    #[must_use]
    pub fn allows(&self, executor: &str) -> bool {
        self.executors.iter().any(|e| e == executor)
    }
}

/// A branch name with nothing git or a shell would read specially.
#[must_use]
pub fn valid_branch(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with(['-', '/', '.'])
        && !name.ends_with(['/', '.'])
        && !name.contains("..")
        && !name.contains("//")
        && !name.to_ascii_lowercase().ends_with(".lock")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
name = "genatrix"
repo = "~/workspace/genatrix"
github = "CympleTech/genatrix"
executors = ["claude-code", "codex"]
commands = ["cargo test --workspace"]
"#;

    #[test]
    fn a_project_reads_with_its_defaults() {
        let p = Project::parse(GOOD).unwrap();
        assert_eq!(p.base, "main");
        assert_eq!(
            p.workspace, "personal",
            "a project is in the personal workspace unless it says"
        );
        assert!(!p.network);
        assert!(
            !p.delegate_personal,
            "personal content stays unless allowed"
        );
        assert_eq!(p.bundle_cap_mb, 20);
        assert!(p.allows("codex"));
        assert!(!p.allows("aider"));
    }

    #[test]
    fn unknown_fields_and_bad_values_are_refused() {
        assert!(Project::parse(&format!("{GOOD}\nshell = true\n")).is_err());
        assert!(Project::parse(&GOOD.replace("genatrix\"\nrepo", "Gen Atrix\"\nrepo")).is_err());
        assert!(Project::parse(&GOOD.replace("CympleTech/genatrix", "a/b/c")).is_err());
        assert!(Project::parse(&GOOD.replace("[\"claude-code\", \"codex\"]", "[]")).is_err());
    }

    #[test]
    fn branch_names_are_plain() {
        for good in ["main", "genatrix/01J9", "release-1.2"] {
            assert!(valid_branch(good), "{good}");
        }
        for bad in ["", "-x", "a..b", "a b", "x.lock", "a/", "$(rm)", "a//b"] {
            assert!(!valid_branch(bad), "{bad}");
        }
    }
}
