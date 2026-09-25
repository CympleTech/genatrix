//! The sandbox: a fresh WASM instance per run, only the doors the core
//! opens, and limits on instructions, memory and time (design 11).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use genatrix_model::Level;
use wasmtime::component::{Component, HasData, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::HostError;
use crate::manifest::{BlobAccess, Effect, Manifest, Purpose, Trigger};
use crate::package::Package;

#[allow(missing_docs, clippy::pedantic)]
mod bindings {
    wasmtime::component::bindgen!({
        world: "agent",
        path: "../../wit",
    });
}

use bindings::genatrix::agent as doors;
/// The types that cross a door, as the WIT file defines them.
pub use bindings::genatrix::agent::types as wit;

/// How often the clock that bounds wall time ticks.
const TICK: Duration = Duration::from_millis(10);
/// Instructions allowed per second of the quota: a ceiling on work that
/// holds even if the clock thread falls behind.
const FUEL_PER_SECOND: u64 = 2_000_000_000;
/// The most items one query returns.
const QUERY_LIMIT: u32 = 200;
/// Lines of log a run may write, and the length of each.
const LOG_LINES: usize = 1000;
const LOG_LINE_CHARS: usize = 2000;

/// What the daemon provides behind each door. The host has already
/// checked what it can without data: the entry point, the manifest's
/// purposes, kinds and attachment grant. The daemon checks the rest.
pub trait Doors: Send {
    /// Items in scope matching the filter. Each item carries its level.
    fn query(&mut self, filter: &wit::Filter) -> Vec<wit::Item>;
    /// One item, if it is in scope.
    fn get(&mut self, id: &str) -> Option<wit::Item>;
    /// An attachment's text and the level of the item it belongs to.
    fn blob_text(&mut self, id: &str) -> Result<(String, Level), String>;
    /// A model call through the egress gate at the run's level.
    fn call_model(
        &mut self,
        purpose: Purpose,
        messages: Vec<wit::Message>,
        level: Level,
    ) -> Result<String, String>;
    /// A statement in the agent's own space.
    fn execute(&mut self, sql: &str, params: Vec<wit::Value>) -> Result<u64, String>;
    /// A query in the agent's own space.
    fn query_space(
        &mut self,
        sql: &str,
        params: Vec<wit::Value>,
    ) -> Result<Vec<Vec<wit::Value>>, String>;
    /// A proposal, stored as a pending action. Returns its id.
    fn propose(
        &mut self,
        kind: &str,
        card: wit::Card,
        payload: String,
        level: Level,
    ) -> Result<String, String>;
}

/// Why the agent is being run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invocation {
    /// New items in scope.
    Items(Vec<String>),
    /// The user wrote to it.
    Message(String),
    /// A declared schedule came due.
    Schedule(String),
    /// An approved proposal of its own kind.
    Apply {
        /// The kind.
        kind: String,
        /// What it proposed.
        payload: String,
    },
}

/// One run's inputs besides the doors.
#[derive(Clone, Debug)]
pub struct Run {
    /// Why.
    pub invocation: Invocation,
    /// The time the agent sees, fixed for the run.
    pub now_ms: i64,
    /// Seeds the agent's random numbers, so a run can be replayed.
    pub seed: u64,
    /// The level of the agent's space; the run's level starts here.
    pub space_level: Level,
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunError {
    /// The run did not match a declared trigger, or the component asks for
    /// something the core does not offer, such as a network.
    Refused(String),
    /// Out of instructions, time or memory.
    Limit(String),
    /// The agent crashed.
    Trap(String),
    /// The agent returned an error of its own.
    Agent(String),
}

/// What a run leaves behind, for the run record.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// The answer to a message, if any, or why it failed.
    pub result: Result<Option<String>, RunError>,
    /// What it logged.
    pub log: Vec<String>,
    /// The highest level of anything it read. Its space rises to this.
    pub taint: Level,
    /// Item ids it read through a door.
    pub reads: Vec<String>,
    /// Proposals it made.
    pub proposals: Vec<String>,
    /// Instructions spent.
    pub fuel_used: u64,
}

/// Compiles and runs packages. One per core; cheap to share.
pub struct Runner {
    engine: Engine,
    linker: Linker<RunState>,
    compiled: Mutex<HashMap<String, Component>>,
}

impl Runner {
    /// A runner with the sandbox configured and the clock started.
    pub fn new() -> Result<Self, HostError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).map_err(|e| HostError::Engine(e.to_string()))?;

        let weak = engine.weak();
        std::thread::Builder::new()
            .name("genatrix-host-clock".into())
            .spawn(move || {
                while let Some(engine) = weak.upgrade() {
                    engine.increment_epoch();
                    drop(engine);
                    std::thread::sleep(TICK);
                }
            })
            .map_err(|e| HostError::Engine(e.to_string()))?;

        let mut linker = Linker::new(&engine);
        link_wasi(&mut linker).map_err(|e| HostError::Engine(e.to_string()))?;
        bindings::Agent::add_to_linker::<_, HasSelf<RunState>>(&mut linker, |s| s)
            .map_err(|e| HostError::Engine(e.to_string()))?;
        Ok(Self {
            engine,
            linker,
            compiled: Mutex::new(HashMap::new()),
        })
    }

    fn component(&self, package: &Package) -> Result<Component, RunError> {
        let mut cache = self
            .compiled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(c) = cache.get(&package.hash) {
            return Ok(c.clone());
        }
        let c = Component::new(&self.engine, &package.bytes)
            .map_err(|e| RunError::Refused(format!("not a valid component: {e}")))?;
        cache.insert(package.hash.clone(), c.clone());
        Ok(c)
    }

    /// Run a package once. Never panics on anything the agent does.
    pub fn run(&self, package: &Package, run: Run, doors: Box<dyn Doors>) -> Outcome {
        let manifest = Arc::new(package.manifest.clone());
        let fail = |e: RunError| Outcome {
            result: Err(e),
            log: Vec::new(),
            taint: run.space_level,
            reads: Vec::new(),
            proposals: Vec::new(),
            fuel_used: 0,
        };
        if let Err(e) = admitted(&manifest, &run.invocation) {
            return fail(e);
        }
        let component = match self.component(package) {
            Ok(c) => c,
            Err(e) => return fail(e),
        };

        let quota = manifest.quota;
        let limits = StoreLimitsBuilder::new()
            .memory_size(usize::try_from(quota.memory_mb).unwrap_or(16) << 20)
            .instances(64)
            .memories(16)
            .tables(64)
            .table_elements(1 << 20)
            .trap_on_grow_failure(true)
            .build();
        let wasi = WasiCtxBuilder::new()
            .wall_clock(FixedClock(run.now_ms))
            .secure_random(wasmtime_wasi::Deterministic::new(seed_bytes(run.seed, 0)))
            .insecure_random(wasmtime_wasi::Deterministic::new(seed_bytes(run.seed, 1)))
            .insecure_random_seed(u128::from(run.seed))
            .stdout(wasmtime_wasi::p2::pipe::SinkOutputStream)
            .stderr(wasmtime_wasi::p2::pipe::SinkOutputStream)
            .allow_tcp(false)
            .allow_udp(false)
            .allow_ip_name_lookup(false)
            .build();
        let applying = matches!(run.invocation, Invocation::Apply { .. });
        let state = RunState {
            manifest: Arc::clone(&manifest),
            doors,
            applying,
            taint: run.space_level,
            reads: BTreeSet::new(),
            log: Vec::new(),
            proposals: Vec::new(),
            now_ms: run.now_ms,
            wasi,
            table: ResourceTable::new(),
            limits,
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limits);
        let fuel = u64::from(quota.seconds) * FUEL_PER_SECOND;
        let _ = store.set_fuel(fuel);
        let ticks =
            u64::from(quota.seconds) * (1000 / u64::try_from(TICK.as_millis()).unwrap_or(10));
        store.set_epoch_deadline(ticks);
        store.epoch_deadline_trap();

        let result = match bindings::Agent::instantiate(&mut store, &component, &self.linker) {
            Err(e) => Err(RunError::Refused(format!(
                "asks for something the core does not offer: {e}"
            ))),
            Ok(agent) => call(&agent, &mut store, run.invocation),
        };
        let fuel_used = fuel.saturating_sub(store.get_fuel().unwrap_or(0));
        let state = store.into_data();
        Outcome {
            result,
            log: state.log,
            taint: state.taint,
            reads: state.reads.into_iter().collect(),
            proposals: state.proposals,
            fuel_used,
        }
    }
}

fn call(
    agent: &bindings::Agent,
    store: &mut Store<RunState>,
    invocation: Invocation,
) -> Result<Option<String>, RunError> {
    let answer = match invocation {
        Invocation::Items(ids) => agent
            .call_on_items(&mut *store, &ids)
            .map(|r| r.map(|()| None)),
        Invocation::Message(text) => agent
            .call_on_message(&mut *store, &text)
            .map(|r| r.map(Some)),
        Invocation::Schedule(name) => agent
            .call_on_schedule(&mut *store, &name)
            .map(|r| r.map(|()| None)),
        Invocation::Apply { kind, payload } => agent
            .call_apply(&mut *store, &kind, &payload)
            .map(|r| r.map(|()| None)),
    };
    match answer {
        Ok(Ok(a)) => Ok(a),
        Ok(Err(e)) => Err(RunError::Agent(e)),
        Err(trap) => Err(classify_trap(&trap)),
    }
}

fn is_memory_limit(trap: &str) -> bool {
    trap.contains("memory") && (trap.contains("grow") || trap.contains("limit"))
}

fn classify_trap(err: &wasmtime::Error) -> RunError {
    match err.downcast_ref::<wasmtime::Trap>() {
        Some(wasmtime::Trap::OutOfFuel) => RunError::Limit("ran out of instructions".into()),
        Some(wasmtime::Trap::Interrupt) => RunError::Limit("ran out of time".into()),
        _ => {
            let text = format!("{err:?}");
            if is_memory_limit(&text) {
                RunError::Limit("ran out of memory".into())
            } else {
                RunError::Trap(err.to_string())
            }
        }
    }
}

/// Whether this invocation matches what the manifest declares.
fn admitted(manifest: &Manifest, invocation: &Invocation) -> Result<(), RunError> {
    let declared = |want: fn(&Trigger) -> bool| manifest.triggers.iter().any(want);
    let ok = match invocation {
        Invocation::Items(_) => declared(|t| matches!(t, Trigger::Items)),
        Invocation::Message(_) => declared(|t| matches!(t, Trigger::Message)),
        Invocation::Schedule(name) => manifest
            .triggers
            .iter()
            .any(|t| matches!(t, Trigger::Schedule { name: n, .. } if n == name)),
        Invocation::Apply { kind, .. } => manifest
            .proposal(kind)
            .is_some_and(|p| p.effect == Effect::Own),
    };
    if ok {
        Ok(())
    } else {
        Err(RunError::Refused(format!(
            "not declared in the manifest: {invocation:?}"
        )))
    }
}

fn seed_bytes(seed: u64, stream: u8) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(seed.to_le_bytes());
    h.update([stream]);
    h.finalize().to_vec()
}

struct FixedClock(i64);

impl wasmtime_wasi::HostWallClock for FixedClock {
    fn resolution(&self) -> Duration {
        Duration::from_millis(1)
    }
    fn now(&self) -> Duration {
        Duration::from_millis(u64::try_from(self.0).unwrap_or(0))
    }
}

struct RunState {
    manifest: Arc<Manifest>,
    doors: Box<dyn Doors>,
    applying: bool,
    taint: Level,
    reads: BTreeSet<String>,
    log: Vec<String>,
    proposals: Vec<String>,
    now_ms: i64,
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
}

impl RunState {
    fn raise(&mut self, level: Level) {
        self.taint = self.taint.max(level);
    }

    fn note(&mut self, line: &str) {
        if self.log.len() < LOG_LINES {
            self.log.push(line.chars().take(LOG_LINE_CHARS).collect());
        }
    }

    /// Keep what the manifest allows and remember what was read.
    fn admit(&mut self, item: wit::Item) -> Option<wit::Item> {
        let level = level_of(item.level);
        if level > self.manifest.reads.max_level {
            return None;
        }
        self.raise(level);
        self.reads.insert(item.id.clone());
        Some(item)
    }
}

impl WasiView for RunState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

fn level_of(l: wit::Level) -> Level {
    match l {
        wit::Level::Public => Level::Public,
        wit::Level::Personal => Level::Personal,
        wit::Level::Secret => Level::Secret,
    }
}

fn purpose_of(p: wit::Purpose) -> Purpose {
    match p {
        wit::Purpose::Classify => Purpose::Classify,
        wit::Purpose::Extract => Purpose::Extract,
        wit::Purpose::Summarize => Purpose::Summarize,
        wit::Purpose::Draft => Purpose::Draft,
        wit::Purpose::Translate => Purpose::Translate,
    }
}

impl wit::Host for RunState {}

impl doors::items::Host for RunState {
    fn query(&mut self, mut filter: wit::Filter) -> Vec<wit::Item> {
        if self.applying {
            self.note("refused: items are not readable while applying");
            return Vec::new();
        }
        filter.limit = filter.limit.clamp(1, QUERY_LIMIT);
        let found = self.doors.query(&filter);
        found
            .into_iter()
            .take(filter.limit as usize)
            .filter_map(|i| self.admit(i))
            .collect()
    }

    fn get(&mut self, id: String) -> Option<wit::Item> {
        if self.applying {
            return None;
        }
        let item = self.doors.get(&id)?;
        if item.id != id {
            return None;
        }
        self.admit(item)
    }
}

impl doors::blobs::Host for RunState {
    fn text(&mut self, id: String) -> Result<String, String> {
        if self.applying {
            return Err("not while applying".into());
        }
        if self.manifest.blobs != BlobAccess::Text {
            return Err("attachment text is not declared in the manifest".into());
        }
        let (text, level) = self.doors.blob_text(&id)?;
        if level > self.manifest.reads.max_level {
            return Err("not in scope".into());
        }
        self.raise(level);
        Ok(text)
    }
}

impl doors::model::Host for RunState {
    fn call(
        &mut self,
        purpose: wit::Purpose,
        messages: Vec<wit::Message>,
    ) -> Result<String, String> {
        if self.applying {
            return Err("not while applying".into());
        }
        let purpose = purpose_of(purpose);
        if !self.manifest.may_call(purpose) {
            return Err(format!("{purpose:?} is not declared in the manifest"));
        }
        if messages
            .iter()
            .any(|m| !matches!(m.role.as_str(), "system" | "user"))
        {
            return Err("roles are system or user".into());
        }
        let level = self.taint;
        self.doors.call_model(purpose, messages, level)
    }
}

impl doors::space::Host for RunState {
    fn execute(&mut self, sql: String, params: Vec<wit::Value>) -> Result<u64, String> {
        self.doors.execute(&sql, params)
    }

    fn query(
        &mut self,
        sql: String,
        params: Vec<wit::Value>,
    ) -> Result<Vec<Vec<wit::Value>>, String> {
        self.doors.query_space(&sql, params)
    }
}

impl doors::actions::Host for RunState {
    fn propose(
        &mut self,
        kind: String,
        card: wit::Card,
        payload: String,
    ) -> Result<String, String> {
        if self.applying {
            return Err("not while applying".into());
        }
        if self.manifest.proposal(&kind).is_none() {
            return Err(format!("`{kind}` is not declared in the manifest"));
        }
        let level = self.taint;
        let id = self.doors.propose(&kind, card, payload, level)?;
        self.proposals.push(id.clone());
        Ok(id)
    }
}

impl doors::host::Host for RunState {
    fn now_ms(&mut self) -> i64 {
        self.now_ms
    }

    fn log(&mut self, line: String) {
        self.note(&line);
    }
}

/// WASI, interface by interface. Filesystem types are linked but no
/// directory is opened, so nothing is reachable. Sockets and HTTP are not
/// linked at all: a component that imports them does not instantiate.
fn link_wasi(l: &mut Linker<RunState>) -> wasmtime::Result<()> {
    use wasmtime_wasi::cli::{WasiCli, WasiCliView as _};
    use wasmtime_wasi::clocks::{WasiClocks, WasiClocksView as _};
    use wasmtime_wasi::filesystem::{WasiFilesystem, WasiFilesystemView as _};
    use wasmtime_wasi::p2::bindings::{cli, clocks, filesystem, random, sync};
    use wasmtime_wasi::random::WasiRandom;

    struct Io;
    impl HasData for Io {
        type Data<'a> = &'a mut ResourceTable;
    }

    wasmtime_wasi_io::bindings::wasi::io::error::add_to_linker::<RunState, Io>(l, |t| {
        &mut t.table
    })?;
    sync::io::poll::add_to_linker::<RunState, Io>(l, |t| &mut t.table)?;
    sync::io::streams::add_to_linker::<RunState, Io>(l, |t| &mut t.table)?;

    clocks::wall_clock::add_to_linker::<RunState, WasiClocks>(l, RunState::clocks)?;
    clocks::monotonic_clock::add_to_linker::<RunState, WasiClocks>(l, RunState::clocks)?;
    random::random::add_to_linker::<RunState, WasiRandom>(l, |t| t.wasi.random())?;
    random::insecure::add_to_linker::<RunState, WasiRandom>(l, |t| t.wasi.random())?;
    random::insecure_seed::add_to_linker::<RunState, WasiRandom>(l, |t| t.wasi.random())?;
    cli::exit::add_to_linker::<RunState, WasiCli>(l, RunState::cli)?;
    cli::environment::add_to_linker::<RunState, WasiCli>(l, RunState::cli)?;
    cli::stdin::add_to_linker::<RunState, WasiCli>(l, RunState::cli)?;
    cli::stdout::add_to_linker::<RunState, WasiCli>(l, RunState::cli)?;
    cli::stderr::add_to_linker::<RunState, WasiCli>(l, RunState::cli)?;
    cli::terminal_input::add_to_linker::<RunState, WasiCli>(l, RunState::cli)?;
    cli::terminal_output::add_to_linker::<RunState, WasiCli>(l, RunState::cli)?;
    cli::terminal_stdin::add_to_linker::<RunState, WasiCli>(l, RunState::cli)?;
    cli::terminal_stdout::add_to_linker::<RunState, WasiCli>(l, RunState::cli)?;
    cli::terminal_stderr::add_to_linker::<RunState, WasiCli>(l, RunState::cli)?;
    filesystem::preopens::add_to_linker::<RunState, WasiFilesystem>(l, RunState::filesystem)?;
    sync::filesystem::types::add_to_linker::<RunState, WasiFilesystem>(l, RunState::filesystem)?;
    Ok(())
}
