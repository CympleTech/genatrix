//! Content-addressed binary payloads: attachments, images, file bodies.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// SHA-256 of some bytes, the primary key of a [`Blob`] and of a
/// [`crate::Raw`] payload. Rendered as lowercase hex.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Hash the given bytes.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// Raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({self})")
    }
}

impl FromStr for ContentHash {
    type Err = hex::FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut out = [0u8; 32];
        hex::decode_to_slice(s, &mut out)?;
        Ok(Self(out))
    }
}

impl TryFrom<String> for ContentHash {
    type Error = hex::FromHexError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<ContentHash> for String {
    fn from(h: ContentHash) -> Self {
        h.to_string()
    }
}

/// Metadata for a content-addressed binary. The bytes themselves live in
/// the file system, encrypted per file (design 08); the database holds only
/// this record. The same file attached to ten mails is stored once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blob {
    /// Content hash; the primary key.
    pub hash: ContentHash,
    /// MIME type as reported by the source, or sniffed.
    pub mime: String,
    /// Size in bytes.
    pub size: u64,
    /// A file name seen for this content, if any. Advisory only.
    pub name_hint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic_and_hex_round_trips() {
        let h = ContentHash::of(b"genatrix");
        assert_eq!(h, ContentHash::of(b"genatrix"));
        assert_ne!(h, ContentHash::of(b"Genatrix"));
        let text = h.to_string();
        assert_eq!(text.len(), 64);
        assert_eq!(text.parse::<ContentHash>().unwrap(), h);
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(serde_json::from_str::<ContentHash>(&json).unwrap(), h);
    }

    #[test]
    fn rejects_malformed_hex() {
        assert!("zz".parse::<ContentHash>().is_err());
        assert!("ab".parse::<ContentHash>().is_err());
    }
}
