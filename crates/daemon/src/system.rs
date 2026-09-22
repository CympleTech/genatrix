//! Assembling the layers into one running thing.
//!
//! This is the only place that knows about all of them, which is the point:
//! every crate below is ignorant of the ones beside it, and the shape of the
//! system lives here rather than being spread through them.

use std::sync::Arc;

use genatrix_gate::gate::EgressGate;
use genatrix_gate::rules::RuleSet;
use genatrix_keys::TicketKey;
use genatrix_ledger::Ledger;
use genatrix_llm::config::Config as GatewayConfig;
use genatrix_store::{FileStore, Store};

use crate::caller::GatewayCaller;
use crate::config::Config;
use crate::keys;
use crate::names::StoreNames;

/// Everything open and wired together.
pub struct System {
    /// Where things live.
    pub config: Config,
    /// The main database.
    pub store: Arc<Store>,
    /// Raw records as fetched, encrypted, one file each.
    pub raw_files: Arc<FileStore>,
    /// Attachments and other binaries, encrypted, one file each.
    pub blob_files: Arc<FileStore>,
    /// The append-only ledger.
    pub ledger: Arc<Ledger>,
    /// The egress gate.
    pub gate: Arc<EgressGate>,
    /// The caller the agent layer uses.
    pub caller: GatewayCaller<StoreNames>,
    /// How each account's sync is doing, for the interface.
    pub accounts: crate::syncing::Accounts,
    /// The gateway's configuration: which models exist and where they run.
    pub gateway_config: GatewayConfig,
    /// How the model side is doing, for the interface and the pipelines.
    pub model_state: tokio::sync::watch::Sender<crate::models::ModelState>,
}

impl std::fmt::Debug for System {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("System")
            .field("data_dir", &self.config.data_dir)
            .finish_non_exhaustive()
    }
}

impl System {
    /// Whether the registry names a local embedder (an entry whose only
    /// purpose is `embed`). Without one, search stays full-text.
    #[must_use]
    pub fn embedder_configured(&self) -> bool {
        self.gateway_config.registry.models.iter().any(|m| {
            matches!(
                m.endpoint,
                genatrix_llm::registry::Endpoint::LocalSocket { .. }
            ) && !m.purposes.is_empty()
                && m.purposes
                    .iter()
                    .all(|p| *p == genatrix_llm::ticket::Purpose::Embed)
        })
    }

    /// Open everything.
    ///
    /// The ticket key is generated here, per run, and handed to the gateway
    /// out of band. A daemon that spawned the gateway itself would pass it on
    /// the way in; until then the two are started separately and the key
    /// comes from the environment on both sides.
    pub fn open(config: Config, ticket_key: TicketKey) -> anyhow::Result<Self> {
        // The one file nobody writes for you, read before anything is
        // created: a run that fails here leaves the directory as it found it,
        // rather than half made.
        let gateway_path = config.gateway_config_path();
        if !gateway_path.exists() {
            anyhow::bail!(
                "no gateway configuration at {}.\n\
                 Write one first; the README has a template. It says which models \
                 exist and where each one runs, and this process needs to agree \
                 with the gateway about that.",
                gateway_path.display()
            );
        }
        let gateway = GatewayConfig::load(&gateway_path)?;

        config.create_dirs()?;
        let master = keys::obtain(&config)?;

        let store = Arc::new(Store::open(config.store_path(), &master.db_key("store"))?);
        let ledger = Arc::new(Ledger::open(
            config.ledger_path(),
            &master.db_key("ledger"),
        )?);
        let raw_files = Arc::new(FileStore::open(
            config.data_dir.join("raw"),
            master.clone(),
        )?);
        let blob_files = Arc::new(FileStore::open(
            config.data_dir.join("blobs"),
            master.clone(),
        )?);

        let rules = if config.rules_path().exists() {
            RuleSet::load(&config.rules_path())?
        } else {
            // Write the shipped rules out on first run so they are there to
            // be read and argued with, rather than hidden in the binary.
            std::fs::write(config.rules_path(), genatrix_gate::rules::DEFAULT_RULES)?;
            RuleSet::builtin()
        };

        // The gateway's own configuration is the authority on where it
        // listens. Deriving that path here as well would be two places
        // computing the same thing, which is how they come to disagree.
        let mut config = config;
        config.gateway_socket.clone_from(&gateway.socket);
        let gate = Arc::new(
            EgressGate::new(rules, gateway.registry.clone(), ledger.clone(), ticket_key)
                .with_cloud(config.cloud_enabled),
        );
        let caller = GatewayCaller::new(
            gate.clone(),
            config.gateway_socket.clone(),
            StoreNames::new(store.clone()),
        );

        Ok(Self {
            config,
            store,
            raw_files,
            blob_files,
            ledger,
            gate,
            caller,
            accounts: crate::syncing::Accounts::default(),
            gateway_config: gateway,
            model_state: crate::models::idle_state(),
        })
    }
}
