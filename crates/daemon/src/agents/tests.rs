//! Agents in a whole core: the read scope against real items (invariant
//! 14), one space per agent (15), only approved bytes run (17), and the
//! level of a model call (20) as the daemon hands it to the gate.

use std::sync::Arc;

use chrono::Utc;
use genatrix_host::manifest::Purpose;
use genatrix_host::{Doors, Invocation, wit};
use genatrix_model::Connector;
use genatrix_model::{
    Blob, ContentHash, Direction, HandleKind, Item, ItemId, Level, Payload, Raw, Source, Thread,
    ThreadId, ThreadKind,
};
use genatrix_store::{Space, SpaceValue};

use super::doors::StoreDoors;
use super::*;
use crate::system::{System, test_system};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../host/tests/fixtures/{name}.wasm",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(path).unwrap()
}

/// A package with this manifest around the hello component.
fn with_manifest(manifest: &str) -> Vec<u8> {
    let bare = genatrix_host::package::unpack(&fixture("hello")).unwrap();
    Package::pack(&bare, manifest, None, None).unwrap()
}

struct Seeded {
    in_scope: ItemId,
    telegram: ItemId,
    secret: ItemId,
    old: ItemId,
    no_pdf: ItemId,
}

fn mail(
    system: &System,
    thread: ThreadId,
    connector: Connector,
    subject: &str,
    days_ago: i64,
    level: Level,
    pdf: bool,
) -> ItemId {
    let store = &system.store;
    let ext = ulid::Ulid::new().to_string();
    let source = Source::new(connector, "me@example.com", &ext);
    let raw = Raw::describe(source.clone(), "message/rfc822", ext.as_bytes());
    store.insert_raw(&raw).unwrap();
    let author = store
        .person_for_handle(HandleKind::Email, "billing@power.nz", "Power Co")
        .unwrap();
    let mut blobs = Vec::new();
    if pdf {
        let hash = ContentHash::of(ext.as_bytes());
        store
            .upsert_blob(&Blob {
                hash,
                mime: "application/pdf".into(),
                size: 10,
                name_hint: Some("invoice.pdf".into()),
            })
            .unwrap();
        blobs.push(hash);
    }
    let when = Utc::now() - chrono::Duration::days(days_ago);
    let item = Item {
        id: ItemId::new(),
        source,
        raw_id: raw.id,
        supersedes: None,
        thread_id: thread,
        occurred_at: when.fixed_offset(),
        ingested_at: Utc::now(),
        direction: Direction::Inbound,
        author: Some(author),
        recipients: Vec::new(),
        text: format!("{subject} body"),
        blobs,
        sensitivity: level,
        tombstoned: false,
        payload: Payload::Mail {
            subject: subject.into(),
            from: "billing@power.nz".into(),
            to: Vec::new(),
            cc: Vec::new(),
            message_id: None,
            in_reply_to: None,
            references: Vec::new(),
            labels: Vec::new(),
            headers: Vec::new(),
        },
    };
    store.insert_item(&item).unwrap();
    item.id
}

fn seed(system: &System) -> Seeded {
    let thread = system
        .store
        .upsert_thread(&Thread {
            id: ThreadId::new(),
            kind: ThreadKind::MailThread,
            source: Source::new(Connector::Imap, "me@example.com", "t1"),
            title: Some("Power".into()),
            members: Vec::new(),
            first_at: None,
            last_at: None,
        })
        .unwrap();
    Seeded {
        in_scope: mail(
            system,
            thread,
            Connector::Imap,
            "Invoice 42",
            2,
            Level::Personal,
            true,
        ),
        telegram: mail(
            system,
            thread,
            Connector::Telegram,
            "Invoice tg",
            2,
            Level::Personal,
            true,
        ),
        secret: mail(
            system,
            thread,
            Connector::Imap,
            "Invoice secret",
            2,
            Level::Secret,
            true,
        ),
        old: mail(
            system,
            thread,
            Connector::Imap,
            "Invoice old",
            400,
            Level::Personal,
            true,
        ),
        no_pdf: mail(
            system,
            thread,
            Connector::Imap,
            "Invoice text only",
            2,
            Level::Personal,
            false,
        ),
    }
}

const SCOPED: &str = r#"
name = "Scoped"
purpose = "Reads PDF invoices from mail in the last year"
author = "tests"
model = ["extract"]

[reads]
connectors = ["imap"]
kinds = ["mail"]
with_attachment = true
mime = ["application/pdf"]
matching = ["invoice"]
days = 365
max_level = "personal"

[[proposes]]
kind = "note"
label = "Keep a note"

[[triggers]]
on = "items"

[[triggers]]
on = "message"
"#;

fn doors_for(
    system: &Arc<System>,
    agent: &StoredAgent,
    handle: tokio::runtime::Handle,
) -> StoreDoors {
    let package = load(system, agent).unwrap();
    let space =
        Arc::new(Space::open_in_memory(&genatrix_keys::DbKey::from_bytes([9; 32]), 10).unwrap());
    StoreDoors::new(
        Arc::clone(system),
        agent.clone(),
        package.manifest,
        space,
        "run".into(),
        Utc::now(),
        handle,
    )
}

fn all() -> wit::Filter {
    wit::Filter {
        since_ms: None,
        until_ms: None,
        text: None,
        limit: 200,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_reads_only_its_scope() {
    // Invariant 14: connector, kind, attachment type, words, time window and
    // level all bound what the doors return, and the agent's filter only
    // narrows.
    let (_dir, system) = test_system();
    let s = seed(&system);
    let agent = install(&system, with_manifest(SCOPED), 1).unwrap();
    let handle = tokio::runtime::Handle::current();
    let sys = Arc::clone(&system);
    let (ids, gets, early) = tokio::task::spawn_blocking(move || {
        let mut doors = doors_for(&sys, &agent, handle);
        let ids: Vec<String> = doors.query(&all()).into_iter().map(|i| i.id).collect();
        let gets: Vec<bool> = [s.in_scope, s.telegram, s.secret, s.old, s.no_pdf]
            .iter()
            .map(|id| doors.get(&id.to_string()).is_some())
            .collect();
        // Asking for everything since the epoch still stops at a year.
        let mut wide = all();
        wide.since_ms = Some(0);
        let early: Vec<String> = doors.query(&wide).into_iter().map(|i| i.id).collect();
        (ids, gets, early)
    })
    .await
    .unwrap();
    assert_eq!(ids, vec![s.in_scope.to_string()]);
    assert_eq!(gets, vec![true, false, false, false, false]);
    assert_eq!(early, vec![s.in_scope.to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_manifest_that_reads_nothing_gets_nothing() {
    let (_dir, system) = test_system();
    seed(&system);
    let none = SCOPED.replace("connectors = [\"imap\"]", "connectors = []");
    let none = none.replace("with_attachment = true\nmime = [\"application/pdf\"]\n", "");
    let agent = install(&system, with_manifest(&none), 1).unwrap();
    let handle = tokio::runtime::Handle::current();
    let sys = Arc::clone(&system);
    let found =
        tokio::task::spawn_blocking(move || doors_for(&sys, &agent, handle).query(&all()).len())
            .await
            .unwrap();
    assert_eq!(found, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_call_goes_to_the_gate_as_the_agent_at_the_run_level() {
    // Invariant 20 on the daemon's side, and ruling 12: the request names
    // the installed agent, keeps it local, and carries the level it is given.
    let (_dir, system) = test_system();
    let s = seed(&system);
    let agent = install(&system, with_manifest(SCOPED), 1).unwrap();
    let handle = tokio::runtime::Handle::current();
    let sys = Arc::clone(&system);
    let id = agent.id.clone();
    let request = tokio::task::spawn_blocking(move || {
        let mut doors = doors_for(&sys, &agent, handle);
        doors.get(&s.in_scope.to_string()).unwrap();
        doors.request(
            Purpose::Extract,
            vec![wit::Message {
                role: "user".into(),
                content: "x".into(),
            }],
            Level::Secret,
        )
    })
    .await
    .unwrap();
    assert_eq!(request.level, Level::Secret);
    assert_eq!(request.items, vec![s.in_scope]);
    match request.initiator {
        genatrix_gate::gate::Initiator::Installed { agent, cloud, .. } => {
            assert_eq!(agent, id);
            assert!(!cloud);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_approved_bytes_run() {
    // Invariant 17: a replaced package file is refused, not run.
    let (_dir, system) = test_system();
    let agent = install(&system, fixture("hello"), 1).unwrap();
    assert!(load(&system, &agent).is_ok());
    let path = system.agents.package_path(&agent.id, &agent.version);
    let other = with_manifest(&SCOPED.replace("Scoped", "Impostor"));
    std::fs::write(&path, other).unwrap();
    let e = load(&system, &agent).unwrap_err();
    assert!(
        e.to_string().contains("not the version you approved"),
        "{e}"
    );
    let e = run(
        Arc::clone(&system),
        &agent.id,
        Invocation::Items(Vec::new()),
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("not the version you approved"));
}

#[tokio::test(flavor = "multi_thread")]
async fn each_agent_has_its_own_space() {
    // Invariant 15: two agents, two files, two keys; neither sees the other.
    let (_dir, system) = test_system();
    let a = install(&system, fixture("hello"), 1).unwrap();
    let b = install(&system, fixture("hello"), 2).unwrap();
    let sa = system.agents.space(&a.id, 10).unwrap();
    let sb = system.agents.space(&b.id, 10).unwrap();
    sa.execute("CREATE TABLE secret (x)", &[]).unwrap();
    sa.execute("INSERT INTO secret VALUES ('a only')", &[])
        .unwrap();
    assert!(sb.query("SELECT * FROM secret", &[]).is_err());
    let a_file = system.config.agents_dir().join(&a.id).join("space.db");
    let attach = format!("ATTACH DATABASE '{}' AS a", a_file.display());
    assert!(sb.execute(&attach, &[]).is_err());
    // And a's file does not open with b's key.
    assert!(Space::open(&a_file, &system.agents.master.space_key(&b.id), 10).is_err());
    assert_eq!(
        sa.query("SELECT x FROM secret", &[]).unwrap(),
        vec![vec![SpaceValue::Text("a only".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_is_recorded_and_raises_the_space() {
    let (_dir, system) = test_system();
    let s = seed(&system);
    let hello = fixture("hello");
    let manifest = Package::read(hello).unwrap().manifest;
    assert_eq!(manifest.reads.days, Some(7));
    let agent = install(&system, fixture("hello"), 1).unwrap();
    let outcome = run(
        Arc::clone(&system),
        &agent.id,
        Invocation::Items(vec![s.in_scope.to_string(), s.telegram.to_string()]),
    )
    .await
    .unwrap();
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    assert_eq!(outcome.reads, vec![s.in_scope.to_string()]);
    assert_eq!(outcome.log, vec!["seen Invoice 42"]);

    let runs = system.store.agent_runs(&agent.id, 5).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome, "ok");
    assert_eq!(runs[0].level, Level::Personal);
    let now = system.store.get_agent(&agent.id).unwrap().unwrap();
    assert_eq!(now.space_level, Level::Personal);

    // What it kept is in its space.
    let space = system.agents.space(&agent.id, 50).unwrap();
    assert_eq!(
        space.query("SELECT title FROM seen", &[]).unwrap(),
        vec![vec![SpaceValue::Text("Invoice 42".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paused_agent_is_not_woken() {
    let (_dir, system) = test_system();
    let agent = install(&system, fixture("hello"), 1).unwrap();
    system
        .store
        .set_agent_state(&agent.id, genatrix_store::AgentState::Paused)
        .unwrap();
    assert!(
        run(
            Arc::clone(&system),
            &agent.id,
            Invocation::Items(Vec::new())
        )
        .await
        .is_err()
    );
}
