//! Database key.

use std::fmt;

/// A 256-bit raw key for `SQLCipher`. Never logged, never displayed.
///
/// Derivation (`HKDF-SHA256(master, "genatrix/db/v1")`, design 08) happens
/// in the caller; the store only consumes the result.
#[derive(Clone)]
pub struct DbKey([u8; 32]);

impl DbKey {
    /// Wrap 32 key bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The `PRAGMA key` literal: a raw hex key so `SQLCipher` skips its own
    /// password derivation. Only the store and ledger crates should call this.
    #[doc(hidden)]
    #[must_use]
    pub fn pragma_literal(&self) -> String {
        format!("\"x'{}'\"", hex::encode(self.0))
    }
}

impl fmt::Debug for DbKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DbKey(..)")
    }
}

impl Drop for DbKey {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_key_material() {
        let k = DbKey::from_bytes([0xab; 32]);
        assert_eq!(format!("{k:?}"), "DbKey(..)");
        assert!(k.pragma_literal().starts_with("\"x'abab"));
    }
}
