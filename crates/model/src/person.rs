//! Person and Handle: one real person, many addresses across sources.
//!
//! Merging handles into one person is an assertion about who is who. It is
//! done by rules or by the user, never by a model; a model may only suggest,
//! and a suggestion is an [`crate::Annotation`]. Inferred merges are marked
//! and can always be split.

use serde::{Deserialize, Serialize};

use crate::{HandleId, PersonId};

/// A real person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    /// Identifier.
    pub id: PersonId,
    /// Name to show.
    pub display_name: String,
    /// Exactly one person is you. Direction is computed from this.
    pub is_self: bool,
    /// Persons merged into this one; kept so a merge can be undone.
    pub merged_from: Vec<PersonId>,
}

/// What kind of address a handle is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandleKind {
    /// An email address.
    Email,
    /// A Telegram user id.
    TelegramId,
    /// A Telegram username.
    TelegramUsername,
    /// A phone number.
    Phone,
}

/// How a handle came to be attached to its person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// The user confirmed it, or it is the only handle the person has.
    Confirmed,
    /// A rule inferred it. Shown as such; can be split at any time.
    Inferred,
}

/// One address belonging to a person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handle {
    /// Identifier.
    pub id: HandleId,
    /// The person it belongs to.
    pub person_id: PersonId,
    /// Kind of address.
    pub kind: HandleKind,
    /// The address, normalized (lowercase email, E.164 phone, numeric id).
    pub value: String,
    /// How sure we are that it belongs to this person.
    pub confidence: Confidence,
}

impl Handle {
    /// Normalize a raw address value for its kind so that equal addresses
    /// compare equal. Emails are lowercased and trimmed; usernames lose a
    /// leading `@`; other kinds are trimmed.
    #[must_use]
    pub fn normalize(kind: HandleKind, value: &str) -> String {
        let v = value.trim();
        match kind {
            HandleKind::Email => v.to_lowercase(),
            HandleKind::TelegramUsername => v.trim_start_matches('@').to_lowercase(),
            HandleKind::TelegramId | HandleKind::Phone => v.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_addresses() {
        assert_eq!(
            Handle::normalize(HandleKind::Email, "  Neo@Example.COM "),
            "neo@example.com"
        );
        assert_eq!(
            Handle::normalize(HandleKind::TelegramUsername, "@Neo"),
            "neo"
        );
        assert_eq!(Handle::normalize(HandleKind::TelegramId, " 42 "), "42");
    }
}
