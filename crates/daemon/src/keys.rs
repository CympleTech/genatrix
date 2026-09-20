//! Getting hold of the master key.
//!
//! Design: `docs/design/08-storage.md`.
//!
//! The design puts this in the macOS keychain. This is not that yet: it is a
//! file in the data directory, readable only by its owner. The difference is
//! worth naming rather than glossing, because it is the whole difference: a
//! key beside the database travels with a stolen copy of the database, so
//! this arrangement protects nothing from a stolen disk. It is a development
//! placeholder, replaced when the installer lands.
//!
//! Everything derived from the key is in `genatrix-keys`, where the
//! algorithms are fixed.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use genatrix_keys::MasterKey;

/// Load the master key, creating one if this is a fresh data directory.
pub fn load_or_create(path: &Path) -> anyhow::Result<MasterKey> {
    if path.exists() {
        let text = std::fs::read_to_string(path)?;
        return MasterKey::from_hex(&text)
            .map_err(|e| anyhow::anyhow!("{} is not a 64-character hex key: {e}", path.display()));
    }
    let key = MasterKey::generate()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::File::create(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    writeln!(file, "{}", key.to_hex())?;
    file.sync_all()?;
    tracing::info!(path = %path.display(), "created a master key");
    Ok(key)
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
        assert_eq!(first.to_hex(), second.to_hex());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "only the owner may read the key");
    }

    #[test]
    fn a_damaged_key_file_is_reported_rather_than_silently_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("master.key");
        std::fs::write(&path, "not a key").unwrap();
        let err = load_or_create(&path).unwrap_err().to_string();
        assert!(err.contains("hex key"), "{err}");
    }
}
