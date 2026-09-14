//! Provenance: which connector, which account, which object in the source.

use serde::{Deserialize, Serialize};

/// The kind of connector a record came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Connector {
    /// Mail over IMAP (and SMTP for sending).
    Imap,
    /// Telegram, user-account protocol.
    Telegram,
}

impl Connector {
    /// Stable lowercase name, used in storage and export.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Imap => "imap",
            Self::Telegram => "telegram",
        }
    }
}

/// The anchor for idempotent ingestion. The triple is unique.
///
/// `external_id` must be unique across the whole account, not just within
/// one folder or chat. Connectors build it as a composite where the source
/// system's own id is not globally unique (design 05): Telegram uses
/// `chat_id:message_id`; IMAP uses the Gmail message id when available,
/// otherwise `folder/uidvalidity/uid`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Source {
    /// Which connector produced the record.
    pub connector: Connector,
    /// Account identifier: an email address, a Telegram user id.
    pub account: String,
    /// Account-wide unique id of the object in the source system.
    pub external_id: String,
}

impl Source {
    /// Build a source reference.
    #[must_use]
    pub fn new(
        connector: Connector,
        account: impl Into<String>,
        external_id: impl Into<String>,
    ) -> Self {
        Self {
            connector,
            account: account.into(),
            external_id: external_id.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_triple_is_equal_different_account_is_not() {
        let a = Source::new(Connector::Telegram, "42", "7:100");
        let b = Source::new(Connector::Telegram, "42", "7:100");
        let c = Source::new(Connector::Telegram, "43", "7:100");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn serializes_connector_as_lowercase() {
        let s = Source::new(Connector::Imap, "me@example.com", "gm:1");
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"connector\":\"imap\""));
    }
}
