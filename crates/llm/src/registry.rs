//! Which models exist, where they run, and which role they serve.
//!
//! Design: `docs/design/04-model-layer.md`. The agent layer asks for a role,
//! never for a model. The registry turns a role into an ordered chain of
//! candidates; the first one the policy allows is used, and the rest are the
//! fallback. A configuration that puts a corpus-reading role on a cloud
//! model is rejected at startup, not at request time.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::ticket::{Purpose, TicketLevel};

/// Where a provider runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Location {
    /// This machine: the sandboxed inference process. Nothing leaves.
    Local,
    /// Someone else's machine.
    Cloud,
}

/// How to reach a provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Endpoint {
    /// The local inference process, over a Unix socket.
    LocalSocket {
        /// Socket path.
        path: String,
    },
    /// An OpenAI-compatible HTTP endpoint.
    OpenAi {
        /// Base URL.
        base_url: String,
        /// Name of the keychain entry holding the API key.
        key_ref: String,
    },
    /// An Anthropic messages endpoint.
    Anthropic {
        /// Base URL.
        base_url: String,
        /// Name of the keychain entry holding the API key.
        key_ref: String,
    },
}

impl Endpoint {
    /// Where this endpoint runs.
    #[must_use]
    pub const fn location(&self) -> Location {
        match self {
            Self::LocalSocket { .. } => Location::Local,
            Self::OpenAi { .. } | Self::Anthropic { .. } => Location::Cloud,
        }
    }
}

/// One model the gateway can serve.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    /// Registry name, unique. This is what a ticket's `target` refers to.
    pub name: String,
    /// How to reach it.
    pub endpoint: Endpoint,
    /// Model identifier to send to the endpoint.
    pub model: String,
    /// Roles this model may serve, as purposes.
    pub purposes: Vec<Purpose>,
    /// Context window in tokens. Advisory; the agent layer budgets with it.
    pub context_length: u32,
}

impl ModelEntry {
    /// Where it runs.
    #[must_use]
    pub const fn location(&self) -> Location {
        self.endpoint.location()
    }
}

/// A misconfiguration. All of these are refused at startup: a bad registry
/// must never become a runtime surprise.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    /// Two entries share a name.
    #[error("duplicate model name `{0}`")]
    DuplicateName(String),
    /// A cloud entry claims a purpose that never leaves the device.
    #[error(
        "model `{model}` is in the cloud but claims purpose `{purpose}`, which is always local"
    )]
    CloudClaimsLocalPurpose {
        /// Offending entry.
        model: String,
        /// Offending purpose.
        purpose: &'static str,
    },
    /// A chain names a model that does not exist.
    #[error("chain for `{purpose}` names unknown model `{model}`")]
    UnknownModel {
        /// The purpose whose chain is broken.
        purpose: &'static str,
        /// The name that does not resolve.
        model: String,
    },
    /// A purpose has no model at all.
    #[error("purpose `{0}` has no model")]
    NoModel(&'static str),
    /// A purpose that must stay local has no local model to fall back to.
    #[error("purpose `{0}` has no local model, so a refused cloud request could not fall back")]
    NoLocalFallback(&'static str),
}

/// The model registry: entries plus a preference chain per purpose.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Registry {
    /// Every model, in configuration order.
    pub models: Vec<ModelEntry>,
    /// Preference chains, purpose to model names, most preferred first.
    #[serde(default)]
    pub chains: BTreeMap<String, Vec<String>>,
}

impl Registry {
    /// Check the whole configuration. Called once at startup.
    pub fn validate(&self) -> Result<(), RegistryError> {
        let mut seen = BTreeSet::new();
        for m in &self.models {
            if !seen.insert(m.name.clone()) {
                return Err(RegistryError::DuplicateName(m.name.clone()));
            }
            if m.location() == Location::Cloud
                && let Some(p) = m.purposes.iter().find(|p| !p.may_use_cloud())
            {
                return Err(RegistryError::CloudClaimsLocalPurpose {
                    model: m.name.clone(),
                    purpose: p.as_str(),
                });
            }
        }
        for purpose in ALL_PURPOSES {
            let chain = self.chain(*purpose);
            if chain.is_empty() {
                return Err(RegistryError::NoModel(purpose.as_str()));
            }
            for name in &chain {
                if !self.models.iter().any(|m| &m.name == name) {
                    return Err(RegistryError::UnknownModel {
                        purpose: purpose.as_str(),
                        model: name.clone(),
                    });
                }
            }
            let has_local = chain.iter().any(|n| {
                self.models
                    .iter()
                    .any(|m| &m.name == n && m.location() == Location::Local)
            });
            if !has_local {
                return Err(RegistryError::NoLocalFallback(purpose.as_str()));
            }
        }
        Ok(())
    }

    /// The ordered candidates for a purpose. An explicit chain wins;
    /// otherwise every model that claims the purpose, local ones last so the
    /// fallback is always available.
    #[must_use]
    pub fn chain(&self, purpose: Purpose) -> Vec<String> {
        if let Some(chain) = self.chains.get(purpose.as_str()) {
            return chain.clone();
        }
        let mut cloud = Vec::new();
        let mut local = Vec::new();
        for m in self.models.iter().filter(|m| m.purposes.contains(&purpose)) {
            if m.location() == Location::Local {
                local.push(m.name.clone());
            } else {
                cloud.push(m.name.clone());
            }
        }
        cloud.extend(local);
        cloud
    }

    /// Look up an entry by registry name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.name == name)
    }

    /// Pick the model to use for a purpose at a level: the first candidate
    /// the policy would allow. Used by the gate when it mints a ticket, so
    /// that a request which could not be served in the cloud is never even
    /// addressed to one.
    #[must_use]
    pub fn resolve(&self, purpose: Purpose, level: TicketLevel) -> Option<&ModelEntry> {
        self.chain(purpose).into_iter().find_map(|name| {
            let entry = self.get(&name)?;
            if entry.location() == Location::Local
                || crate::policy::cloud_is_possible(purpose, level)
            {
                Some(entry)
            } else {
                None
            }
        })
    }

    /// The first local candidate for a purpose. Used when the cloud is
    /// switched off, and as the fallback when a cloud request is refused.
    #[must_use]
    pub fn resolve_local(&self, purpose: Purpose) -> Option<&ModelEntry> {
        self.chain(purpose)
            .into_iter()
            .find_map(|name| self.get(&name).filter(|e| e.location() == Location::Local))
    }
}

/// Every purpose, for validation.
pub const ALL_PURPOSES: &[Purpose] = &[
    Purpose::Classify,
    Purpose::Extract,
    Purpose::Embed,
    Purpose::IdentitySuggestion,
    Purpose::Summarize,
    Purpose::Draft,
    Purpose::Translate,
    Purpose::SearchRewrite,
    Purpose::Plan,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn local(name: &str, purposes: Vec<Purpose>) -> ModelEntry {
        ModelEntry {
            name: name.into(),
            endpoint: Endpoint::LocalSocket {
                path: "/tmp/gx/infer.sock".into(),
            },
            model: "qwen3-8b-4bit".into(),
            purposes,
            context_length: 32_768,
        }
    }

    fn cloud(name: &str, purposes: Vec<Purpose>) -> ModelEntry {
        ModelEntry {
            name: name.into(),
            endpoint: Endpoint::Anthropic {
                base_url: "https://api.anthropic.com".into(),
                key_ref: "anthropic".into(),
            },
            model: "claude-sonnet-5".into(),
            purposes,
            context_length: 200_000,
        }
    }

    fn all() -> Vec<Purpose> {
        ALL_PURPOSES.to_vec()
    }

    #[test]
    fn a_local_only_registry_is_valid() {
        let r = Registry {
            models: vec![local("local", all())],
            chains: BTreeMap::new(),
        };
        r.validate().unwrap();
        assert_eq!(r.chain(Purpose::Draft), ["local"]);
    }

    #[test]
    fn cloud_claiming_a_local_only_purpose_is_refused_at_startup() {
        let r = Registry {
            models: vec![
                local("local", all()),
                cloud("cloud", vec![Purpose::Classify]),
            ],
            chains: BTreeMap::new(),
        };
        let err = r.validate().unwrap_err();
        assert!(
            matches!(err, RegistryError::CloudClaimsLocalPurpose { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn a_purpose_without_a_local_fallback_is_refused() {
        let r = Registry {
            models: vec![
                local(
                    "local",
                    vec![
                        Purpose::Classify,
                        Purpose::Extract,
                        Purpose::Embed,
                        Purpose::IdentitySuggestion,
                    ],
                ),
                cloud(
                    "cloud",
                    vec![
                        Purpose::Summarize,
                        Purpose::Draft,
                        Purpose::Translate,
                        Purpose::SearchRewrite,
                        Purpose::Plan,
                    ],
                ),
            ],
            chains: BTreeMap::new(),
        };
        let err = r.validate().unwrap_err();
        assert!(matches!(err, RegistryError::NoLocalFallback(_)), "{err:?}");
    }

    #[test]
    fn duplicate_names_and_dangling_chains_are_refused() {
        let dup = Registry {
            models: vec![local("m", all()), local("m", all())],
            chains: BTreeMap::new(),
        };
        assert!(matches!(
            dup.validate(),
            Err(RegistryError::DuplicateName(_))
        ));

        let mut chains = BTreeMap::new();
        chains.insert("draft".to_string(), vec!["nope".to_string()]);
        let dangling = Registry {
            models: vec![local("m", all())],
            chains,
        };
        assert!(matches!(
            dangling.validate(),
            Err(RegistryError::UnknownModel { .. })
        ));
    }

    #[test]
    fn default_chain_puts_cloud_first_and_keeps_local_as_fallback() {
        let r = Registry {
            models: vec![local("local", all()), cloud("cloud", vec![Purpose::Draft])],
            chains: BTreeMap::new(),
        };
        r.validate().unwrap();
        assert_eq!(r.chain(Purpose::Draft), ["cloud", "local"]);
        assert_eq!(r.chain(Purpose::Classify), ["local"]);
    }

    #[test]
    fn resolve_skips_cloud_when_the_content_may_not_leave() {
        let r = Registry {
            models: vec![local("local", all()), cloud("cloud", vec![Purpose::Draft])],
            chains: BTreeMap::new(),
        };
        assert_eq!(
            r.resolve(Purpose::Draft, TicketLevel::Redacted)
                .unwrap()
                .name,
            "cloud"
        );
        assert_eq!(
            r.resolve(Purpose::Draft, TicketLevel::Secret).unwrap().name,
            "local"
        );
        assert_eq!(
            r.resolve(Purpose::Classify, TicketLevel::Public)
                .unwrap()
                .name,
            "local",
            "a corpus purpose never resolves to the cloud"
        );
    }
}
