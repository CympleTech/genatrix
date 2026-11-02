//! What may be handed to an executor, how, and in what order.
//!
//! Design: `docs/design/13-executors.md`.
//!
//! Genatrix does not write code. It hands a task to an executor such as
//! Claude Code or Codex, in a workspace of their own, and keeps the
//! judgement: which project, which steps, what the executor may do while it
//! works, and when the work stops. This crate is that judgement, written as
//! data and pure functions, so that the core and the executor host decide
//! the same way and the tests need nothing running:
//!
//! - [`project`]: a repository, the executors allowed to see it, the
//!   commands they may run.
//! - [`role`]: a prompt and a set of permissions; six of them come built in.
//! - [`recipe`]: the steps of one piece of work, with a bounded loop.
//! - [`machine`]: given a recipe and what has happened, the next step.
//! - [`scope`]: the answer to an executor's request for permission.
//! - [`prompt`]: the text one step hands to its executor.
//! - [`report`]: what an executor hands back at the end of each turn.
//!
//! Nothing here reads a file, starts a process or touches the network.

pub mod builtin;
pub mod machine;
pub mod project;
pub mod prompt;
pub mod recipe;
pub mod report;
pub mod role;
pub mod scope;

/// SHA-256 of a definition's text, hex: its identity (design 13: "模板是
/// 版本化的文件，hash 就是身份").
#[must_use]
pub fn hash_of(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Why a definition was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DefinitionError {
    /// The text is not valid TOML of the expected shape.
    #[error("{0}")]
    Parse(String),
    /// The shape is right but something in it is not allowed.
    #[error("{0}")]
    Invalid(String),
}

impl From<toml::de::Error> for DefinitionError {
    fn from(e: toml::de::Error) -> Self {
        Self::Parse(e.to_string())
    }
}

/// A name for a project, a role, a recipe or an executor: lowercase letters,
/// digits and dashes, so it is safe in a path, a branch and a log line.
pub(crate) fn check_name(what: &str, name: &str) -> Result<(), DefinitionError> {
    let ok = !name.is_empty()
        && name.len() <= 48
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-');
    if ok {
        Ok(())
    } else {
        Err(DefinitionError::Invalid(format!(
            "{what} name `{name}`: use lowercase letters, digits and dashes"
        )))
    }
}
