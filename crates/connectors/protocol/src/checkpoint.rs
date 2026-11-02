//! How far a connector has got, and how it picks up again.
//!
//! Design: `docs/design/05-connectors.md`, "幂等与检查点".
//!
//! Not losing anything and not duplicating anything are two different
//! problems with two different answers. Duplication is handled once, for
//! everyone, by the uniqueness of `Source`: fetch the same object twice and
//! the second one is dropped. That makes "fetch more than you need" a safe
//! move, which is what lets the cursors here be simple.
//!
//! So a cursor only has to be conservative. When it is unclear how far we
//! got, the right answer is always to go back further, never forward.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Where a connector has got to in one subdivision of an account: a mail
/// folder, or a whole Telegram account's update stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cursor", rename_all = "snake_case")]
pub enum Cursor {
    /// IMAP. The validity marker goes with the number: a server that changes
    /// `uidvalidity` has renumbered everything, and a UID carried across that
    /// boundary points at a different message.
    ImapUid {
        /// The folder's validity marker when this was recorded.
        uidvalidity: u32,
        /// Highest UID taken.
        highest: u32,
    },
    /// IMAP history, walked newest first: how far back the backfill has
    /// reached in one folder. Carries the validity marker for the same
    /// reason the live cursor does: after a renumbering, "everything below
    /// UID 1200" no longer names the same messages.
    ImapHistory {
        /// The folder's validity marker when this was recorded.
        uidvalidity: u32,
        /// Lowest UID taken so far, or none before the first batch.
        oldest: Option<u32>,
        /// Whether the beginning of the folder has been reached.
        complete: bool,
    },
    /// Telegram's update sequence.
    TelegramUpdates {
        /// Update state.
        pts: i32,
        /// Secret-chat update state.
        qts: i32,
        /// Sequence number.
        seq: i32,
        /// Timestamp of the last update applied.
        date: i32,
    },
    /// Walking history backwards: the oldest message id reached so far.
    History {
        /// Oldest id taken, or none before the first pass.
        oldest: Option<String>,
        /// Whether the far end has been reached.
        complete: bool,
    },
}

impl Cursor {
    /// Whether a newly read cursor can continue from this one, or whether
    /// the ground has moved and the subdivision must be read again.
    ///
    /// The conservative answer is the safe one: a rescan costs time, and
    /// duplicates are dropped anyway, while a wrong continuation loses mail
    /// silently.
    #[must_use]
    pub fn continues_from(&self, previous: &Self) -> bool {
        match (self, previous) {
            (
                Self::ImapUid { uidvalidity, .. },
                Self::ImapUid {
                    uidvalidity: before,
                    ..
                },
            )
            | (
                Self::ImapHistory { uidvalidity, .. },
                Self::ImapHistory {
                    uidvalidity: before,
                    ..
                },
            ) => uidvalidity == before,
            (Self::TelegramUpdates { pts, .. }, Self::TelegramUpdates { pts: before, .. }) => {
                pts >= before
            }
            (Self::History { .. }, Self::History { .. }) => true,
            // Different kinds of cursor mean the connector changed how it
            // reads this account. Start again.
            _ => false,
        }
    }
}

/// One account's progress in one subdivision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// The account.
    pub account: String,
    /// Which subdivision: a folder name, a chat id, or `""` for the account
    /// as a whole.
    pub scope: String,
    /// Where it has got to.
    pub cursor: Cursor,
    /// When this was last written.
    pub at: DateTime<Utc>,
}

impl Checkpoint {
    /// Record a position.
    pub fn new(account: impl Into<String>, scope: impl Into<String>, cursor: Cursor) -> Self {
        Self {
            account: account.into(),
            scope: scope.into(),
            cursor,
            at: Utc::now(),
        }
    }

    /// Move it forward, or say that the ground moved and a rescan is needed.
    ///
    /// Returns whether the new position continues the old one. Either way the
    /// checkpoint now holds the new cursor; the caller decides what a rescan
    /// means for it.
    pub fn advance(&mut self, cursor: Cursor) -> bool {
        let continues = cursor.continues_from(&self.cursor);
        self.cursor = cursor;
        self.at = Utc::now();
        continues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backfill_position_dies_with_the_folders_validity() {
        let before = Cursor::ImapHistory {
            uidvalidity: 10,
            oldest: Some(1200),
            complete: false,
        };
        assert!(
            Cursor::ImapHistory {
                uidvalidity: 10,
                oldest: Some(1150),
                complete: false,
            }
            .continues_from(&before)
        );
        assert!(
            !Cursor::ImapHistory {
                uidvalidity: 11,
                oldest: None,
                complete: false,
            }
            .continues_from(&before),
            "a renumbered folder has to be walked again from the top"
        );
    }

    #[test]
    fn an_imap_folder_continues_while_its_validity_holds() {
        let before = Cursor::ImapUid {
            uidvalidity: 10,
            highest: 500,
        };
        assert!(
            Cursor::ImapUid {
                uidvalidity: 10,
                highest: 600
            }
            .continues_from(&before)
        );
        assert!(
            !Cursor::ImapUid {
                uidvalidity: 11,
                highest: 600
            }
            .continues_from(&before),
            "a renumbered folder must be read again, not continued"
        );
    }

    #[test]
    fn a_telegram_stream_that_went_backwards_is_not_a_continuation() {
        let before = Cursor::TelegramUpdates {
            pts: 100,
            qts: 0,
            seq: 5,
            date: 1,
        };
        let forward = Cursor::TelegramUpdates {
            pts: 140,
            qts: 0,
            seq: 9,
            date: 2,
        };
        let backward = Cursor::TelegramUpdates {
            pts: 80,
            qts: 0,
            seq: 3,
            date: 2,
        };
        assert!(forward.continues_from(&before));
        assert!(!backward.continues_from(&before));
    }

    #[test]
    fn changing_how_an_account_is_read_starts_it_again() {
        let uid = Cursor::ImapUid {
            uidvalidity: 1,
            highest: 1,
        };
        let history = Cursor::History {
            oldest: None,
            complete: false,
        };
        assert!(!history.continues_from(&uid));
        assert!(!uid.continues_from(&history));
    }

    #[test]
    fn advancing_reports_whether_the_ground_moved() {
        let mut checkpoint = Checkpoint::new(
            "me@example.com",
            "INBOX",
            Cursor::ImapUid {
                uidvalidity: 10,
                highest: 500,
            },
        );
        assert!(checkpoint.advance(Cursor::ImapUid {
            uidvalidity: 10,
            highest: 700
        }));
        assert!(!checkpoint.advance(Cursor::ImapUid {
            uidvalidity: 12,
            highest: 3
        }));
        assert_eq!(
            checkpoint.cursor,
            Cursor::ImapUid {
                uidvalidity: 12,
                highest: 3
            },
            "the new position is kept either way"
        );
    }
}
