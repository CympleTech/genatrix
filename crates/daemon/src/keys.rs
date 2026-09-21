//! Getting hold of the master key.
//!
//! Design: `docs/design/08-storage.md`, "密钥": the master key lives in the
//! macOS keychain, unlocked with the login, and nowhere in the data
//! directory. That is the whole point of encrypting at rest: a copy of the
//! directory, on a stolen disk or in a synced backup, is ciphertext without
//! the keychain.
//!
//! That holds for the real data directory. A directory named with
//! `--data-dir` is a development one, and there the key stays in a file
//! beside the data, readable by its owner only, so that tests and scratch
//! setups never touch the keychain and never leave anything in it. The
//! difference is said out loud when such a directory is opened: a key
//! beside the database protects nothing from a stolen disk.
//!
//! Everything derived from the key is in `genatrix-keys`, where the
//! algorithms are fixed.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use genatrix_keys::MasterKey;

use crate::config::Config;
use crate::keychain::Keychain;

/// The keychain service and account the master key is filed under.
const SERVICE: &str = "Genatrix";
const ACCOUNT: &str = "master-key";

/// The master key for this data directory: from the keychain for the real
/// one, from a file for a development one.
pub fn obtain(config: &Config) -> anyhow::Result<MasterKey> {
    if uses_keychain(config) {
        from_keychain(&Keychain::named(SERVICE), &config.key_path())
    } else {
        tracing::info!(
            path = %config.key_path().display(),
            "development data directory: the master key is a file beside the data"
        );
        load_or_create(&config.key_path())
    }
}

/// Only the default data directory keeps its key in the keychain.
fn uses_keychain(config: &Config) -> bool {
    config.data_dir == Config::default_data_dir()
}

/// The key from the keychain; created there if absent. A key file left by
/// an earlier build is moved in and then removed, so an upgrade keeps the
/// data readable and ends with no key on disk.
fn from_keychain(chain: &Keychain, legacy_file: &Path) -> anyhow::Result<MasterKey> {
    if let Some(hex) = chain.read(ACCOUNT)? {
        return MasterKey::from_hex(&hex)
            .map_err(|e| anyhow::anyhow!("the master key in the keychain is damaged: {e}"));
    }
    let key = if legacy_file.exists() {
        let key = load_or_create(legacy_file)?;
        chain.store(ACCOUNT, &key.to_hex())?;
        std::fs::remove_file(legacy_file)?;
        tracing::info!("moved the master key from a file into the keychain");
        key
    } else {
        let key = MasterKey::generate()?;
        chain.store(ACCOUNT, &key.to_hex())?;
        tracing::info!("created a master key in the keychain");
        key
    };
    Ok(key)
}

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
    fn only_the_real_data_directory_uses_the_keychain() {
        assert!(uses_keychain(&Config::under(Config::default_data_dir())));
        assert!(!uses_keychain(&Config::under("/tmp/genatrix-dev")));
    }

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
