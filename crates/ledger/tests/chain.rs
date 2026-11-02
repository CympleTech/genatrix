//! The ledger's promises from design 02 and 08: append-only, contiguous,
//! verifiable, and tamper-evident against bugs and edits.

use genatrix_keys::DbKey;
use genatrix_ledger::{Entry, EntryFilter, Error, Ledger, kind};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

fn key() -> DbKey {
    DbKey::from_bytes([3; 32])
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Egress {
    purpose: String,
    level: String,
    items: Vec<String>,
    payload_hash: String,
}

fn sample(i: usize) -> Egress {
    Egress {
        purpose: "summary".into(),
        level: "redacted".into(),
        items: vec![format!("item-{i}")],
        payload_hash: format!("{i:064x}"),
    }
}

#[test]
fn append_is_contiguous_and_bodies_round_trip() {
    let l = Ledger::open_in_memory(&key()).unwrap();
    assert!(l.is_empty().unwrap());
    let first = l.append(kind::EGRESS, "e-1", &sample(1)).unwrap();
    let second = l.append(kind::EGRESS_RESULT, "e-1", &"sent").unwrap();
    let third = l.append(kind::RUN, "r-1", &sample(3)).unwrap();
    assert_eq!((first.seq, second.seq, third.seq), (1, 2, 3));
    assert_eq!(second.prev_hash, first.hash);
    assert_eq!(third.prev_hash, second.hash);
    assert_eq!(first.decode::<Egress>().unwrap(), sample(1));
    assert_eq!(l.get(2).unwrap().unwrap(), second);
    assert_eq!(l.last().unwrap().unwrap(), third);

    let about_e1 = l
        .entries(&EntryFilter {
            subject: Some("e-1".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(about_e1.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2]);
    let runs = l
        .entries(&EntryFilter {
            kind: Some(kind::RUN.into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(runs.len(), 1);
    let v = l.verify().unwrap();
    assert_eq!(v.entries, 3);
    assert_eq!(v.head, third.hash);
}

#[test]
fn update_and_delete_are_refused_by_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    let l = Ledger::open(&path, &key()).unwrap();
    l.append(kind::ACTION, "a-1", &"proposed").unwrap();
    drop(l);

    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&format!("PRAGMA key = {};", key().pragma_literal()))
        .unwrap();
    let upd = conn.execute("UPDATE entry SET body = '\"edited\"' WHERE seq = 1", []);
    assert!(upd.unwrap_err().to_string().contains("append-only"));
    let del = conn.execute("DELETE FROM entry WHERE seq = 1", []);
    assert!(del.unwrap_err().to_string().contains("append-only"));
}

#[test]
fn tampering_is_detected_even_when_triggers_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    let l = Ledger::open(&path, &key()).unwrap();
    for i in 0..5 {
        l.append(kind::EGRESS, &format!("e-{i}"), &sample(i))
            .unwrap();
    }
    assert_eq!(l.verify().unwrap().entries, 5);
    drop(l);

    // An attacker with the key can drop the triggers. The chain still tells.
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&format!("PRAGMA key = {};", key().pragma_literal()))
        .unwrap();
    conn.execute_batch("DROP TRIGGER entry_no_update;").unwrap();
    conn.execute("UPDATE entry SET body = '\"edited\"' WHERE seq = 3", [])
        .unwrap();
    drop(conn);

    let l = Ledger::open(&path, &key()).unwrap();
    match l.verify() {
        Err(Error::ChainBroken { seq, .. }) => assert_eq!(seq, 3),
        other => panic!("expected ChainBroken at 3, got {other:?}"),
    }
}

#[test]
fn a_rolled_back_database_is_caught_by_the_head_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    let l = Ledger::open(&path, &key()).unwrap();
    l.append(kind::RUN, "r-1", &"start").unwrap();
    l.append(kind::RUN_END, "r-1", &"ok").unwrap();
    drop(l);

    // Replace the database with an older copy that has only one entry.
    let l2 = Ledger::open(dir.path().join("older.db"), &key()).unwrap();
    l2.append(kind::RUN, "r-1", &"start").unwrap();
    drop(l2);
    std::fs::remove_file(&path).unwrap();
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    std::fs::copy(dir.path().join("older.db"), &path).unwrap();

    match Ledger::open(&path, &key()) {
        Err(Error::HeadMismatch(_)) => {}
        other => panic!("expected HeadMismatch, got {:?}", other.map(|_| ())),
    }
}

/// Put the head file back to what it said after `entries` appends, as a
/// failed head write after a successful append leaves it.
fn head_left_behind(path: &std::path::Path, seq: i64, hash: &[u8; 32]) {
    std::fs::write(
        format!("{}.head", path.display()),
        format!("{seq} {}\n", hex::encode(hash)),
    )
    .unwrap();
}

#[test]
fn a_head_left_behind_by_a_failed_write_is_caught_up() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    let l = Ledger::open(&path, &key()).unwrap();
    let first = l.append(kind::RUN, "r-1", &"start").unwrap();
    l.append(kind::RUN_END, "r-1", &"ok").unwrap();
    l.append(kind::RUN, "r-2", &"start").unwrap();
    drop(l);
    head_left_behind(&path, first.seq, &first.hash);

    let l = Ledger::open(&path, &key()).unwrap();
    assert_eq!(l.verify().unwrap().entries, 3);
    assert_eq!(l.append(kind::RUN_END, "r-2", &"ok").unwrap().seq, 4);
}

#[test]
fn a_head_at_the_root_is_caught_up_too() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    let l = Ledger::open(&path, &key()).unwrap();
    l.append(kind::RUN, "r-1", &"start").unwrap();
    drop(l);
    head_left_behind(&path, 0, &[0; 32]);
    assert_eq!(
        Ledger::open(&path, &key())
            .unwrap()
            .verify()
            .unwrap()
            .entries,
        1
    );
}

#[test]
fn a_head_behind_a_database_that_does_not_run_on_from_it_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    let l = Ledger::open(&path, &key()).unwrap();
    l.append(kind::RUN, "r-1", &"start").unwrap();
    l.append(kind::RUN_END, "r-1", &"ok").unwrap();
    drop(l);
    // The head names entry 1 with a hash the database does not have.
    head_left_behind(&path, 1, &[7; 32]);
    match Ledger::open(&path, &key()) {
        Err(Error::HeadMismatch(_)) => {}
        other => panic!("expected HeadMismatch, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn reopen_continues_the_chain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    let first: Entry;
    {
        let l = Ledger::open(&path, &key()).unwrap();
        first = l.append(kind::ACTION, "a-1", &"proposed").unwrap();
    }
    let l = Ledger::open(&path, &key()).unwrap();
    let second = l.append(kind::ACTION_DECISION, "a-1", &"approved").unwrap();
    assert_eq!(second.seq, 2);
    assert_eq!(second.prev_hash, first.hash);
    assert_eq!(l.verify().unwrap().entries, 2);
}
