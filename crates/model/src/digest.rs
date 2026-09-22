//! The daily digest (design 03, the daily pipeline; design 06, the Today
//! page): grouped points, each with the items it was drawn from.
//!
//! A point with no sources is shown, greyed, as unfounded; a point whose
//! sources are not in the context it was made from is dropped before it
//! gets here (design 03). The granularity is the point, not the sentence.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::id::ItemId;

/// Which of the three groups a point belongs to (design 06).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigestGroup {
    /// Someone is waiting on the user.
    NeedsReply,
    /// The user promised something.
    Promised,
    /// Worth knowing, nothing to do.
    WorthKnowing,
}

/// One point.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigestPoint {
    /// The line.
    pub text: String,
    /// What it was drawn from. Empty means unfounded, and is shown so.
    pub sources: Vec<ItemId>,
}

/// One day's digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Digest {
    /// The day it was made for, local.
    pub day: NaiveDate,
    /// When it was made.
    pub generated_at: DateTime<Utc>,
    /// How many items it looked at.
    pub considered: u32,
    /// The points, grouped.
    pub groups: Vec<(DigestGroup, Vec<DigestPoint>)>,
}

impl Digest {
    /// Points in one group.
    #[must_use]
    pub fn points(&self, group: DigestGroup) -> &[DigestPoint] {
        self.groups
            .iter()
            .find(|(g, _)| *g == group)
            .map_or(&[], |(_, points)| points.as_slice())
    }
}
