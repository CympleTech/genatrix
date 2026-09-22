//! Commitments: who promised whom what, by when (design 07).
//!
//! A commitment is an assertion about the world with evidence, like every
//! entry in the profile. The model proposes them as inferred; the user
//! confirms or rejects; a rejection is kept, so the same thing is not
//! proposed again.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::id::{CommitmentId, ItemId, PersonId};

/// Whether the promise still stands to be kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitmentStatus {
    /// Not yet done.
    Open,
    /// Kept.
    Done,
    /// No longer applies.
    Cancelled,
    /// Past due and still open.
    Overdue,
}

/// How much the assertion itself is to be trusted (design 07).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// The model read it out of a message. Shown as such.
    Inferred,
    /// The user said it is so.
    Confirmed,
    /// The user said it is not. Kept, so it is not proposed again.
    Rejected,
}

/// One promise.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commitment {
    /// Identifier.
    pub id: CommitmentId,
    /// Who promised.
    pub from: PersonId,
    /// Whom it was promised to, when known.
    pub to: Option<PersonId>,
    /// What, in a short line.
    pub what: String,
    /// By when, when a time was given.
    pub due: Option<DateTime<Utc>>,
    /// The items it was read from. Never empty: no evidence, no assertion.
    pub evidence: Vec<ItemId>,
    /// Kept, done, cancelled, overdue.
    pub status: CommitmentStatus,
    /// Inferred, confirmed, rejected.
    pub standing: Standing,
    /// When it was recorded.
    pub created_at: DateTime<Utc>,
}

impl Commitment {
    /// Whether the promise is the user's own to keep.
    #[must_use]
    pub fn is_mine(&self, me: Option<PersonId>) -> bool {
        me == Some(self.from)
    }

    /// Whether it should be shown as something still to do.
    #[must_use]
    pub const fn is_pending(&self) -> bool {
        matches!(
            self.status,
            CommitmentStatus::Open | CommitmentStatus::Overdue
        ) && !matches!(self.standing, Standing::Rejected)
    }
}

impl CommitmentStatus {
    /// Stable name, for storage.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::Overdue => "overdue",
        }
    }

    /// From its stable name.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "open" => Self::Open,
            "done" => Self::Done,
            "cancelled" => Self::Cancelled,
            "overdue" => Self::Overdue,
            _ => return None,
        })
    }
}

impl Standing {
    /// Stable name, for storage.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inferred => "inferred",
            Self::Confirmed => "confirmed",
            Self::Rejected => "rejected",
        }
    }

    /// From its stable name.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "inferred" => Self::Inferred,
            "confirmed" => Self::Confirmed,
            "rejected" => Self::Rejected,
            _ => return None,
        })
    }
}
