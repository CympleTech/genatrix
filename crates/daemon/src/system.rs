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
use genatrix_store::Store;

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
    /// The append-only ledger.
    pub ledger: Arc<Ledger>,
    /// The egress gate.
    pub gate: Arc<EgressGate>,
    /// The caller the agent layer uses.
    pub caller: GatewayCaller<StoreNames>,
}

impl std::fmt::Debug for System {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("System")
            .field("data_dir", &self.config.data_dir)
            .finish_non_exhaustive()
    }
}

impl System {
    /// Open everything.
    ///
    /// The ticket key is generated here, per run, and handed to the gateway
    /// out of band. A daemon that spawned the gateway itself would pass it on
    /// the way in; until then the two are started separately and the key
    /// comes from the environment on both sides.
    pub fn open(config: Config, ticket_key: TicketKey) -> anyhow::Result<Self> {
        config.create_dirs()?;
        let master = keys::load_or_create(&config.key_path())?;

        let store = Arc::new(Store::open(
            config.store_path(),
            &keys::db_key(&master, "store"),
        )?);
        let ledger = Arc::new(Ledger::open(
            config.ledger_path(),
            &keys::db_key(&master, "ledger"),
        )?);

        let rules = if config.rules_path().exists() {
            RuleSet::load(&config.rules_path())?
        } else {
            // Write the shipped rules out on first run so they are there to
            // be read and argued with, rather than hidden in the binary.
            std::fs::write(config.rules_path(), genatrix_gate::rules::DEFAULT_RULES)?;
            RuleSet::builtin()
        };

        let gateway = GatewayConfig::load(&config.gateway_config_path())?;
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
            ledger,
            gate,
            caller,
        })
    }
}
