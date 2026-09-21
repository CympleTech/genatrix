//! What one account is doing right now, in words for the interface.
//!
//! Design: `docs/design/05-connectors.md`, "账号状态" and "恢复".
//!
//! The design asks for one state per account that a person can read at a
//! glance: syncing, synced, needs a new login, or stopped, with the last
//! sync time and the backfill progress alongside. This is that state. The
//! connector publishes it, the core relays it, the interface shows it; none
//! of them interprets it, which is why the words are chosen here.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::backfill::Progress;
use crate::error::{Fault, Severity};

/// One account's state, for the interface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SyncState {
    /// Nothing has happened yet.
    Starting,
    /// Reaching the server.
    Connecting,
    /// History is still coming in. New mail is taken between batches.
    Backfilling {
        /// How far.
        progress: Progress,
    },
    /// Everything is in; waiting for something new.
    Live {
        /// When the account was last brought up to date.
        synced_at: DateTime<Utc>,
    },
    /// Something went wrong that will pass. The account is being retried
    /// and nobody has to do anything.
    Retrying {
        /// What happened, in the fault's words.
        detail: String,
        /// How many times in a row it has failed.
        attempt: u32,
        /// Seconds until the next try.
        next_in_secs: u64,
    },
    /// Only the user can fix it.
    NeedsLogin {
        /// What happened, in the fault's words.
        detail: String,
    },
    /// It will not come back. What was fetched stays.
    Stopped {
        /// What happened, in the fault's words.
        detail: String,
    },
}

impl SyncState {
    /// The state a fault puts an account in.
    #[must_use]
    pub fn after(fault: &Fault, attempt: u32) -> Self {
        match fault.severity {
            Severity::Transient => Self::Retrying {
                detail: fault.detail.clone(),
                attempt,
                next_in_secs: Fault::backoff(attempt).as_secs(),
            },
            Severity::NeedsUser => Self::NeedsLogin {
                detail: fault.detail.clone(),
            },
            Severity::Permanent => Self::Stopped {
                detail: fault.detail.clone(),
            },
        }
    }

    /// Whether the connector is still working on this account.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        !matches!(self, Self::NeedsLogin { .. } | Self::Stopped { .. })
    }

    /// One line for a terminal or a status bar.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Starting => "starting".to_owned(),
            Self::Connecting => "connecting".to_owned(),
            Self::Backfilling { progress } => format!("importing history: {}", progress.describe()),
            Self::Live { synced_at } => {
                format!("up to date as of {}", synced_at.format("%H:%M:%S"))
            }
            Self::Retrying {
                detail,
                next_in_secs,
                ..
            } => format!("{detail}; trying again in {next_in_secs}s"),
            Self::NeedsLogin { detail } => format!("needs a new login: {detail}"),
            Self::Stopped { detail } => format!("stopped: {detail}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_of_fault_lands_in_the_state_the_design_names() {
        let hiccup = Fault::transient("a", "the connection dropped");
        assert!(matches!(
            SyncState::after(&hiccup, 3),
            SyncState::Retrying {
                attempt: 3,
                next_in_secs: 8,
                ..
            }
        ));
        let password = Fault::needs_user("a", "the password was refused");
        assert!(matches!(
            SyncState::after(&password, 0),
            SyncState::NeedsLogin { .. }
        ));
        let gone = Fault::permanent("a", "no such server");
        assert!(matches!(
            SyncState::after(&gone, 0),
            SyncState::Stopped { .. }
        ));
    }

    #[test]
    fn only_the_user_facing_states_stop_the_connector() {
        assert!(SyncState::Starting.is_running());
        assert!(
            SyncState::Retrying {
                detail: String::new(),
                attempt: 1,
                next_in_secs: 2
            }
            .is_running()
        );
        assert!(
            !SyncState::NeedsLogin {
                detail: String::new()
            }
            .is_running()
        );
        assert!(
            !SyncState::Stopped {
                detail: String::new()
            }
            .is_running()
        );
    }

    #[test]
    fn the_state_serializes_with_a_tag_the_page_can_switch_on() {
        let json = serde_json::to_string(&SyncState::NeedsLogin {
            detail: "refused".into(),
        })
        .unwrap();
        assert!(json.contains(r#""state":"needs_login""#), "{json}");
    }
}
