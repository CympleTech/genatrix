//! The sandbox against real components built from `agents/` (rebuilt with
//! `agents/build.sh`). Design 02 invariants 13, 14, 19 and 20 as far as the
//! host can hold them without data; the daemon's side is tested there.

use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use genatrix_host::manifest::Purpose;
use genatrix_host::wit::{BlobRef, Card, Filter, Item, Level as WitLevel, Message, Value};
use genatrix_host::{Doors, Invocation, Package, Run, RunError, Runner};
use genatrix_model::Level;

static RUNNER: LazyLock<Runner> = LazyLock::new(|| Runner::new().unwrap());

fn package(name: &str) -> Package {
    let path = format!("{}/tests/fixtures/{name}.wasm", env!("CARGO_MANIFEST_DIR"));
    Package::read(std::fs::read(path).unwrap()).unwrap()
}

fn item(id: &str, level: WitLevel) -> Item {
    Item {
        id: id.into(),
        connector: "imap".into(),
        kind: "mail".into(),
        direction: "inbound".into(),
        occurred_ms: 0,
        author: Some("Alice".into()),
        title: Some(format!("subject {id}")),
        text: "body".into(),
        blobs: vec![BlobRef {
            id: format!("{id}-blob"),
            name: None,
            mime: "application/pdf".into(),
            size: 1,
        }],
        level,
    }
}

/// What the doors saw, shared with the test after the run.
#[derive(Default)]
struct Seen {
    model_levels: Vec<Level>,
    proposals: Vec<(String, Level)>,
    statements: Vec<String>,
}

struct Fake {
    items: Vec<Item>,
    seen: Arc<Mutex<Seen>>,
}

impl Fake {
    fn new(items: Vec<Item>) -> (Box<Self>, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        (
            Box::new(Self {
                items,
                seen: Arc::clone(&seen),
            }),
            seen,
        )
    }
}

impl Doors for Fake {
    fn query(&mut self, _filter: &Filter) -> Vec<Item> {
        self.items.clone()
    }
    fn get(&mut self, id: &str) -> Option<Item> {
        self.items.iter().find(|i| i.id == id).cloned()
    }
    fn blob_text(&mut self, _id: &str) -> Result<(String, Level), String> {
        Ok(("text".into(), Level::Personal))
    }
    fn call_model(
        &mut self,
        _purpose: Purpose,
        _messages: Vec<Message>,
        level: Level,
    ) -> Result<String, String> {
        self.seen.lock().unwrap().model_levels.push(level);
        Ok("model says hi".into())
    }
    fn execute(&mut self, sql: &str, _params: Vec<Value>) -> Result<u64, String> {
        self.seen.lock().unwrap().statements.push(sql.to_owned());
        Ok(1)
    }
    fn query_space(&mut self, _sql: &str, _params: Vec<Value>) -> Result<Vec<Vec<Value>>, String> {
        let n = self
            .seen
            .lock()
            .unwrap()
            .statements
            .iter()
            .filter(|s| s.starts_with("INSERT OR IGNORE INTO seen"))
            .count();
        Ok(vec![vec![Value::Integer(i64::try_from(n).unwrap())]])
    }
    fn propose(
        &mut self,
        kind: &str,
        _card: Card,
        _payload: String,
        level: Level,
    ) -> Result<String, String> {
        let mut seen = self.seen.lock().unwrap();
        seen.proposals.push((kind.to_owned(), level));
        Ok(format!("action-{}", seen.proposals.len()))
    }
}

fn run_at(
    name: &str,
    invocation: Invocation,
    space_level: Level,
    items: Vec<Item>,
) -> (genatrix_host::Outcome, Arc<Mutex<Seen>>) {
    let (doors, seen) = Fake::new(items);
    let run = Run {
        invocation,
        now_ms: 1_758_000_000_000,
        seed: 7,
        space_level,
    };
    (RUNNER.run(&package(name), run, doors), seen)
}

fn ask(message: &str) -> Result<Option<String>, RunError> {
    run_at(
        "probe",
        Invocation::Message(message.into()),
        Level::Public,
        Vec::new(),
    )
    .0
    .result
}

fn answer(message: &str) -> String {
    ask(message).unwrap().unwrap()
}

#[test]
fn a_component_that_imports_sockets_does_not_start() {
    // Invariant 13: no network, because sockets are not linked at all.
    let (outcome, _) = run_at(
        "net",
        Invocation::Message(String::new()),
        Level::Public,
        Vec::new(),
    );
    match outcome.result {
        Err(RunError::Refused(why)) => assert!(why.contains("sockets"), "{why}"),
        other => panic!("expected refusal, got {other:?}"),
    }
}

#[test]
fn no_files_no_directories_no_environment() {
    // Invariant 13: the file system is linked with nothing opened.
    assert!(answer("file:/etc/passwd").starts_with("no file"));
    assert!(answer("file:tests/sandbox.rs").starts_with("no file"));
    assert!(answer("dir").starts_with("no dir"));
    assert_eq!(answer("env"), "0 variables");
}

#[test]
fn the_clock_is_the_run_clock() {
    // Replayable: the agent's wall clock is the run's time.
    let a = answer("time");
    let (wall, host) = a.split_once(' ').unwrap();
    assert_eq!(wall, "1758000000000");
    assert_eq!(host, "1758000000000");
}

#[test]
fn an_endless_loop_is_stopped() {
    // Invariant 19: the probe's quota is two seconds.
    let started = Instant::now();
    match ask("loop") {
        Err(RunError::Limit(_)) => {}
        other => panic!("expected a limit, got {other:?}"),
    }
    assert!(started.elapsed().as_secs() < 10);
}

#[test]
fn memory_beyond_the_quota_is_refused() {
    // Invariant 19: the probe's quota is 64 MB; it asks for 512.
    match ask("alloc") {
        Err(RunError::Limit(_)) => {}
        other => panic!("expected a limit, got {other:?}"),
    }
}

#[test]
fn a_panic_is_a_trap_not_a_crash() {
    assert!(matches!(ask("panic"), Err(RunError::Trap(_))));
}

#[test]
fn reads_above_the_manifest_level_are_dropped() {
    // Invariant 14, the host's half: the probe reads up to personal.
    let items = vec![item("a", WitLevel::Personal), item("s", WitLevel::Secret)];
    let (o, _) = run_at(
        "probe",
        Invocation::Message("query".into()),
        Level::Public,
        items.clone(),
    );
    assert_eq!(o.result.unwrap().unwrap(), "a");
    assert_eq!(o.reads, vec!["a".to_owned()]);
    let (o, _) = run_at(
        "probe",
        Invocation::Message("get:s".into()),
        Level::Public,
        items,
    );
    assert_eq!(o.result.unwrap().unwrap(), "absent");
}

#[test]
fn a_query_returns_at_most_two_hundred() {
    let many: Vec<Item> = (0..500)
        .map(|n| item(&n.to_string(), WitLevel::Public))
        .collect();
    let (o, _) = run_at(
        "probe",
        Invocation::Message("query:1000".into()),
        Level::Public,
        many,
    );
    assert_eq!(o.result.unwrap().unwrap().split(',').count(), 200);
}

#[test]
fn a_model_call_carries_what_the_run_has_read() {
    // Invariant 20: the level is the run's taint, not the agent's word.
    let (o, seen) = run_at(
        "probe",
        Invocation::Message("model".into()),
        Level::Public,
        Vec::new(),
    );
    assert_eq!(o.result.unwrap().unwrap(), "model says hi");
    assert_eq!(seen.lock().unwrap().model_levels, vec![Level::Public]);

    let (o, seen) = run_at(
        "probe",
        Invocation::Message("model".into()),
        Level::Secret,
        Vec::new(),
    );
    assert!(o.result.is_ok());
    assert_eq!(seen.lock().unwrap().model_levels, vec![Level::Secret]);
    assert_eq!(o.taint, Level::Secret);
}

#[test]
fn reading_raises_the_run_level() {
    let items = vec![item("a", WitLevel::Personal)];
    let (o, _) = run_at(
        "probe",
        Invocation::Message("get:a".into()),
        Level::Public,
        items,
    );
    assert_eq!(o.result.unwrap().unwrap(), "a");
    assert_eq!(o.taint, Level::Personal);
}

#[test]
fn only_what_the_manifest_declares() {
    assert!(answer("model:draft").starts_with("refused"));
    assert!(answer("propose:send_everything").starts_with("refused"));
    assert_eq!(answer("propose:note"), "action-1");
    assert!(answer("blob:x").starts_with("refused"));
}

#[test]
fn undeclared_triggers_are_refused() {
    for invocation in [
        Invocation::Items(vec!["a".into()]),
        Invocation::Schedule("nightly".into()),
        Invocation::Apply {
            kind: "other".into(),
            payload: String::new(),
        },
    ] {
        let (o, _) = run_at("probe", invocation, Level::Public, Vec::new());
        assert!(
            matches!(o.result, Err(RunError::Refused(_))),
            "{:?}",
            o.result
        );
    }
}

#[test]
fn applying_touches_only_the_space() {
    let (o, seen) = run_at(
        "probe",
        Invocation::Apply {
            kind: "note".into(),
            payload: "p".into(),
        },
        Level::Public,
        Vec::new(),
    );
    assert!(matches!(o.result, Err(RunError::Agent(ref e)) if e.contains("applying")));
    assert!(seen.lock().unwrap().proposals.is_empty());
}

#[test]
fn hello_notes_mail_and_proposes() {
    let items = vec![item("m1", WitLevel::Personal), item("m2", WitLevel::Public)];
    let (doors, seen) = Fake::new(items);
    let hello = package("hello");
    let run = |invocation| Run {
        invocation,
        now_ms: 1,
        seed: 1,
        space_level: Level::Public,
    };
    let o = RUNNER.run(
        &hello,
        run(Invocation::Items(vec![
            "m1".into(),
            "m2".into(),
            "nope".into(),
        ])),
        doors,
    );
    assert!(o.result.is_ok(), "{:?}", o.result);
    assert_eq!(o.log, vec!["seen subject m1", "seen subject m2"]);
    assert_eq!(o.taint, Level::Personal);

    // The same doors again, as the daemon would hand them over.
    let doors = Box::new(Fake {
        items: Vec::new(),
        seen: Arc::clone(&seen),
    });
    let o = RUNNER.run(
        &hello,
        run(Invocation::Message("pay the rates".into())),
        doors,
    );
    assert_eq!(
        o.result.unwrap().unwrap(),
        "I have seen 2 mails. I proposed keeping your note."
    );
    assert_eq!(o.proposals, vec!["action-1".to_owned()]);
    assert_eq!(seen.lock().unwrap().proposals[0].0, "note");
}

#[test]
fn the_package_is_its_hash_and_packs_once() {
    use sha2::{Digest, Sha256};
    let p = package("hello");
    assert_eq!(p.hash, hex::encode(Sha256::digest(&p.bytes)));
    assert_eq!(p.manifest.name, "Hello");
    let again = Package::pack(&p.bytes, "name = \"x\"", None, None);
    assert!(again.is_err());
    // Unpacked, it is the bare component, and packs again to the same bytes.
    let bare = genatrix_host::package::unpack(&p.bytes).unwrap();
    assert!(bare.len() < p.bytes.len());
    let manifest = std::fs::read_to_string(format!(
        "{}/../../agents/hello/manifest.toml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    assert_eq!(
        Package::pack(&bare, &manifest, None, None).unwrap(),
        p.bytes
    );
}
