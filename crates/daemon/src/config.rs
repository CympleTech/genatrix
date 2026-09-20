//! Where everything lives, and how the daemon is told about it.
//!
//! Design: `docs/design/08-storage.md` for the layout.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The data directory and the sockets.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// Everything Genatrix owns lives under here.
    pub data_dir: PathBuf,
    /// Socket of the model gateway. Filled in from the gateway's own
    /// configuration when the system opens; the value here is only a guess
    /// for error messages before that happens.
    pub gateway_socket: PathBuf,
    /// Whether cloud models may be used at all. Off, as design 02 requires;
    /// there is no transport for it in this build either.
    #[serde(default)]
    pub cloud_enabled: bool,
}

impl Config {
    /// The conventional layout under a data directory.
    #[must_use]
    pub fn under(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        Self {
            gateway_socket: data_dir.join("run").join("gateway.sock"),
            data_dir,
            cloud_enabled: false,
        }
    }

    /// The default location: `~/Library/Application Support/Genatrix`.
    #[must_use]
    pub fn default_data_dir() -> PathBuf {
        std::env::var_os("HOME").map_or_else(
            || PathBuf::from("./genatrix-data"),
            |home| {
                Path::new(&home)
                    .join("Library")
                    .join("Application Support")
                    .join("Genatrix")
            },
        )
    }

    /// The main database.
    #[must_use]
    pub fn store_path(&self) -> PathBuf {
        self.data_dir.join("data.db")
    }

    /// The ledger.
    #[must_use]
    pub fn ledger_path(&self) -> PathBuf {
        self.data_dir.join("ledger.db")
    }

    /// The master key file. A stand-in: design 08 puts this in the macOS
    /// keychain, which lands with the installer in M1. Until then it is a
    /// file only the user can read, which is weaker and is said so out loud
    /// rather than quietly.
    #[must_use]
    pub fn key_path(&self) -> PathBuf {
        self.data_dir.join("master.key")
    }

    /// The sensitivity rules.
    #[must_use]
    pub fn rules_path(&self) -> PathBuf {
        self.data_dir.join("rules").join("sensitivity.toml")
    }

    /// The accounts this copy reads.
    #[must_use]
    pub fn accounts_path(&self) -> PathBuf {
        self.data_dir.join("accounts.toml")
    }

    /// The model registry, shared with the gateway.
    #[must_use]
    pub fn gateway_config_path(&self) -> PathBuf {
        self.data_dir.join("gateway.toml")
    }

    /// Create the directories this layout needs.
    pub fn create_dirs(&self) -> std::io::Result<()> {
        for dir in [
            self.data_dir.clone(),
            self.data_dir.join("run"),
            self.data_dir.join("rules"),
            self.data_dir.join("raw"),
            self.data_dir.join("blobs"),
            self.data_dir.join("models"),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}
