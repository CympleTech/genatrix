//! Who an executor runs as.
//!
//! Design: `docs/design/13-executors.md`, "本机隔离", ruling 3; proposed
//! invariant 23. An executor runs commands, and one that has read a hostile
//! issue runs the commands the issue asks for. So it runs as a system user
//! of its own, which cannot read the core's data directory, the user's
//! keychain, or the user's git and SSH credentials. `sudo` makes the switch;
//! `genatrix exec setup` installs the one rule that lets the user's account
//! start programs as that user without a password, which is a step down in
//! privilege, never up.

use serde::{Deserialize, Serialize};

/// The executor's system user, as `genatrix exec setup` creates it.
pub const EXEC_USER: &str = "_genatrix_exec";

/// The group the user and the executor's user share, for the mirrors.
pub const EXEC_GROUP: &str = "_genatrix";

/// How a program is started for the executor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "as", rename_all = "snake_case")]
pub enum Isolation {
    /// As this system user, through `sudo`.
    User {
        /// The user's name.
        name: String,
    },
    /// As whoever runs the host. For tests of the host itself; the core
    /// never builds this outside its tests.
    SameUserForTests,
}

impl Isolation {
    /// The isolation the core uses: the executor's own user.
    #[must_use]
    pub fn exec_user() -> Self {
        Self::User {
            name: EXEC_USER.to_owned(),
        }
    }

    /// The program and arguments that run `argv` under this isolation.
    ///
    /// `-n`: never ask for a password; if the rule is missing, fail at once
    /// instead of hanging on a prompt nobody sees. `-H`: the executor's own
    /// home, where its own settings and its vendor's login live.
    #[must_use]
    pub fn wrap(&self, argv: &[String]) -> Vec<String> {
        match self {
            Self::User { name } => {
                let mut out = vec![
                    "/usr/bin/sudo".to_owned(),
                    "-n".to_owned(),
                    "-H".to_owned(),
                    "-u".to_owned(),
                    name.clone(),
                    "--".to_owned(),
                ];
                out.extend(argv.iter().cloned());
                out
            }
            Self::SameUserForTests => argv.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proposed invariant 23: what the core starts for an executor is
    /// started as the executor's user, never as the user.
    #[test]
    fn the_executor_is_started_as_its_own_user() {
        let argv = vec!["npx".to_owned(), "-y".to_owned(), "adapter".to_owned()];
        let wrapped = Isolation::exec_user().wrap(&argv);
        assert_eq!(
            wrapped[..6],
            ["/usr/bin/sudo", "-n", "-H", "-u", EXEC_USER, "--"]
        );
        assert_eq!(wrapped[6..], argv[..]);
    }
}
