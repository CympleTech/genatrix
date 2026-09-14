//! Typed identifiers.
//!
//! Every entity has a ULID: time-sortable, random, and free of any source
//! information. Each entity gets its own newtype so an `ItemId` can never be
//! passed where a `ThreadId` is expected.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Ulid);

        impl $name {
            /// Mint a fresh identifier for the current instant.
            #[must_use]
            pub fn new() -> Self {
                Self(Ulid::new())
            }

            /// The underlying ULID.
            #[must_use]
            pub fn as_ulid(&self) -> Ulid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<Ulid> for $name {
            fn from(u: Ulid) -> Self {
                Self(u)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl FromStr for $name {
            type Err = ulid::DecodeError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ulid::from_str(s).map(Self)
            }
        }
    };
}

define_id!(
    /// Identifier of a [`crate::Raw`] record.
    RawId
);
define_id!(
    /// Identifier of an [`crate::Item`].
    ItemId
);
define_id!(
    /// Identifier of a [`crate::Thread`].
    ThreadId
);
define_id!(
    /// Identifier of a [`crate::Person`].
    PersonId
);
define_id!(
    /// Identifier of a [`crate::Handle`].
    HandleId
);
define_id!(
    /// Identifier of an [`crate::Annotation`].
    AnnotationId
);

/// Reference to a [`crate::Blob`]. Blobs are content-addressed, so the
/// reference is the content hash itself rather than a ULID.
pub type BlobRef = crate::blob::ContentHash;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_time_sortable_and_distinct() {
        let a = ItemId::new();
        let b = ItemId::new();
        assert_ne!(a, b);
        assert!(a.as_ulid().timestamp_ms() <= b.as_ulid().timestamp_ms());
    }

    #[test]
    fn ids_round_trip_through_text_and_json() {
        let id = ThreadId::new();
        let text = id.to_string();
        assert_eq!(text.parse::<ThreadId>().unwrap(), id);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{text}\""));
        assert_eq!(serde_json::from_str::<ThreadId>(&json).unwrap(), id);
    }
}
