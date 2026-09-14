//! Thread: the container an item belongs to.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{PersonId, Source, ThreadId};

/// What kind of container a thread is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadKind {
    /// An email conversation, reconstructed from headers or a source thread id.
    MailThread,
    /// A one-to-one chat.
    DirectChat,
    /// A group chat.
    GroupChat,
    /// A broadcast channel.
    Channel,
    /// A calendar.
    Calendar,
    /// A notebook.
    Notebook,
    /// A folder of files.
    Folder,
}

/// A container of items. Not a parent: a thread does not own the items, it
/// groups them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    /// Identifier.
    pub id: ThreadId,
    /// Kind.
    pub kind: ThreadKind,
    /// Source reference of the container itself (chat id, thread id).
    pub source: Source,
    /// Subject, group name, calendar name.
    pub title: Option<String>,
    /// Current members. Updated when the source membership changes.
    pub members: Vec<PersonId>,
    /// Earliest `occurred_at` of its items; derived.
    pub first_at: Option<DateTime<Utc>>,
    /// Latest `occurred_at` of its items; derived.
    pub last_at: Option<DateTime<Utc>>,
}
