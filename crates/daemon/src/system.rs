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
    /// Actions: proposed, decided, executed, recorded.
    pub actions: crate::actions::Actions,
    /// The live pairing code, if any (design 06, "形态").
    pub pairing: crate::web::Pairing,
    /// The key the gateway checks tickets with, kept so the model side can be
    /// started later in the life of the process, after a download.
    pub ticket_key: TicketKey,
    /// The running connector tasks, so an account change can stop them and
    /// start them again with the new accounts (design 05, "在界面上接入").
    pub connector_tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Telegram sign-ins in progress from the page, by id, with when they
    /// began; a sign-in holds a connection open between its steps.
    pub logins: tokio::sync::Mutex<
        std::collections::HashMap<
            String,
            (
                std::time::Instant,
                genatrix_connector_telegram::login::LoginFlow,
            ),
        >,
    >,
    /// How the model download is going (design 04, "模型下载").
    pub download: std::sync::Mutex<crate::download::Progress>,
    /// Numbers that are slow to compute and slow to change: bytes on disk,
    /// bytes that left the device. Refreshed at most every half minute
    /// for a page that asks every five seconds.
    pub slow_status: std::sync::Mutex<Option<(std::time::Instant, SlowStatus)>>,
    /// Who the user talks to and how much, which is arithmetic over every
    /// item there is. It costs two passes over the corpus and changes only
    /// when mail arrives, so it is worked out at most once a minute. What
    /// the user can change from the page, the roles and the notes, is not
    /// in here and is read afresh every time.
    pub people: std::sync::Mutex<Option<(std::time::Instant, Vec<genatrix_store::PersonOverview>)>>,
    /// The same for groups and channels: size and last activity of every
    /// multi-party container, which is a pass over all their messages.
    pub groups: std::sync::Mutex<Option<(std::time::Instant, Vec<genatrix_store::GroupOverview>)>>,
}

/// The status numbers worth caching.
#[derive(Clone, Copy, Debug, Default)]
pub struct SlowStatus {
    /// Raw records and attachments on disk.
    pub bytes_on_disk: u64,
    /// Payload bytes recorded as having left the device.
    pub bytes_left_device: usize,
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
        // Which models exist and where each one runs, agreed with the gateway.
        // Absent, it is written from the built-in catalog (design 04), so a
        // first run needs nothing written by hand; present, it is the user's
        // and is left alone.
        let gateway_path = config.gateway_config_path();
        if !gateway_path.exists() {
            if let Some(parent) = gateway_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(
                &gateway_path,
                crate::download::default_gateway_config(&config),
            )?;
            tracing::info!(path = %gateway_path.display(), "wrote the default gateway configuration");
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
            EgressGate::new(
                rules,
                gateway.registry.clone(),
                ledger.clone(),
                ticket_key.clone(),
            )
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
            slow_status: std::sync::Mutex::new(None),
            people: std::sync::Mutex::new(None),
            groups: std::sync::Mutex::new(None),
            actions: crate::actions::Actions::default(),
            pairing: crate::web::Pairing::default(),
            ticket_key,
            connector_tasks: std::sync::Mutex::new(Vec::new()),
            logins: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            download: std::sync::Mutex::new(crate::download::Progress::default()),
        })
    }
}

/// A whole core in a temporary directory, for tests.
#[cfg(test)]
pub(crate) fn test_system() -> (tempfile::TempDir, std::sync::Arc<System>) {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::config::Config::under(dir.path());
    config.create_dirs().unwrap();
    let run = dir.path().join("run");
    std::fs::write(
        config.gateway_config_path(),
        format!(
            r#"socket = "{}"

[[models]]
name = "local"
model = "test"
context_length = 4096
purposes = ["classify", "extract", "embed", "identity_suggestion", "summarize", "draft", "translate", "search_rewrite", "plan"]
endpoint = {{ kind = "local_socket", path = "{}" }}
"#,
            run.join("g.sock").display(),
            run.join("i.sock").display()
        ),
    )
    .unwrap();
    let system = System::open(config, genatrix_keys::TicketKey::generate().unwrap()).unwrap();
    (dir, std::sync::Arc::new(system))
}
