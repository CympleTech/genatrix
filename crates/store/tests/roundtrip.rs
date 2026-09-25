//! End-to-end behaviour of the store against the design documents:
//! idempotent raw ingestion, versioned items, timeline queries, trigram
//! search over CJK text, and a lossless export/import round trip.

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use genatrix_model::{
    Annotation, AnnotationKind, Blob, Confidence, Connector, ContentHash, Direction, Handle,
    HandleId, HandleKind, Item, ItemId, Level, Payload, Person, PersonId, Producer, Raw, Source,
    Thread, ThreadId, ThreadKind,
};
use genatrix_store::{DbKey, ItemQuery, ItemVersion, Store};

fn key() -> DbKey {
    DbKey::from_bytes([7; 32])
}

fn at(s: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(s).unwrap()
}

struct Fixture {
    store: Store,
    me: PersonId,
    alice: PersonId,
    thread: ThreadId,
}

fn message(f: &Fixture, ext: &str, from: PersonId, to: PersonId, when: &str, text: &str) -> Item {
    let source = Source::new(Connector::Telegram, "me", ext);
    let raw = Raw::describe(source.clone(), "application/json", text.as_bytes());
    assert!(f.store.insert_raw(&raw).unwrap());
    let dir = if from == f.me {
        Direction::Outbound
    } else {
        Direction::Inbound
    };
    let item = Item {
        id: ItemId::new(),
        source,
        raw_id: raw.id,
        supersedes: None,
        thread_id: f.thread,
        occurred_at: at(when),
        ingested_at: Utc::now(),
        direction: dir,
        author: Some(from),
        recipients: vec![to],
        text: text.to_owned(),
        blobs: vec![],
        sensitivity: Level::Personal,
        tombstoned: false,
        payload: Payload::Message {
            reply_to: None,
            forwarded_from: None,
            edited: false,
        },
    };
    f.store.insert_item(&item).unwrap();
    item
}

fn fixture() -> Fixture {
    let store = Store::open_in_memory(&key()).unwrap();
    let me = store
        .person_for_handle(HandleKind::TelegramId, "1", "Neo")
        .unwrap();
    store.set_self(me).unwrap();
    let alice = store
        .person_for_handle(HandleKind::TelegramId, "2", "Alice")
        .unwrap();
    let thread = store
        .upsert_thread(&Thread {
            id: ThreadId::new(),
            kind: ThreadKind::DirectChat,
            source: Source::new(Connector::Telegram, "me", "chat:2"),
            title: Some("Alice".into()),
            members: vec![me, alice],
            first_at: None,
            last_at: None,
        })
        .unwrap();
    Fixture {
        store,
        me,
        alice,
        thread,
    }
}

#[test]
fn raw_ingestion_is_idempotent_by_source_and_hash() {
    let f = fixture();
    let src = Source::new(Connector::Imap, "me@example.com", "gm:1");
    let a = Raw::describe(src.clone(), "message/rfc822", b"hello");
    let again = Raw::describe(src.clone(), "message/rfc822", b"hello");
    let edited = Raw::describe(src.clone(), "message/rfc822", b"hello, edited");
    assert!(f.store.insert_raw(&a).unwrap());
    assert!(
        !f.store.insert_raw(&again).unwrap(),
        "same bytes are skipped"
    );
    assert!(
        f.store.insert_raw(&edited).unwrap(),
        "changed bytes are a new raw"
    );
    assert!(f.store.has_raw(&src, &ContentHash::of(b"hello")).unwrap());
    assert_eq!(f.store.all_raw().unwrap().len(), 2);
}

#[test]
fn handles_resolve_to_one_person_and_self_is_unique() {
    let f = fixture();
    let again = f
        .store
        .person_for_handle(HandleKind::TelegramId, " 2 ", "Alice B.")
        .unwrap();
    assert_eq!(again, f.alice, "normalized handle finds the same person");
    assert_eq!(f.store.self_person().unwrap().unwrap().id, f.me);
    assert!(f.store.set_self(f.alice).is_err(), "only one self");
    f.store
        .insert_handle(&Handle {
            id: HandleId::new(),
            person_id: f.alice,
            kind: HandleKind::Email,
            value: "alice@example.com".into(),
            confidence: Confidence::Inferred,
        })
        .unwrap();
    assert_eq!(f.store.handles_of(f.alice).unwrap().len(), 2);
}

#[test]
fn timeline_orders_by_occurrence_and_hides_superseded_and_tombstoned() {
    let f = fixture();
    let old = message(
        &f,
        "2:10",
        f.alice,
        f.me,
        "2026-09-01T10:00:00+08:00",
        "first",
    );
    let _b = message(
        &f,
        "2:11",
        f.me,
        f.alice,
        "2026-09-02T10:00:00+08:00",
        "second",
    );
    // Ingested last but occurred first: must sort by occurrence.
    let _c = message(
        &f,
        "2:9",
        f.alice,
        f.me,
        "2026-08-30T10:00:00+08:00",
        "zeroth",
    );

    // An edit of the first message: same source, new raw, supersedes old.
    let src = Source::new(Connector::Telegram, "me", "2:10");
    let raw2 = Raw::describe(src.clone(), "application/json", b"first (edited)");
    f.store.insert_raw(&raw2).unwrap();
    let mut v2 = old.clone();
    v2.id = ItemId::new();
    v2.raw_id = raw2.id;
    v2.supersedes = Some(old.id);
    v2.text = "first (edited)".into();
    f.store.insert_item(&v2).unwrap();

    let current = f
        .store
        .query_items(&ItemQuery {
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    let texts: Vec<&str> = current.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(texts, ["second", "first (edited)", "zeroth"]);
    assert_eq!(
        f.store.current_item(&src).unwrap().unwrap().id,
        v2.id,
        "current version is the edit"
    );

    let all = f
        .store
        .query_items(&ItemQuery {
            version: ItemVersion::All,
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(all.len(), 4);

    // Tombstone: a new version flagged deleted, hidden by default.
    let raw3 = Raw::describe(src.clone(), "application/json", b"deleted");
    f.store.insert_raw(&raw3).unwrap();
    let mut v3 = v2.clone();
    v3.id = ItemId::new();
    v3.raw_id = raw3.id;
    v3.supersedes = Some(v2.id);
    v3.tombstoned = true;
    f.store.insert_item(&v3).unwrap();
    let visible = f
        .store
        .query_items(&ItemQuery {
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(visible.len(), 2);
    let with_deleted = f
        .store
        .query_items(&ItemQuery {
            include_tombstoned: true,
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(with_deleted.len(), 3);
}

#[test]
fn query_filters_by_direction_person_and_level() {
    let f = fixture();
    let a = message(
        &f,
        "2:1",
        f.alice,
        f.me,
        "2026-09-01T10:00:00Z",
        "from alice",
    );
    let _b = message(&f, "2:2", f.me, f.alice, "2026-09-01T11:00:00Z", "to alice");
    f.store.set_item_sensitivity(a.id, Level::Secret).unwrap();

    let outbound = f
        .store
        .query_items(&ItemQuery {
            direction: Some(Direction::Outbound),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(outbound.len(), 1);
    assert_eq!(outbound[0].text, "to alice");

    let with_alice = f
        .store
        .query_items(&ItemQuery {
            person: Some(f.alice),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(with_alice.len(), 2, "author or recipient");

    let up_to_personal = f
        .store
        .query_items(&ItemQuery {
            max_level: Some(Level::Personal),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(up_to_personal.len(), 1);
    assert_eq!(up_to_personal[0].text, "to alice");
}

#[test]
fn trigram_search_finds_cjk_substrings_and_ignores_syntax() {
    let f = fixture();
    message(
        &f,
        "2:1",
        f.alice,
        f.me,
        "2026-09-01T10:00:00Z",
        "周四能不能改到下午三点？",
    );
    message(
        &f,
        "2:2",
        f.me,
        f.alice,
        "2026-09-01T11:00:00Z",
        "Contract clause 4 is confirmed.",
    );
    let hits = f.store.search_items("下午三点", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].text.contains("下午三点"));
    let hits = f.store.search_items("clause 4", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(
        f.store
            .search_items("AND OR \"x\" NEAR(", 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        f.store.search_items("下", 10).unwrap().is_empty(),
        "too short for trigram"
    );
}

#[test]
fn export_then_import_is_lossless() {
    let f = fixture();
    let a = message(
        &f,
        "2:1",
        f.alice,
        f.me,
        "2026-09-01T10:00:00+08:00",
        "hello 你好",
    );
    f.store
        .upsert_blob(&Blob {
            hash: ContentHash::of(b"attachment"),
            mime: "application/pdf".into(),
            size: 10,
            name_hint: Some("contract.pdf".into()),
        })
        .unwrap();
    let ann = Annotation::new(
        a.id,
        Producer::Rule {
            rule: "source-default".into(),
            version: "1".into(),
        },
        AnnotationKind::Sensitivity {
            level: Level::Personal,
            reason: "direct chat".into(),
        },
    );
    f.store.insert_annotation(&ann).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let out = f.store.export_to(dir.path()).unwrap();
    assert_eq!(out.items, 1);
    assert_eq!(out.persons, 2);
    assert_eq!(out.handles, 2);
    assert_eq!(out.blobs, 1);
    assert_eq!(out.annotations, 1);
    assert!(dir.path().join("items.jsonl").exists());

    let fresh = Store::open_in_memory(&key()).unwrap();
    let inn = fresh.import_from(dir.path()).unwrap();
    assert_eq!(inn, out, "every exported row was imported");

    assert_eq!(fresh.get_item(a.id).unwrap().unwrap(), a);
    assert_eq!(fresh.all_persons().unwrap(), f.store.all_persons().unwrap());
    assert_eq!(fresh.all_handles().unwrap(), f.store.all_handles().unwrap());
    assert_eq!(fresh.all_threads().unwrap(), f.store.all_threads().unwrap());
    assert_eq!(fresh.all_raw().unwrap(), f.store.all_raw().unwrap());
    assert_eq!(fresh.all_blobs().unwrap(), f.store.all_blobs().unwrap());
    assert_eq!(
        fresh.annotations_of(a.id).unwrap(),
        f.store.annotations_of(a.id).unwrap()
    );
    // Importing again changes nothing.
    let again = fresh.import_from(dir.path()).unwrap();
    assert_eq!(again, genatrix_store::ExportSummary::default());

    // The offset survived: +08:00 in, +08:00 out.
    let back = fresh.get_item(a.id).unwrap().unwrap();
    assert_eq!(back.occurred_at.offset().local_minus_utc(), 8 * 3600);
    let _ = Utc.timestamp_opt(0, 0);
    let _ = Person {
        id: PersonId::new(),
        display_name: String::new(),
        is_self: false,
        merged_from: vec![],
    };
}

#[test]
fn folding_a_stray_self_address_rewrites_authors_recipients_and_directions() {
    // Alice turns out to be the user's second account.
    let f = fixture();
    let bob = f
        .store
        .person_for_handle(HandleKind::TelegramId, "3", "Bob")
        .unwrap();
    message(&f, "2:1", f.alice, f.me, "2026-09-01T10:00:00Z", "a to me");
    message(&f, "2:2", f.me, f.alice, "2026-09-01T11:00:00Z", "me to a");
    message(&f, "2:3", f.alice, bob, "2026-09-01T12:00:00Z", "a to bob");
    message(&f, "2:4", bob, f.me, "2026-09-01T13:00:00Z", "bob to me");

    assert_eq!(f.store.fold_person(f.alice, f.me, true).unwrap(), 3);

    let direction_of = |text: &str| {
        let all = f.store.query_items(&ItemQuery::default()).unwrap();
        let item = all.into_iter().find(|i| i.text == text).unwrap();
        assert!(!item.recipients.contains(&f.alice));
        assert_ne!(item.author, Some(f.alice));
        item.direction
    };
    assert_eq!(direction_of("a to me"), Direction::Internal);
    assert_eq!(direction_of("me to a"), Direction::Internal);
    assert_eq!(direction_of("a to bob"), Direction::Outbound);
    assert_eq!(direction_of("bob to me"), Direction::Inbound);

    let handle = f
        .store
        .find_handle(HandleKind::TelegramId, "2")
        .unwrap()
        .unwrap();
    assert_eq!(handle.person_id, f.me);
    let me = f.store.self_person().unwrap().unwrap();
    assert!(me.merged_from.contains(&f.alice));
    assert_eq!(f.store.fold_person(f.me, f.me, true).unwrap(), 0);
}
