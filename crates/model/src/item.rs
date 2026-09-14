//! Item: one cell on the timeline. Spine plus payload.

use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};

use crate::{BlobRef, ItemId, Level, PersonId, RawId, Source, ThreadId};

/// A point in time as the source recorded it, with its UTC offset.
pub type Timestamp = DateTime<FixedOffset>;

/// What kind of trace an item is. New kinds are new payload variants and a
/// migration, never a generic JSON field (design 01, "explicitly not").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// An email message.
    Mail,
    /// A chat message.
    Message,
    /// A calendar event.
    Event,
    /// A note.
    Note,
    /// A file.
    File,
}

/// The item from your point of view. Computed from the `is_self` person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Sent to you.
    Inbound,
    /// Sent by you.
    Outbound,
    /// You to yourself: memos, saved messages.
    Internal,
    /// Has no direction: events, files.
    Neutral,
}

/// Kind-specific fields. Each variant carries only what is needed to read
/// the cell; everything else is in the [`crate::Raw`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Payload {
    /// Email headers that matter for display and threading.
    Mail {
        /// Subject line, as received.
        subject: String,
        /// `From` header, raw.
        from: String,
        /// `To` header addresses, raw.
        to: Vec<String>,
        /// `Cc` header addresses, raw.
        cc: Vec<String>,
        /// `Message-ID` header, if present. Advisory: may repeat or be absent.
        message_id: Option<String>,
        /// `In-Reply-To` header, if present.
        in_reply_to: Option<String>,
        /// `References` header ids, in order.
        references: Vec<String>,
        /// Source-side labels or folder names.
        labels: Vec<String>,
    },
    /// Chat message specifics.
    Message {
        /// The item this one replies to, if any.
        reply_to: Option<ItemId>,
        /// Original author when the message was forwarded.
        forwarded_from: Option<PersonId>,
        /// Whether the source marked it as edited.
        edited: bool,
    },
    /// Calendar event.
    Event {
        /// Title.
        title: String,
        /// Start.
        start: Timestamp,
        /// End.
        end: Timestamp,
        /// Whether it spans whole days.
        all_day: bool,
        /// Location text, if any.
        location: Option<String>,
        /// Attendees known as persons.
        attendees: Vec<PersonId>,
    },
    /// A note.
    Note {
        /// Title.
        title: String,
        /// Whether the body is markdown.
        markdown: bool,
    },
    /// A file.
    File {
        /// File name.
        name: String,
        /// MIME type.
        mime: String,
        /// Size in bytes.
        size: u64,
    },
}

impl Payload {
    /// The kind this payload belongs to.
    #[must_use]
    pub const fn kind(&self) -> Kind {
        match self {
            Self::Mail { .. } => Kind::Mail,
            Self::Message { .. } => Kind::Message,
            Self::Event { .. } => Kind::Event,
            Self::Note { .. } => Kind::Note,
            Self::File { .. } => Kind::File,
        }
    }
}

/// One cell on the timeline.
///
/// Items are immutable. An upstream edit produces a new item whose
/// `supersedes` points at the old one; an upstream deletion sets
/// `tombstoned` on a new version and keeps the content. No field here is
/// ever written by a model: the sensitivity level is a cache of the
/// currently effective judgement, and that judgement is an
/// [`crate::Annotation`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// Identifier.
    pub id: ItemId,
    /// Where it came from. Unique with the account.
    pub source: Source,
    /// The raw record this item was derived from.
    pub raw_id: RawId,
    /// The previous version this one replaces, if the source edited it.
    pub supersedes: Option<ItemId>,
    /// The conversation this item belongs to.
    pub thread_id: ThreadId,
    /// When it happened in the source, with the source's offset.
    pub occurred_at: Timestamp,
    /// When it entered Genatrix.
    pub ingested_at: DateTime<Utc>,
    /// From your point of view.
    pub direction: Direction,
    /// Who produced it, when known.
    pub author: Option<PersonId>,
    /// Explicit recipients of this item, not the whole thread's members.
    pub recipients: Vec<PersonId>,
    /// Normalized plain text: what search and models read. HTML stripped,
    /// quoted replies removed, formatting entities flattened.
    pub text: String,
    /// Attachments and inline media.
    pub blobs: Vec<BlobRef>,
    /// Effective sensitivity. Defaults to `Personal`.
    pub sensitivity: Level,
    /// Deleted upstream. Kept locally; hidden by default.
    pub tombstoned: bool,
    /// Kind-specific fields.
    pub payload: Payload,
}

impl Item {
    /// The kind, derived from the payload.
    #[must_use]
    pub const fn kind(&self) -> Kind {
        self.payload.kind()
    }
}

/// Decide the direction of an item given who wrote it and who received it,
/// relative to the set of persons that are "you".
#[must_use]
pub fn direction_of(
    author: Option<PersonId>,
    recipients: &[PersonId],
    is_self: impl Fn(PersonId) -> bool,
    has_direction: bool,
) -> Direction {
    if !has_direction {
        return Direction::Neutral;
    }
    let from_self = author.is_some_and(&is_self);
    let to_self = recipients.iter().copied().any(&is_self);
    match (from_self, to_self) {
        (true, true) => Direction::Internal,
        (true, false) => Direction::Outbound,
        (false, _) => Direction::Inbound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_follows_self() {
        let me = PersonId::new();
        let you = PersonId::new();
        let is_me = |p: PersonId| p == me;
        assert_eq!(
            direction_of(Some(you), &[me], is_me, true),
            Direction::Inbound
        );
        assert_eq!(
            direction_of(Some(me), &[you], is_me, true),
            Direction::Outbound
        );
        assert_eq!(
            direction_of(Some(me), &[me], is_me, true),
            Direction::Internal
        );
        assert_eq!(
            direction_of(Some(me), &[], is_me, true),
            Direction::Outbound
        );
        assert_eq!(direction_of(None, &[me], is_me, true), Direction::Inbound);
        assert_eq!(
            direction_of(Some(me), &[], is_me, false),
            Direction::Neutral
        );
    }

    #[test]
    fn payload_is_tagged_by_kind_in_json() {
        let p = Payload::Message {
            reply_to: None,
            forwarded_from: None,
            edited: true,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.starts_with("{\"kind\":\"message\""));
        assert_eq!(p.kind(), Kind::Message);
        assert_eq!(serde_json::from_str::<Payload>(&json).unwrap(), p);
    }
}
