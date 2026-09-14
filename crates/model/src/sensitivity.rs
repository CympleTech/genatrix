//! Sensitivity levels (design 02).
//!
//! Three levels, no more. The ordering is the whole point: machines may only
//! raise a level, never lower it, and a request containing several items takes
//! the maximum. `Personal` is the default: anything not yet judged is already
//! unfit to leave the device as-is.

use serde::{Deserialize, Serialize};

/// How sensitive a piece of content is.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Already public or sent to everyone: newsletters, public channels,
    /// marketing. May go to a cloud model as-is, with a record.
    Public,
    /// Between you and specific people. The default. May go to a cloud model
    /// only after redaction, only if cloud is enabled, only if the task allows.
    #[default]
    Personal,
    /// Disclosure causes real harm: credentials, one-time codes, account
    /// numbers, health, legal, financial detail, anything you mark. Never
    /// leaves via Genatrix, no exceptions.
    Secret,
}

impl Level {
    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Personal => "personal",
            Self::Secret => "secret",
        }
    }

    /// Combine a machine judgement with an existing level: the result is
    /// never lower than either. This is the "escalate only" rule from
    /// design 02 expressed as a function. Lowering is a human act and does
    /// not go through here.
    #[must_use]
    pub fn escalate(self, other: Self) -> Self {
        self.max(other)
    }

    /// The level of a set of contents: the maximum, or `Personal` for an
    /// empty set, because an empty request still carries the user's words.
    pub fn of_all<I: IntoIterator<Item = Self>>(levels: I) -> Self {
        levels
            .into_iter()
            .reduce(Self::escalate)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_is_public_personal_secret() {
        assert!(Level::Public < Level::Personal);
        assert!(Level::Personal < Level::Secret);
    }

    #[test]
    fn default_is_personal() {
        assert_eq!(Level::default(), Level::Personal);
    }

    #[test]
    fn escalate_never_lowers() {
        assert_eq!(Level::Secret.escalate(Level::Public), Level::Secret);
        assert_eq!(Level::Public.escalate(Level::Secret), Level::Secret);
        assert_eq!(Level::Personal.escalate(Level::Public), Level::Personal);
    }

    #[test]
    fn one_secret_makes_the_set_secret() {
        let set = [Level::Public, Level::Personal, Level::Secret, Level::Public];
        assert_eq!(Level::of_all(set), Level::Secret);
        assert_eq!(Level::of_all([Level::Public, Level::Public]), Level::Public);
        assert_eq!(Level::of_all([]), Level::Personal);
    }

    #[test]
    fn serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Level::Secret).unwrap(), "\"secret\"");
    }
}
