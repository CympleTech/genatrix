//! Junk (design 01, "垃圾"; design 06, "标为垃圾"): a person, or a group or
//! channel, the user does not want. What is there is removed, files and
//! all; what arrives later is dropped in ingest. The ledger keeps the
//! counts, not the content.
//!
//! Deleting a conversation (design 01 v0.7, "删除对话") removes the same
//! things without the mark: what arrives later is kept. What was there when
//! it was deleted does not come back, even if a connector fetches it again.

use anyhow::{Context, bail};
use genatrix_model::{PersonId, ThreadId, ThreadKind};
use genatrix_store::Purged;

use crate::system::System;

/// What marking would remove.
pub fn count_person(system: &System, person: PersonId) -> anyhow::Result<usize> {
    Ok(system.store.person_conversation_items(person)?.len())
}

/// What marking would remove.
pub fn count_thread(system: &System, thread: ThreadId) -> anyhow::Result<usize> {
    Ok(system.store.thread_items(thread)?.len())
}

/// Mark a person as junk and remove their conversation with the user.
pub fn mark_person(system: &System, person: PersonId) -> anyhow::Result<Purged> {
    let found = system.store.get_person(person)?.context("no such person")?;
    if found.is_self {
        bail!("you cannot mark yourself as junk");
    }
    let ids = system.store.person_conversation_items(person)?;
    let done = remove(system, &ids)?;
    system
        .store
        .set_person_junk(person, Some(&chrono::Utc::now().to_rfc3339()))?;
    record(system, "person", &person.to_string(), &done)?;
    Ok(done)
}

/// Mark a group or channel as junk and remove its messages.
pub fn mark_thread(system: &System, thread: ThreadId) -> anyhow::Result<Purged> {
    let found = system
        .store
        .get_thread(thread)?
        .context("no such conversation")?;
    if !matches!(found.kind, ThreadKind::GroupChat | ThreadKind::Channel) {
        bail!("only a group or a channel is marked this way; mark the person instead");
    }
    let ids = system.store.thread_items(thread)?;
    let done = remove(system, &ids)?;
    system
        .store
        .set_thread_junk(thread, Some(&chrono::Utc::now().to_rfc3339()))?;
    record(system, "thread", &thread.to_string(), &done)?;
    Ok(done)
}

/// Delete a person's conversation with the user, and keep what they send
/// from now on.
pub fn delete_person(system: &System, person: PersonId) -> anyhow::Result<Purged> {
    let found = system.store.get_person(person)?.context("no such person")?;
    if found.is_self {
        bail!("you cannot delete your own conversation");
    }
    let ids = system.store.person_conversation_items(person)?;
    let done = remove(system, &ids)?;
    system
        .store
        .set_person_cleared(person, &chrono::Utc::now().to_rfc3339())?;
    record_as(system, "deleted", "person", &person.to_string(), &done)?;
    Ok(done)
}

/// Delete the messages in a group or channel, and keep what comes from now
/// on.
pub fn delete_thread(system: &System, thread: ThreadId) -> anyhow::Result<Purged> {
    let found = system
        .store
        .get_thread(thread)?
        .context("no such conversation")?;
    if !matches!(found.kind, ThreadKind::GroupChat | ThreadKind::Channel) {
        bail!(
            "only a group or a channel is deleted this way; delete the person's conversation instead"
        );
    }
    let ids = system.store.thread_items(thread)?;
    let done = remove(system, &ids)?;
    system
        .store
        .set_thread_cleared(thread, &chrono::Utc::now().to_rfc3339())?;
    record_as(system, "deleted", "thread", &thread.to_string(), &done)?;
    Ok(done)
}

/// Take the mark away: new messages are kept again. What was removed stays
/// removed.
pub fn restore(system: &System, kind: &str, id: &str) -> anyhow::Result<()> {
    match kind {
        "person" => system
            .store
            .set_person_junk(id.parse().context("not a person")?, None)?,
        "thread" => system
            .store
            .set_thread_junk(id.parse().context("not a conversation")?, None)?,
        other => bail!("nothing called {other} is marked"),
    }
    system
        .ledger
        .append("junk_restored", id, &serde_json::json!({ "kind": kind }))?;
    forget_overviews(system);
    Ok(())
}

/// Take something off the junk list for good (design 01 v0.7): the mark
/// goes, and it is as if its conversation had been deleted now. What was
/// removed, and what was dropped while it was marked, does not come back
/// when fetched again; what is sent from now on is kept.
pub fn forget(system: &System, kind: &str, id: &str) -> anyhow::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    match kind {
        "person" => {
            let person = id.parse().context("not a person")?;
            system.store.set_person_cleared(person, &now)?;
            system.store.set_person_junk(person, None)?;
        }
        "thread" => {
            let thread = id.parse().context("not a conversation")?;
            system.store.set_thread_cleared(thread, &now)?;
            system.store.set_thread_junk(thread, None)?;
        }
        other => bail!("nothing called {other} is marked"),
    }
    system
        .ledger
        .append("junk_forgotten", id, &serde_json::json!({ "kind": kind }))?;
    forget_overviews(system);
    Ok(())
}

fn remove(system: &System, ids: &std::collections::HashSet<String>) -> anyhow::Result<Purged> {
    let done = system.store.remove_items(ids)?;
    for h in &done.raw_files {
        let _ = system.raw_files.remove(h);
    }
    for h in &done.blob_files {
        let _ = system.blob_files.remove(h);
    }
    forget_overviews(system);
    Ok(done)
}

fn record(system: &System, kind: &str, id: &str, done: &Purged) -> anyhow::Result<()> {
    record_as(system, "junk", kind, id, done)
}

fn record_as(
    system: &System,
    what: &str,
    kind: &str,
    id: &str,
    done: &Purged,
) -> anyhow::Result<()> {
    system.ledger.append(
        what,
        id,
        &serde_json::json!({
            "kind": kind,
            "items": done.items,
            "raws": done.raws,
            "commitments": done.commitments,
            "files": done.raw_files.len() + done.blob_files.len(),
        }),
    )?;
    Ok(())
}

/// The people and groups lists counted what just went.
fn forget_overviews(system: &System) {
    *system
        .people
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    *system
        .groups
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}
