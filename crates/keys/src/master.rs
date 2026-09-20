//! The master key, and everything derived from it.
//!
//! Design: `docs/design/08-storage.md`, "密钥".
//!
//! One secret to protect. Everything else comes out of it by a labelled
//! derivation, so the two databases hold different keys, every file holds a
//! different key again, and none of them is the master key. Losing one
//! derived key would not give up the others; there is no scheme here where
//! that could happen anyway, but the property is cheap and worth having.
//!
//! The algorithms are named in the design and fixed here: HKDF-SHA256 for
//! derivation, with an info string that carries both the purpose and a
//! version. When an algorithm has to change, the version in the label changes
//! with it, old data stays readable, and nothing has to be guessed.

use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::DbKey;

/// Derivation labels. A new purpose gets a new label; a changed algorithm
/// gets a new version inside the existing one.
const DB_INFO: &[u8] = b"genatrix/db/v1";
const FILE_INFO: &[u8] = b"genatrix/file/v1";

/// The one secret. Everything else is derived.
///
/// Lives in the macOS keychain in the finished product (design 08); how it
/// gets here is the caller's problem, and deliberately not this type's.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MasterKey([u8; 32]);

impl MasterKey {
    /// Wrap 32 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Generate a fresh one from the operating system.
    pub fn generate() -> std::io::Result<Self> {
        use rand::TryRngCore;
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(std::io::Error::other)?;
        Ok(Self(bytes))
    }

    /// Hex encoding, for the recovery key and for handing the key around
    /// before the keychain exists.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse a hex encoding.
    pub fn from_hex(text: &str) -> Result<Self, hex::FromHexError> {
        let mut bytes = [0u8; 32];
        hex::decode_to_slice(text.trim(), &mut bytes)?;
        Ok(Self(bytes))
    }

    /// The key for one of the databases. `label` names which.
    #[must_use]
    pub fn db_key(&self, label: &str) -> DbKey {
        DbKey::from_bytes(self.derive(DB_INFO, label.as_bytes()))
    }

    /// The key for one content-addressed file.
    ///
    /// Bound to the content hash, so every file has its own key and a nonce
    /// can never be reused across files even if one were chosen badly.
    #[must_use]
    pub fn file_key(&self, content_hash: &[u8; 32]) -> FileKey {
        FileKey(self.derive(FILE_INFO, content_hash))
    }

    fn derive(&self, purpose: &[u8], context: &[u8]) -> [u8; 32] {
        let hkdf = Hkdf::<Sha256>::new(None, &self.0);
        let mut info = Vec::with_capacity(purpose.len() + 1 + context.len());
        info.extend_from_slice(purpose);
        // A separator, so a purpose and a context cannot be shifted across
        // the boundary to produce the same key from different inputs.
        info.push(0);
        info.extend_from_slice(context);
        let mut out = [0u8; 32];
        hkdf.expand(&info, &mut out)
            .expect("32 bytes is within HKDF-SHA256's output limit");
        out
    }
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey(..)")
    }
}

/// The key for one file's contents.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct FileKey([u8; 32]);

impl FileKey {
    /// The key bytes, for the cipher.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for FileKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileKey(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn master() -> MasterKey {
        MasterKey::from_bytes([3; 32])
    }

    #[test]
    fn derivation_is_deterministic() {
        assert_eq!(
            master().db_key("store").pragma_literal(),
            master().db_key("store").pragma_literal()
        );
        assert_eq!(
            master().file_key(&[9; 32]).as_bytes(),
            master().file_key(&[9; 32]).as_bytes()
        );
    }

    #[test]
    fn every_purpose_and_context_gets_its_own_key() {
        let m = master();
        let store = m.db_key("store").pragma_literal();
        let ledger = m.db_key("ledger").pragma_literal();
        assert_ne!(store, ledger);
        assert_ne!(
            m.file_key(&[1; 32]).as_bytes(),
            m.file_key(&[2; 32]).as_bytes()
        );
        // And a file key is not a database key wearing a different hat.
        assert!(!store.contains(&hex::encode(m.file_key(&[1; 32]).as_bytes())));
    }

    #[test]
    fn no_derived_key_is_the_master_key() {
        let m = master();
        let master_hex = m.to_hex();
        assert!(!m.db_key("store").pragma_literal().contains(&master_hex));
        assert_ne!(m.file_key(&[0; 32]).as_bytes(), &[3u8; 32]);
    }

    #[test]
    fn the_label_separator_stops_inputs_running_together() {
        // Without the separator, purpose "ab" with context "c" and purpose
        // "a" with context "bc" would derive the same key.
        let m = master();
        let a = m.derive(b"ab", b"c");
        let b = m.derive(b"a", b"bc");
        assert_ne!(a, b);
    }

    #[test]
    fn generate_is_random_and_hex_round_trips() {
        let a = MasterKey::generate().unwrap();
        let b = MasterKey::generate().unwrap();
        assert_ne!(a.to_hex(), b.to_hex());
        assert_eq!(
            MasterKey::from_hex(&a.to_hex()).unwrap().to_hex(),
            a.to_hex()
        );
        assert_eq!(a.to_hex().len(), 64);
    }

    #[test]
    fn keys_never_print_themselves() {
        assert_eq!(format!("{:?}", master()), "MasterKey(..)");
        assert_eq!(format!("{:?}", master().file_key(&[0; 32])), "FileKey(..)");
    }
}
