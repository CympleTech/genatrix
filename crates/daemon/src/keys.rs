//! The master key, and the two keys derived from it.
//!
//! Design: `docs/design/08-storage.md`.
//!
//! The design puts the master key in the macOS keychain. This is not that
//! yet: it is a file in the data directory, readable only by its owner. The
//! difference matters and is worth naming rather than glossing: a key beside
//! the database is a key that travels with a stolen copy of the database, so
//! this arrangement protects nothing from a stolen disk. It is a development
//! placeholder, replaced when the installer lands in M1.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use genatrix_keys::DbKey;

/// Load the master key, creating one if this is a fresh data directory.
pub fn load_or_create(path: &Path) -> anyhow::Result<[u8; 32]> {
    if path.exists() {
        let text = std::fs::read_to_string(path)?;
        let mut key = [0u8; 32];
        hex::decode_to_slice(text.trim(), &mut key).map_err(|e| {
            anyhow::anyhow!("{} is not a 64-character hex key: {e}", path.display())
        })?;
        return Ok(key);
    }
    let key = random_key()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::File::create(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    writeln!(file, "{}", hex::encode(key))?;
    file.sync_all()?;
    tracing::info!(path = %path.display(), "created a master key");
    Ok(key)
}

fn random_key() -> anyhow::Result<[u8; 32]> {
    let mut key = [0u8; 32];
    getrandom(&mut key)?;
    Ok(key)
}

fn getrandom(buf: &mut [u8]) -> anyhow::Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(buf)?;
    Ok(())
}

/// Derive the key for one of the databases.
///
/// Design 08 specifies `HKDF-SHA256(master, "genatrix/db/v1")`. Until the key
/// schedule crate lands this uses a labelled SHA-256, which has the property
/// that matters here, namely that the two databases get different keys and
/// neither is the master key.
#[must_use]
pub fn db_key(master: &[u8; 32], label: &str) -> DbKey {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"genatrix/db/v1\0");
    hasher.update(label.as_bytes());
    hasher.update([0u8]);
    hasher.update(master);
    DbKey::from_bytes(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_created_once_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("master.key");
        let first = load_or_create(&path).unwrap();
        let second = load_or_create(&path).unwrap();
        assert_eq!(first, second);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "only the owner may read the key");
    }

    #[test]
    fn the_two_databases_get_different_keys() {
        let master = [7u8; 32];
        let store = db_key(&master, "store");
        let ledger = db_key(&master, "ledger");
        assert_ne!(store.pragma_literal(), ledger.pragma_literal());
        assert!(!store.pragma_literal().contains(&hex::encode(master)));
    }
}
