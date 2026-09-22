//! The application credentials Telegram requires of every client.
//!
//! Design: `docs/design/05-connectors.md`, "应用凭据": one pair for
//! Genatrix, compiled in and shared by every user. Until a release build
//! carries them, they come from the environment or the developer's
//! secrets file, and never from the repository.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where the developer keeps them, outside the checkout.
const DEV_SECRETS: &str = ".config/genatrix-dev/secrets.toml";

/// Telegram's `api_id` and `api_hash`.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    /// The numeric application id.
    pub api_id: i32,
    /// The hash that goes with it.
    pub api_hash: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("api_id", &self.api_id)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct SecretsFile {
    telegram: Option<Credentials>,
}

impl Credentials {
    /// The environment (`GENATRIX_TELEGRAM_API_ID`, `GENATRIX_TELEGRAM_API_HASH`),
    /// then the developer's secrets file, then nothing.
    pub fn find() -> anyhow::Result<Self> {
        if let (Ok(id), Ok(hash)) = (
            std::env::var("GENATRIX_TELEGRAM_API_ID"),
            std::env::var("GENATRIX_TELEGRAM_API_HASH"),
        ) {
            return Ok(Self {
                api_id: id.trim().parse()?,
                api_hash: hash.trim().to_owned(),
            });
        }
        let path = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(DEV_SECRETS))
            .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
        let text = std::fs::read_to_string(&path).map_err(|e| {
            anyhow::anyhow!(
                "no Telegram application credentials: set GENATRIX_TELEGRAM_API_ID and \
                 GENATRIX_TELEGRAM_API_HASH, or put [telegram] api_id and api_hash in {} ({e})",
                path.display()
            )
        })?;
        let file: SecretsFile = toml::from_str(&text)?;
        file.telegram
            .filter(|c| c.api_id != 0 && !c.api_hash.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("{} has no [telegram] api_id and api_hash", path.display())
            })
    }
}
