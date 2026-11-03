//! A workspace: the accounts and projects of one area of the user's work.
//!
//! Design: `docs/design/14-workspaces.md`.
//!
//! A workspace is a scope over the one base, not a second store. It
//! names the accounts that belong to it; a project names the workspace it
//! belongs to. What matters for trust follows from that: an item's content
//! may go to a project's executors only when the item's account is in the
//! project's workspace (invariant 31).

use serde::{Deserialize, Serialize};

use crate::project::Project;
use crate::{DefinitionError, check_name};

/// The workspace every project belongs to unless it says otherwise. It
/// comes built in, with no accounts.
pub const PERSONAL: &str = "personal";

/// The kinds of account a workspace may name, as Settings names them.
pub const ACCOUNT_KINDS: &[&str] = &["mail", "telegram", "lark"];

/// A workspace, as written in its TOML file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    /// Its name.
    pub name: String,
    /// What the page calls it.
    #[serde(default)]
    pub title: String,
    /// Its accounts, each `<kind>:<id>`, such as `lark:ou_…`.
    #[serde(default)]
    pub accounts: Vec<String>,
}

impl Workspace {
    /// Read and check a workspace file.
    pub fn parse(text: &str) -> Result<Self, DefinitionError> {
        let workspace: Self = toml::from_str(text)?;
        workspace.validate()?;
        Ok(workspace)
    }

    /// Check what the schema cannot.
    pub fn validate(&self) -> Result<(), DefinitionError> {
        check_name("workspace", &self.name)?;
        if self.title.len() > 80 || self.title.contains('\n') {
            return Err(DefinitionError::Invalid(
                "title is one line of at most 80 bytes".into(),
            ));
        }
        for (i, key) in self.accounts.iter().enumerate() {
            let Some((kind, id)) = key.split_once(':') else {
                return Err(DefinitionError::Invalid(format!(
                    "account `{key}`: expected <kind>:<id>, such as lark:ou_…"
                )));
            };
            if !ACCOUNT_KINDS.contains(&kind) || id.trim().is_empty() || id != id.trim() {
                return Err(DefinitionError::Invalid(format!(
                    "account `{key}`: the kind is one of {} and the id is not empty",
                    ACCOUNT_KINDS.join(", ")
                )));
            }
            if self.accounts[..i].contains(key) {
                return Err(DefinitionError::Invalid(format!(
                    "account `{key}` is named twice"
                )));
            }
        }
        Ok(())
    }

    /// The name the page shows.
    #[must_use]
    pub fn shown(&self) -> &str {
        if self.title.is_empty() {
            &self.name
        } else {
            &self.title
        }
    }

    /// Whether this account is one of this workspace's.
    #[must_use]
    pub fn has(&self, account: &str) -> bool {
        self.accounts.iter().any(|a| a == account)
    }
}

/// An account's key: `<kind>:<id>`, the kind as Settings names it. The
/// mail connector is `imap` inside, `mail` here.
#[must_use]
pub fn account_key(connector: &str, account: &str) -> String {
    let kind = match connector {
        "imap" => "mail",
        other => other,
    };
    format!("{kind}:{account}")
}

/// The workspace an account belongs to, if any (invariant 30: at most one).
#[must_use]
pub fn owner<'a>(workspaces: &'a [Workspace], account: &str) -> Option<&'a Workspace> {
    workspaces.iter().find(|w| w.has(account))
}

/// Why an item may not go to a project's executors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The account is in no workspace.
    #[error(
        "{account} is in no workspace, so what it brings goes to no executor; put it in a workspace first"
    )]
    Unowned {
        /// The account.
        account: String,
    },
    /// The account is in another workspace.
    #[error(
        "{account} belongs to workspace `{owner}`, and project `{project}` is in `{workspace}`; its content stays in its own workspace"
    )]
    Elsewhere {
        /// The account.
        account: String,
        /// Its workspace.
        owner: String,
        /// The project.
        project: String,
        /// The project's workspace.
        workspace: String,
    },
}

/// Whether the content of an item from `account` may go to `project`'s
/// executors (invariant 31). Only the item's own workspace may have it.
pub fn may_delegate(
    workspaces: &[Workspace],
    project: &Project,
    account: &str,
) -> Result<(), Refusal> {
    match owner(workspaces, account) {
        None => Err(Refusal::Unowned {
            account: account.to_owned(),
        }),
        Some(w) if w.name == project.workspace => Ok(()),
        Some(w) => Err(Refusal::Elsewhere {
            account: account.to_owned(),
            owner: w.name.clone(),
            project: project.name.clone(),
            workspace: project.workspace.clone(),
        }),
    }
}

/// Whether `candidate` may be filed beside `others`, the workspaces on file
/// (one of which may be its older version): no account in two workspaces
/// (invariant 30).
pub fn fits(candidate: &Workspace, others: &[Workspace]) -> Result<(), DefinitionError> {
    for other in others.iter().filter(|o| o.name != candidate.name) {
        if let Some(shared) = candidate.accounts.iter().find(|a| other.has(a)) {
            return Err(DefinitionError::Invalid(format!(
                "account `{shared}` is already in workspace `{}`; an account belongs to one workspace at most",
                other.name
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DPT: &str = r#"
name = "dpt"
title = "DPT"
accounts = ["lark:ou_1", "mail:neo@dpt.xyz"]
"#;

    fn project(workspace: &str) -> Project {
        Project::parse(&format!(
            "name = \"app\"\nrepo = \"~/app\"\nexecutors = [\"claude-code\"]\nworkspace = \"{workspace}\"\n"
        ))
        .unwrap()
    }

    #[test]
    fn a_workspace_reads_and_names_accounts_by_kind_and_id() {
        let w = Workspace::parse(DPT).unwrap();
        assert_eq!(w.shown(), "DPT");
        assert!(w.has("lark:ou_1"));
        assert!(!w.has("lark:ou_2"));
        for bad in [
            "accounts = [\"ou_1\"]",
            "accounts = [\"slack:x\"]",
            "accounts = [\"lark:\"]",
            "accounts = [\"lark:ou_1\", \"lark:ou_1\"]",
        ] {
            let text = format!("name = \"w\"\n{bad}\n");
            assert!(Workspace::parse(&text).is_err(), "{bad}");
        }
        assert!(Workspace::parse("name = \"w\"\nshare = true\n").is_err());
        assert_eq!(account_key("imap", "a@b.c"), "mail:a@b.c");
        assert_eq!(account_key("lark", "ou_1"), "lark:ou_1");
    }

    /// Invariant 30: an account belongs to one workspace at most.
    #[test]
    fn an_account_in_one_workspace_cannot_join_another() {
        let dpt = Workspace::parse(DPT).unwrap();
        let mine = Workspace::parse("name = \"mine\"\naccounts = [\"lark:ou_1\"]\n").unwrap();
        assert!(fits(&mine, std::slice::from_ref(&dpt)).is_err());
        // Its own older version is not another workspace.
        let again = Workspace::parse(&DPT.replace("\"DPT\"", "\"DPT Ltd\"")).unwrap();
        assert!(fits(&again, &[dpt]).is_ok());
    }

    /// Invariant 31: an item's content goes only to its own workspace's
    /// executors; an account in no workspace gives none.
    #[test]
    fn an_item_goes_only_to_executors_of_its_own_workspace() {
        let spaces = [
            Workspace::parse(DPT).unwrap(),
            Workspace::parse("name = \"personal\"\n").unwrap(),
        ];
        assert!(may_delegate(&spaces, &project("dpt"), "lark:ou_1").is_ok());
        assert!(matches!(
            may_delegate(&spaces, &project("personal"), "lark:ou_1"),
            Err(Refusal::Elsewhere { .. })
        ));
        assert!(matches!(
            may_delegate(&spaces, &project("dpt"), "mail:me@gmail.com"),
            Err(Refusal::Unowned { .. })
        ));
    }
}
