//! Gateway configuration.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::registry::Registry;

/// What the gateway process needs to start.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// Unix socket the gateway listens on. Keep the path short: macOS caps
    /// socket paths at 104 bytes.
    pub socket: PathBuf,
    /// Models and preference chains.
    #[serde(flatten)]
    pub registry: Registry,
}

/// Configuration problems.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("reading {path}: {source}")]
    Read {
        /// Path.
        path: PathBuf,
        /// Cause.
        source: std::io::Error,
    },
    /// The file is not valid TOML or does not match the schema.
    #[error("parsing {path}: {source}")]
    Parse {
        /// Path.
        path: PathBuf,
        /// Cause.
        source: toml::de::Error,
    },
    /// The registry itself is inconsistent.
    #[error(transparent)]
    Registry(#[from] crate::registry::RegistryError),
}

impl Config {
    /// Read and validate a configuration file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Self = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        config.registry.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Location;
    use crate::ticket::Purpose;

    const SAMPLE: &str = r#"
socket = "/tmp/gx/gateway.sock"

[[models]]
name = "local"
model = "qwen3-8b-4bit"
context_length = 32768
purposes = ["classify", "extract", "embed", "identity_suggestion",
            "summarize", "draft", "translate", "search_rewrite", "plan"]
endpoint = { kind = "local_socket", path = "/tmp/gx/infer.sock" }
"#;

    #[test]
    fn a_local_only_config_parses_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gateway.toml");
        std::fs::write(&path, SAMPLE).unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(c.socket, PathBuf::from("/tmp/gx/gateway.sock"));
        assert_eq!(c.registry.models.len(), 1);
        assert_eq!(c.registry.models[0].location(), Location::Local);
        assert_eq!(c.registry.chain(Purpose::Draft), ["local"]);
    }

    #[test]
    fn a_config_that_would_send_classification_to_the_cloud_is_refused() {
        let bad = format!(
            "{SAMPLE}
[[models]]
name = \"cloud\"
model = \"claude-sonnet-5\"
context_length = 200000
purposes = [\"classify\"]
endpoint = {{ kind = \"anthropic\", base_url = \"https://api.anthropic.com\", key_ref = \"anthropic\" }}
"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gateway.toml");
        std::fs::write(&path, bad).unwrap();
        let err = Config::load(&path).unwrap_err();
        assert!(matches!(err, ConfigError::Registry(_)), "{err:?}");
    }
}
