//! Chunks and vectors: written once, found by nearness.

use chrono::Utc;
use genatrix_model::{
    Connector, Direction, HandleKind, Item, ItemId, Level, Payload, Producer, Raw, Source, Thread,
    ThreadId, ThreadKind,
};
use genatrix_store::{DbKey, EMBEDDING_DIMS, Store};

fn store_with_items(n: usize) -> (Store, Vec<Item>) {
    let store = Store::open_in_memory(&DbKey::from_bytes([5; 32])).unwrap();
    let me = store
        .person_for_handle(HandleKind::Email, "me@example.com", "Me")
        .unwrap();
    store.set_self(me).unwrap();
    let thread = store
        .upsert_thread(&Thread {
            id: ThreadId::new(),
            kind: ThreadKind::MailThread,
            source: Source::new(Connector::Imap, "me@example.com", "t"),
            title: None,
            members: vec![me],
            first_at: None,
            last_at: None,
        })
        .unwrap();
    let mut items = Vec::new();
    for i in 0..n {
        let source = Source::new(Connector::Imap, "me@example.com", format!("m{i}"));
        let raw = Raw::describe(
            source.clone(),
            "message/rfc822",
            format!("body {i}").as_bytes(),
        );
        assert!(store.insert_raw(&raw).unwrap());
        let item = Item {
            id: ItemId::new(),
            source,
            raw_id: raw.id,
            supersedes: None,
            thread_id: thread,
            occurred_at: Utc::now().into(),
            ingested_at: Utc::now(),
            direction: Direction::Inbound,
            author: Some(me),
            recipients: vec![],
            text: format!("message number {i}"),
            blobs: vec![],
            sensitivity: Level::Personal,
            tombstoned: false,
            payload: Payload::Mail {
                subject: format!("s{i}"),
                from: String::new(),
                to: vec![],
                cc: vec![],
                message_id: None,
                in_reply_to: None,
                references: vec![],
                labels: vec![],
                headers: vec![],
            },
        };
        store.insert_item(&item).unwrap();
        items.push(item);
    }
    (store, items)
}

fn unit(axis: usize) -> Vec<f32> {
    let mut v = vec![0.0; EMBEDDING_DIMS];
    v[axis] = 1.0;
    v
}

fn producer() -> Producer {
    Producer::Model {
        model: "embed".into(),
        prompt_version: "chunk/1".into(),
    }
}

#[test]
fn the_nearest_item_comes_first_and_each_item_once() {
    let (store, items) = store_with_items(3);
    assert_eq!(store.items_without_embedding(10).unwrap().len(), 3);

    store
        .put_embedding(items[0].id, 0, (0, 5), &unit(0), &producer())
        .unwrap();
    store
        .put_embedding(items[0].id, 1, (5, 9), &unit(1), &producer())
        .unwrap();
    store
        .put_embedding(items[1].id, 0, (0, 9), &unit(2), &producer())
        .unwrap();

    let mut query = vec![0.0; EMBEDDING_DIMS];
    query[0] = 0.9;
    query[1] = 0.1;
    let hits = store.similar_items(&query, 10).unwrap();
    assert_eq!(hits.len(), 2, "two items have vectors; one of them twice");
    assert_eq!(hits[0].0.id, items[0].id);
    assert!(hits[0].1 < hits[1].1, "nearer first");

    let left = store.items_without_embedding(10).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, items[2].id);
    assert_eq!(store.count_embedded_items().unwrap(), 2);
    assert_eq!(store.count_annotated_items("embedding").unwrap(), 2);
}

#[test]
fn a_chunk_written_again_replaces_its_vector() {
    let (store, items) = store_with_items(1);
    store
        .put_embedding(items[0].id, 0, (0, 5), &unit(0), &producer())
        .unwrap();
    store
        .put_embedding(items[0].id, 0, (0, 5), &unit(3), &producer())
        .unwrap();
    let hits = store.similar_items(&unit(3), 5).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].1 < 0.01, "the new vector is what is indexed");
    let far = store.similar_items(&unit(0), 5).unwrap();
    assert!(far[0].1 > 1.0, "the old one is gone");
}

#[test]
fn a_wrong_dimension_is_refused() {
    let (store, items) = store_with_items(1);
    assert!(
        store
            .put_embedding(items[0].id, 0, (0, 1), &[1.0, 2.0], &producer())
            .is_err()
    );
}
