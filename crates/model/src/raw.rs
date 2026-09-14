//! Raw records: what a connector fetched, byte for byte, never modified.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ContentHash, RawId, Source};

/// One source object as fetched. Append-only: the same source object fetched
/// again is skipped when the hash matches and stored as a new `Raw` when it
/// does not, which then derives a new version of the [`crate::Item`].
///
/// The payload bytes are stored content-addressed alongside blobs
/// (design 08); this record carries the hash and the metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Raw {
    /// Identifier.
    pub id: RawId,
    /// Where it came from.
    pub source: Source,
    /// When the connector fetched it.
    pub fetched_at: DateTime<Utc>,
    /// Media type of the payload, e.g. `message/rfc822` or
    /// `application/x-telegram-message+json`.
    pub content_type: String,
    /// SHA-256 of the payload bytes.
    pub hash: ContentHash,
    /// Payload size in bytes.
    pub size: u64,
}

impl Raw {
    /// Describe a freshly fetched payload. The caller stores the bytes under
    /// the returned hash.
    #[must_use]
    pub fn describe(source: Source, content_type: impl Into<String>, payload: &[u8]) -> Self {
        Self {
            id: RawId::new(),
            source,
            fetched_at: Utc::now(),
            content_type: content_type.into(),
            hash: ContentHash::of(payload),
            size: payload.len() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Connector;

    #[test]
    fn describe_hashes_payload() {
        let src = Source::new(Connector::Imap, "me@example.com", "gm:1");
        let raw = Raw::describe(src, "message/rfc822", b"From: a\r\n\r\nhi");
        assert_eq!(raw.size, 13);
        assert_eq!(raw.hash, ContentHash::of(b"From: a\r\n\r\nhi"));
    }
}
